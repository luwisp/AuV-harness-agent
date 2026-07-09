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
    async fn run(&self, task: &str) -> Result<String>;
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
    active_count: Arc<AtomicUsize>,
    runner: Box<dyn AgentRunner>,
}

impl SubagentSpawner {
    /// Create a new [`SubagentSpawner`].
    ///
    /// `max_depth` is the maximum nesting depth allowed (0-based).
    /// `max_total_agents` is the maximum number of concurrently active subagents.
    /// `runner` is the agent execution strategy (real agent loop, or mock for tests).
    pub fn new(
        max_depth: usize,
        max_total_agents: usize,
        runner: Box<dyn AgentRunner>,
    ) -> Self {
        Self {
            max_depth,
            max_total_agents,
            active_count: Arc::new(AtomicUsize::new(0)),
            runner,
        }
    }

    /// Spawn a subagent to handle the given task.
    ///
    /// # Errors
    ///
    /// - `RecursionDepthExceeded` if `depth >= max_depth`.
    /// - `SubagentLimitReached` if the active agent count is already at
    ///   `max_total_agents`.
    ///
    /// # Returns
    ///
    /// On success the agent's output is captured in a [`SubagentResult`].
    /// If the agent itself fails, the error is captured as a failed
    /// `SubagentResult` (success = false) rather than propagated, so the
    /// caller always receives a result struct.
    pub async fn spawn(
        &self,
        task: &str,
        depth: usize,
        _isolation: IsolationMode,
    ) -> Result<SubagentResult> {
        // Check depth limit
        if depth >= self.max_depth {
            return Err(HarnessError::RecursionDepthExceeded);
        }

        // Check total agent concurrency limit
        if self.active_count.load(Ordering::SeqCst) >= self.max_total_agents {
            return Err(HarnessError::SubagentLimitReached);
        }

        // Increment active count before running the agent
        self.active_count.fetch_add(1, Ordering::SeqCst);

        // Run the agent in (potentially) isolated context
        let result = self.runner.run(task).await;

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
        async fn run(&self, _task: &str) -> Result<String> {
            Ok(self.result.clone())
        }
    }

    /// A mock runner that sleeps briefly, used to test concurrency limits.
    struct SlowRunner;

    #[async_trait]
    impl AgentRunner for SlowRunner {
        async fn run(&self, _task: &str) -> Result<String> {
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
            Box::new(MockRunner {
                result: "Task completed successfully.".to_string(),
            }),
        );

        let result = spawner
            .spawn("Do something", 0, IsolationMode::SameProcess)
            .await;

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
            Box::new(MockRunner {
                result: "ok".to_string(),
            }),
        );

        // Depths 0, 1, 2 should all succeed
        for depth in 0..3 {
            let result = spawner
                .spawn("task", depth, IsolationMode::SameProcess)
                .await;
            assert!(
                result.is_ok(),
                "depth {} should be allowed, got {:?}",
                depth,
                result
            );
        }

        // Depth 3 should fail (>= max_depth)
        let result = spawner
            .spawn("task", 3, IsolationMode::SameProcess)
            .await;
        assert!(result.is_err(), "depth 3 should be rejected");
        match result.unwrap_err() {
            HarnessError::RecursionDepthExceeded => {}
            other => panic!("expected RecursionDepthExceeded, got {:?}", other),
        }

        // Depth 4 should also fail
        let result = spawner
            .spawn("task", 4, IsolationMode::SameProcess)
            .await;
        assert!(result.is_err(), "depth 4 should be rejected");
        match result.unwrap_err() {
            HarnessError::RecursionDepthExceeded => {}
            other => panic!("expected RecursionDepthExceeded, got {:?}", other),
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
            Box::new(SlowRunner),
        ));

        // Spawn 2 agents concurrently — both should succeed
        let s1 = spawner.clone();
        let s2 = spawner.clone();

        let handle1 = tokio::spawn(async move {
            s1.spawn("task1", 0, IsolationMode::SameProcess).await
        });
        let handle2 = tokio::spawn(async move {
            s2.spawn("task2", 0, IsolationMode::SameProcess).await
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
            async fn run(&self, _task: &str) -> Result<String> {
                // Block at the barrier, keeping the agent "active"
                self.barrier.wait().await;
                Ok("done".to_string())
            }
        }

        let spawner2 = Arc::new(SubagentSpawner::new(
            10,
            2,
            Box::new(BarrierRunner {
                barrier: barrier.clone(),
            }),
        ));

        let s3 = spawner2.clone();
        let s4 = spawner2.clone();

        // Spawn 2 agents — they will block at the barrier
        let h3 = tokio::spawn(async move {
            s3.spawn("task3", 0, IsolationMode::SameProcess).await
        });
        let h4 = tokio::spawn(async move {
            s4.spawn("task4", 0, IsolationMode::SameProcess).await
        });

        // Give them time to increment active_count and reach the barrier
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // active_count should now be 2
        assert_eq!(spawner2.active_count(), 2);

        // Try to spawn a 3rd — should be rejected
        let result3 = spawner2
            .spawn("task5", 0, IsolationMode::SameProcess)
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
}