//! Subagent 委派工具：父 agent 将任务委派给独立的子 agent 循环执行。

use crate::error::{HarnessError, Result};
use crate::subagent::{IsolationMode, SubagentSpawner};
use crate::tools::context::ToolContext;
use crate::tools::Tool;
use crate::types::ToolResult;
use serde_json::{json, Value};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// 将任务委派给子 agent 执行（SameProcess 隔离）。
///
/// 子 agent 是独立的 AgentLoop 实例，在子线程内运行；父 agent 阻塞等待
/// 结果或超时。超时取自可选 `timeout_secs` 参数，默认用上下文的
/// `command_timeout`。
///
/// 同步 execute 中运行异步子 agent 采用 BashTool 先例：子线程 +
/// current_thread runtime + `block_on`；父线程 `recv_timeout` 等待。
pub struct SubagentTool {
    spawner: Arc<SubagentSpawner>,
}

impl SubagentTool {
    pub fn new(spawner: Arc<SubagentSpawner>) -> Self {
        Self { spawner }
    }
}

impl Tool for SubagentTool {
    fn name(&self) -> &str {
        "subagent"
    }

    fn description(&self) -> &str {
        "委派任务给独立的子 agent 执行；子 agent 拥有独立的对话上下文与工具集，\
         完成后返回结果摘要。适用于需要独立上下文的长任务。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "要委派给子 agent 的任务描述。"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "最大等待时间（秒），覆盖上下文默认值。",
                    "minimum": 1
                }
            },
            "required": ["task"]
        })
    }

    fn execute(&self, params: &Value, ctx: &ToolContext) -> Result<ToolResult> {
        let task = params["task"]
            .as_str()
            .ok_or_else(|| HarnessError::ToolExecution("Missing 'task' parameter".to_string()))?;

        let timeout = params["timeout_secs"]
            .as_u64()
            .map(Duration::from_secs)
            .unwrap_or(ctx.command_timeout);

        let spawner = Arc::clone(&self.spawner);
        let child_depth = spawner.depth() + 1;
        let task = task.to_string();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send((
                        Err(HarnessError::ToolExecution(format!(
                            "构建子 agent 运行时失败: {}",
                            e
                        ))),
                        Duration::ZERO,
                    ));
                    return;
                }
            };
            let start = Instant::now();
            let result = rt.block_on(spawner.spawn(&task, IsolationMode::SameProcess));
            let _ = tx.send((result, start.elapsed()));
        });

        match rx.recv_timeout(timeout) {
            Ok((Ok(subagent_result), elapsed)) => {
                let duration = elapsed.as_secs_f64();
                let content = if subagent_result.success {
                    format!(
                        "子 agent 完成（耗时 {:.1} 秒）：{}",
                        duration, subagent_result.summary
                    )
                } else {
                    format!(
                        "子 agent 失败（耗时 {:.1} 秒）：{}",
                        duration, subagent_result.summary
                    )
                };
                Ok(ToolResult {
                    success: subagent_result.success,
                    content,
                    structured: Some(json!({
                        "success": subagent_result.success,
                        "summary": subagent_result.summary,
                        "duration_secs": (duration * 10.0).round() / 10.0,
                        "depth": child_depth,
                        "timed_out": false,
                    })),
                    artifacts: vec![],
                })
            }
            Ok((Err(e), elapsed)) => Err(HarnessError::ToolExecution(format!(
                "子 agent 启动失败（耗时 {:.1} 秒）：{}",
                elapsed.as_secs_f64(),
                e
            ))),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(HarnessError::ToolExecution(format!(
                "子 agent 执行超时（{} 秒）；子任务将在后台继续运行直至完成（结果被丢弃）",
                timeout.as_secs()
            ))),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(HarnessError::ToolExecution(
                "子 agent 线程意外终止".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subagent::AgentRunner;
    use async_trait::async_trait;
    use std::time::Duration;

    fn test_context() -> ToolContext {
        ToolContext {
            workspace_root: std::path::PathBuf::from("/tmp/test-workspace"),
            command_timeout: Duration::from_secs(5),
            network_allowed: false,
        }
    }

    /// 固定返回的 mock runner。
    struct MockRunner {
        result: String,
    }

    #[async_trait]
    impl AgentRunner for MockRunner {
        async fn run(&self, _task: &str, _spawner: Arc<SubagentSpawner>) -> Result<String> {
            Ok(self.result.clone())
        }
    }

    /// 慢速 mock runner（1.5 秒），用于超时测试。
    struct SlowRunner;

    #[async_trait]
    impl AgentRunner for SlowRunner {
        async fn run(&self, _task: &str, _spawner: Arc<SubagentSpawner>) -> Result<String> {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            Ok("slow done".to_string())
        }
    }

    #[test]
    fn test_subagent_tool_success() {
        let spawner = Arc::new(SubagentSpawner::new(
            3,
            5,
            Arc::new(MockRunner {
                result: "任务完成：结果为 4".to_string(),
            }),
        ));
        let tool = SubagentTool::new(spawner);
        let result = tool
            .execute(&json!({"task": "计算 2+2"}), &test_context())
            .unwrap();
        assert!(result.success, "子 agent 成功时工具结果应为 success");
        assert!(
            result.content.contains("任务完成：结果为 4"),
            "内容应包含子 agent 摘要：{}",
            result.content
        );
        assert!(result.content.contains("子 agent 完成"));
        assert!(
            result.content.contains("耗时"),
            "内容应包含耗时：{}",
            result.content
        );
        let structured = result.structured.as_ref().unwrap();
        assert_eq!(structured["success"], true);
        assert_eq!(structured["depth"], 1, "根 spawner 委派的子 agent 深度为 1");
        assert_eq!(structured["timed_out"], false);
    }

    #[test]
    fn test_subagent_tool_missing_task_param() {
        let spawner = Arc::new(SubagentSpawner::new(
            3,
            5,
            Arc::new(MockRunner {
                result: "unused".to_string(),
            }),
        ));
        let tool = SubagentTool::new(spawner);
        let err = tool.execute(&json!({}), &test_context()).unwrap_err();
        assert!(
            err.to_string().contains("task"),
            "错误信息应指出缺少 task 参数：{}",
            err
        );
    }

    #[test]
    fn test_subagent_tool_depth_rejected() {
        // max_depth = 1：根（深度 0）可委派一次；深度 1 的 spawner 再委派即被拒
        let root = SubagentSpawner::new(
            1,
            5,
            Arc::new(MockRunner {
                result: "unused".to_string(),
            }),
        );
        let child = root.for_child();
        let tool = SubagentTool::new(child);
        let err = tool
            .execute(&json!({"task": "递归任务"}), &test_context())
            .unwrap_err();
        assert!(
            err.to_string().contains("Recursion depth exceeded"),
            "深度超限应返回 RecursionDepthExceeded：{}",
            err
        );
    }

    #[test]
    fn test_subagent_tool_timeout() {
        let spawner = Arc::new(SubagentSpawner::new(3, 5, Arc::new(SlowRunner)));
        let tool = SubagentTool::new(spawner);
        let err = tool
            .execute(
                &json!({"task": "慢任务", "timeout_secs": 1}),
                &test_context(),
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("超时"),
            "超时错误应说明超时：{}",
            err
        );
    }

    #[test]
    fn test_subagent_tool_failed_subagent_reports_failure() {
        struct FailingRunner;

        #[async_trait]
        impl AgentRunner for FailingRunner {
            async fn run(
                &self,
                _task: &str,
                _spawner: Arc<SubagentSpawner>,
            ) -> Result<String> {
                Err(crate::error::HarnessError::Config("mock 失败".to_string()))
            }
        }

        let spawner = Arc::new(SubagentSpawner::new(3, 5, Arc::new(FailingRunner)));
        let tool = SubagentTool::new(spawner);
        // spawn 把 runner 错误捕获为 success=false 的 SubagentResult
        let result = tool
            .execute(&json!({"task": "会失败的任务"}), &test_context())
            .unwrap();
        assert!(!result.success);
        assert!(result.content.contains("子 agent 失败"));
    }
}
