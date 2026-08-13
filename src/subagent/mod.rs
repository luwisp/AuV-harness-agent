use crate::error::{HarnessError, Result};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// How the subagent is isolated from the parent agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationMode {
    /// Run in the same process as the parent.
    SameProcess,
    /// Run in a separate git worktree.
    Worktree,
}

/// The result returned by a subagent after completing its task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentResult {
    pub summary: String,
    pub success: bool,
}

/// A trait for running an agent to complete a task.
///
/// This allows the [`SubagentSpawner`] to be tested with mock runners
/// without needing a full agent loop wired up.
#[async_trait]
pub trait AgentRunner: Send + Sync {
    /// Run the agent for the given task and return a summary string.
    ///
    /// `spawner` 是**子层** spawner（深度 +1），生产 Runner 用它注册
    /// 递归 subagent 工具，实现嵌套委派。
    async fn run(&self, task: &str, spawner: &SubagentSpawner) -> Result<String>;
}

/// Manages spawning of subagents with depth and concurrency limits.
///
/// # Limits
///
/// - **Depth limit**: prevents infinite recursion. If `depth >= max_depth`,
///   the spawn returns [`HarnessError::RecursionDepthExceeded`].
/// - **Concurrency limit**: caps the total number of simultaneously active
///   subagents. If the active count reaches `max_total_agents`, the spawn
///   returns [`HarnessError::SubagentLimitReached`].
pub struct SubagentSpawner {
    max_depth: usize,
    max_total_agents: usize,
    /// 当前 spawner 所在的嵌套深度（根为 0，每层子 agent +1）。
    depth: usize,
    active_count: Arc<AtomicUsize>,
    runner: Arc<dyn AgentRunner>,
}

impl SubagentSpawner {
    /// Create a new root [`SubagentSpawner`]（depth = 0）。
    ///
    /// `max_depth` is the maximum nesting depth allowed (0-based).
    /// `max_total_agents` is the maximum number of concurrently active subagents.
    /// `runner` is the agent execution strategy (real agent loop, or mock for tests).
    pub fn new(max_depth: usize, max_total_agents: usize, runner: Arc<dyn AgentRunner>) -> Self {
        Self {
            max_depth,
            max_total_agents,
            depth: 0,
            active_count: Arc::new(AtomicUsize::new(0)),
            runner,
        }
    }

    /// Create the child spawner for the next nesting level (depth + 1).
    ///
    /// Shares `active_count`（全链共享并发上限）与 `runner`（复用执行策略），
    /// 仅深度递增。
    pub fn for_child(&self) -> Arc<Self> {
        Arc::new(Self {
            max_depth: self.max_depth,
            max_total_agents: self.max_total_agents,
            depth: self.depth + 1,
            active_count: Arc::clone(&self.active_count),
            runner: Arc::clone(&self.runner),
        })
    }

    /// Spawn a subagent to handle the given task.
    ///
    /// # Errors
    ///
    /// - `RecursionDepthExceeded` if `depth >= max_depth`.
    /// - `Config` if isolation is [`IsolationMode::Worktree`]（尚未实现）.
    /// - `SubagentLimitReached` if the active agent count is already at
    ///   `max_total_agents`.
    ///
    /// # Returns
    ///
    /// On success the agent's output is captured in a [`SubagentResult`].
    /// If the agent itself fails, the error is captured as a failed
    /// `SubagentResult` (success = false) rather than propagated, so the
    /// caller always receives a result struct.
    pub async fn spawn(&self, task: &str, isolation: IsolationMode) -> Result<SubagentResult> {
        // Check depth limit
        if self.depth >= self.max_depth {
            return Err(HarnessError::RecursionDepthExceeded);
        }

        // Worktree 隔离尚未实现（仅 SameProcess 可用）
        if isolation == IsolationMode::Worktree {
            return Err(HarnessError::Config(
                "Worktree 隔离模式尚未实现，仅支持 SameProcess".to_string(),
            ));
        }

        // Check total agent concurrency limit
        if self.active_count.load(Ordering::SeqCst) >= self.max_total_agents {
            return Err(HarnessError::SubagentLimitReached);
        }

        // Increment active count before running the agent
        self.active_count.fetch_add(1, Ordering::SeqCst);

        // Run the agent；子层 spawner 用于递归委派
        let child = self.for_child();
        let result = self.runner.run(task, &child).await;

        // Decrement active count after the agent completes
        self.active_count.fetch_sub(1, Ordering::SeqCst);

        match result {
            Ok(summary) => Ok(SubagentResult {
                summary,
                success: true,
            }),
            Err(e) => {
                // Capture the error as a failed subagent result rather than
                // propagating it, so the caller always receives a result struct.
                Ok(SubagentResult {
                    summary: format!("Subagent failed: {}", e),
                    success: false,
                })
            }
        }
    }

    /// Return the current number of active subagents.
    pub fn active_count(&self) -> usize {
        self.active_count.load(Ordering::SeqCst)
    }

    /// Return this spawner's nesting depth (root = 0).
    pub fn depth(&self) -> usize {
        self.depth
    }
}

// ============================================================================
// AgentLoopRunner：生产 Runner（工厂闭包构建子 AgentLoop）
// ============================================================================

/// 生产环境的 runner：通过工厂闭包为每次委派构建独立的子 AgentLoop。
///
/// 工厂在 main.rs 装配（捕获配置、API key、审批模式等），子 loop 通过
/// 传入的**子层** spawner 注册递归 subagent 工具，实现嵌套委派。
pub struct AgentLoopRunner {
    factory: Arc<dyn Fn(&SubagentSpawner) -> Result<crate::r#loop::AgentLoop> + Send + Sync>,
}

impl AgentLoopRunner {
    pub fn new(
        factory: Arc<dyn Fn(&SubagentSpawner) -> Result<crate::r#loop::AgentLoop> + Send + Sync>,
    ) -> Self {
        Self { factory }
    }
}

#[async_trait]
impl AgentRunner for AgentLoopRunner {
    async fn run(&self, task: &str, spawner: &SubagentSpawner) -> Result<String> {
        let mut child_loop = (self.factory)(spawner)?;
        let (result, _messages) = child_loop.run_with_history(task, &[]).await?;
        Ok(result)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HarnessError;

    /// A mock runner that returns a fixed string.
    struct MockRunner {
        result: String,
    }

    #[async_trait]
    impl AgentRunner for MockRunner {
        async fn run(&self, _task: &str, _spawner: &SubagentSpawner) -> Result<String> {
            Ok(self.result.clone())
        }
    }

    /// A mock runner that sleeps briefly, used to test concurrency limits.
    struct SlowRunner;

    #[async_trait]
    impl AgentRunner for SlowRunner {
        async fn run(&self, _task: &str, _spawner: &SubagentSpawner) -> Result<String> {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok("slow done".to_string())
        }
    }

    // -----------------------------------------------------------------------
    // Test: Subagent returns a summary
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_subagent_returns_summary() {
        let spawner = SubagentSpawner::new(
            3, // max_depth
            5, // max_total_agents
            Arc::new(MockRunner {
                result: "Task completed successfully.".to_string(),
            }),
        );

        let result = spawner.spawn("Do something", IsolationMode::SameProcess).await;

        assert!(result.is_ok(), "expected Ok, got {:?}", result);
        let subagent_result = result.unwrap();
        assert!(subagent_result.success);
        assert_eq!(subagent_result.summary, "Task completed successfully.");
    }

    // -----------------------------------------------------------------------
    // Test: Depth limit is enforced
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_subagent_depth_limit() {
        let spawner = SubagentSpawner::new(
            3, // max_depth = 3, so depths 0, 1, 2 are allowed
            5,
            Arc::new(MockRunner {
                result: "ok".to_string(),
            }),
        );

        // Depths 0, 1, 2 should all succeed
        let result = spawner.spawn("task", IsolationMode::SameProcess).await;
        assert!(result.is_ok(), "depth 0 should be allowed, got {:?}", result);

        let child = spawner.for_child();
        let result = child.spawn("task", IsolationMode::SameProcess).await;
        assert!(result.is_ok(), "depth 1 should be allowed, got {:?}", result);

        let grandchild = child.for_child();
        let result = grandchild.spawn("task", IsolationMode::SameProcess).await;
        assert!(result.is_ok(), "depth 2 should be allowed, got {:?}", result);

        // Depth 3 should fail (>= max_depth)
        let great_grandchild = grandchild.for_child();
        let result = great_grandchild
            .spawn("task", IsolationMode::SameProcess)
            .await;
        assert!(result.is_err(), "depth 3 should be rejected");
        match result.unwrap_err() {
            HarnessError::RecursionDepthExceeded => {}
            other => panic!("expected RecursionDepthExceeded, got {:?}", other),
        }

        // Depth 4 should also fail
        let result = great_grandchild
            .for_child()
            .spawn("task", IsolationMode::SameProcess)
            .await;
        assert!(result.is_err(), "depth 4 should be rejected");
        match result.unwrap_err() {
            HarnessError::RecursionDepthExceeded => {}
            other => panic!("expected RecursionDepthExceeded, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test: for_child shares the active counter (global concurrency cap)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_for_child_shares_active_count() {
        let spawner = SubagentSpawner::new(10, 5, Arc::new(MockRunner {
            result: "ok".to_string(),
        }));
        let child = spawner.for_child();
        let grandchild = child.for_child();

        // 子层 spawn 后，根层应看到计数 +1（共享同一计数器）
        let result = grandchild.spawn("task", IsolationMode::SameProcess).await;
        assert!(result.is_ok());
        assert_eq!(spawner.active_count(), 0, "spawn 完成后计数应归零");

        // 借用 barrier 验证共享：SlowRunner 挂起期间根层计数可见
        let spawner2 = Arc::new(SubagentSpawner::new(10, 2, Arc::new(SlowRunner)));
        let child2 = spawner2.for_child();
        let handle = tokio::spawn(async move {
            child2.spawn("task", IsolationMode::SameProcess).await
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(spawner2.active_count(), 1, "子层运行中，根层应看到计数 1");
        let result = handle.await.unwrap();
        assert!(result.is_ok());
        assert_eq!(spawner2.active_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Test: Worktree isolation is explicitly unsupported
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_worktree_isolation_unsupported() {
        let spawner = SubagentSpawner::new(
            3,
            5,
            Arc::new(MockRunner {
                result: "ok".to_string(),
            }),
        );

        let result = spawner.spawn("task", IsolationMode::Worktree).await;
        match result.unwrap_err() {
            HarnessError::Config(msg) => assert!(msg.contains("尚未实现"), "错误信息应说明未实现：{}", msg),
            other => panic!("expected Config error, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test: Total agent concurrency limit is enforced
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_subagent_total_limit() {
        let spawner = Arc::new(SubagentSpawner::new(
            10, // generous depth
            2,  // max_total_agents = 2
            Arc::new(SlowRunner),
        ));

        // Spawn 2 agents concurrently — both should succeed
        let s1 = spawner.clone();
        let s2 = spawner.clone();

        let handle1 = tokio::spawn(async move {
            s1.spawn("task1", IsolationMode::SameProcess).await
        });
        let handle2 = tokio::spawn(async move {
            s2.spawn("task2", IsolationMode::SameProcess).await
        });

        let (r1, r2) = tokio::join!(handle1, handle2);
        assert!(r1.unwrap().is_ok(), "first concurrent agent should succeed");
        assert!(r2.unwrap().is_ok(), "second concurrent agent should succeed");

        // After both complete, active count should be back to 0
        assert_eq!(spawner.active_count(), 0);

        // Now verify that a 3rd agent is rejected while 2 are active.
        // We use a Barrier to hold 2 agents in-flight, then try a 3rd.
        let barrier = Arc::new(tokio::sync::Barrier::new(3));

        struct BarrierRunner {
            barrier: Arc<tokio::sync::Barrier>,
        }

        #[async_trait]
        impl AgentRunner for BarrierRunner {
            async fn run(&self, _task: &str, _spawner: &SubagentSpawner) -> Result<String> {
                // Block at the barrier, keeping the agent "active"
                self.barrier.wait().await;
                Ok("done".to_string())
            }
        }

        let spawner2 = Arc::new(SubagentSpawner::new(
            10,
            2,
            Arc::new(BarrierRunner {
                barrier: barrier.clone(),
            }),
        ));

        let s3 = spawner2.clone();
        let s4 = spawner2.clone();

        // Spawn 2 agents — they will block at the barrier
        let h3 = tokio::spawn(async move {
            s3.spawn("task3", IsolationMode::SameProcess).await
        });
        let h4 = tokio::spawn(async move {
            s4.spawn("task4", IsolationMode::SameProcess).await
        });

        // Give them time to increment active_count and reach the barrier
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // active_count should now be 2
        assert_eq!(spawner2.active_count(), 2);

        // Try to spawn a 3rd — should be rejected
        let result3 = spawner2
            .spawn("task5", IsolationMode::SameProcess)
            .await;
        assert!(
            result3.is_err(),
            "3rd concurrent agent should be rejected, got {:?}",
            result3
        );
        match result3.unwrap_err() {
            HarnessError::SubagentLimitReached => {}
            other => panic!("expected SubagentLimitReached, got {:?}", other),
        }

        // Release the barrier so the first 2 agents can complete
        barrier.wait().await;
        let (r3, r4) = tokio::join!(h3, h4);
        assert!(r3.unwrap().is_ok());
        assert!(r4.unwrap().is_ok());

        // After all complete, active count should be 0 again
        assert_eq!(spawner2.active_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Tests: AgentLoopRunner
    // -----------------------------------------------------------------------

    /// 构建一个最小可运行的子 AgentLoop：mock LLM 直接返回最终答案。
    fn build_minimal_loop(content: &str) -> crate::r#loop::AgentLoop {
        use crate::feedback::FeedbackRunner;
        use crate::guardrails::rules::StaticRuleEngine;
        use crate::guardrails::sandbox::SandboxBoundary;
        use crate::guardrails::GuardrailPipeline;
        use crate::llm::mock::MockLlmProvider;
        use crate::memory::MemoryStore;
        use crate::r#loop::context::ContextBuilder;
        use crate::types::{FinishReason, LlmResponse, TokenUsage};

        let mock = MockLlmProvider::new(vec![LlmResponse {
            content: content.to_string(),
            reasoning_content: None,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage::default(),
            tool_calls: None,
        }]);
        let rules = StaticRuleEngine::new();
        let assessors: Vec<Box<dyn crate::guardrails::assessor::RiskAssessor>> = vec![];
        let sandbox = SandboxBoundary {
            workspace_root: std::path::PathBuf::from("/tmp"),
            allowed_commands: vec![],
            forbidden_commands: vec![],
            max_timeout: std::time::Duration::from_secs(300),
            network_allowed: true,
        };
        let pipeline = GuardrailPipeline::for_testing(rules, assessors, sandbox);
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let memory = MemoryStore::new(tempdir.path().to_path_buf()).expect("memory store");
        let mut config = crate::config::HarnessConfig::default();
        config.agent.max_turns = 10;
        config.agent.token_budget = None;

        crate::r#loop::AgentLoop::new(
            Box::new(mock),
            pipeline,
            crate::tools::ToolRegistry::new(),
            FeedbackRunner::new(vec![], 0),
            memory,
            config,
            ContextBuilder::new(
                "You are a test subagent.".to_string(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ),
            std::path::PathBuf::from("/tmp"),
            None,
        )
    }

    #[tokio::test]
    async fn test_agent_loop_runner_returns_child_summary() {
        let runner = AgentLoopRunner::new(Arc::new(|_spawner| {
            Ok(build_minimal_loop("计算完成：结果为 4"))
        }));
        let spawner = SubagentSpawner::new(3, 5, Arc::new(runner));
        let result = spawner
            .spawn("计算 2+2", IsolationMode::SameProcess)
            .await;
        let subagent_result = result.expect("spawn 应成功");
        assert!(subagent_result.success);
        assert_eq!(subagent_result.summary, "计算完成：结果为 4");
    }

    #[tokio::test]
    async fn test_agent_loop_runner_receives_child_spawner() {
        // 工厂应收到**子层** spawner（深度 +1），供子 loop 注册递归工具
        let received_depth = Arc::new(std::sync::Mutex::new(None));
        let depth_for_closure = Arc::clone(&received_depth);
        let runner = AgentLoopRunner::new(Arc::new(move |spawner| {
            *depth_for_closure.lock().unwrap() = Some(spawner.depth());
            Ok(build_minimal_loop("完成"))
        }));
        let spawner = SubagentSpawner::new(3, 5, Arc::new(runner));
        let result = spawner
            .spawn("任务", IsolationMode::SameProcess)
            .await;
        assert!(result.is_ok());
        assert_eq!(*received_depth.lock().unwrap(), Some(1), "工厂收到的应是深度 1 的子层 spawner");
    }

    #[tokio::test]
    async fn test_agent_loop_runner_factory_error_becomes_failed_result() {
        let runner = AgentLoopRunner::new(Arc::new(|_spawner| {
            Err(HarnessError::Config("工厂构建失败".to_string()))
        }));
        let spawner = SubagentSpawner::new(3, 5, Arc::new(runner));
        let result = spawner
            .spawn("任务", IsolationMode::SameProcess)
            .await;
        let subagent_result = result.expect("spawn 不应传播错误");
        assert!(!subagent_result.success, "工厂错误应降级为失败结果");
        assert!(
            subagent_result.summary.contains("工厂构建失败"),
            "摘要应包含工厂错误信息：{}",
            subagent_result.summary
        );
    }
}