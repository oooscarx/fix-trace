# 课程要求映射

| 要求 | 实现 | 可验证证据 |
|---|---|---|
| R1：Rust 核心业务逻辑 | Snapshot、隔离复制、Action 重放、Oracle、Trial、依赖图、ddmin、Agent Loop、Provider 编排和 SQLite 全部由 Rust 实现 | `src/domain`、`src/replay`、`src/minimize`、`src/agent`；`cargo test` |
| R2：交互式 CLI | clap 子命令与 controlled REPL；支持普通命令、cd、export/unset、edit/checkpoint、verify/status/done/quit | `src/cli.rs`、`src/recorder/repl.rs`；`tests/session_cli.rs` |
| R3：模型配置 | TOML/CLI 可配置 Provider、Endpoint、API Key 环境变量名、模型、API style、上下文、推理模式和价格；拒绝保存 `model.api_key` | `src/config.rs`；`fixtrace config show/set` |
| R4：实时进度与打断 | Tokio mpsc 结构化事件；Trial repetition、Action、Oracle、Agent step、Usage 实时输出；Ctrl+C 触发 CancellationToken；Unix 终止进程组 | `src/progress`、`src/replay/executor.rs`；`cancellation_stops_a_long_running_trial` |
| R5：历史管理 | SQLite 保存 sessions/actions/trials/attempts/messages/tool_calls/api_usage/events/diagnoses；show/list；脱敏 JSON 导入导出 | `src/history`；JSON roundtrip 单测和 `tests/session_cli.rs` |
| R6：Token 与费用 | 解析 prompt/completion Token、Request ID 和 model；Unknown Usage；按百万 Token 价格计费；Token/费用预算停止 | `src/llm/usage.rs`、`src/agent/loop_runner.rs`；费用公式与 Mock Agent 测试 |

## 场景定制要求

1. Rust/Cargo 调试轨迹的重放 Oracle、五类 Action 和 Snapshot 权限/符号链接语义。
2. dependency-aware ddmin、稳定性分类、逐项消融和最终绕过 cache 的复验。
3. 证据约束工具与结构化 Diagnosis，模型引用的 Action/Trial ID 由 Rust 验证存在。

## 测试要求对应

| 测试点 | 位置 |
|---|---|
| Snapshot 稳定 hash、创建/删除/内容/权限 | `domain::snapshot::tests` |
| 路径不能逃逸 | `sandbox::local_copy::tests` |
| 环境设置/撤销 | `replay::executor::tests` |
| 硬依赖闭包 | `minimize::dependency::tests` |
| 人工 ddmin | `minimize::ddmin::tests` |
| Flaky 不视为 Pass | `replay::runner::tests` |
| cache key | `minimize::cache::tests` |
| Token 费用 | `llm::usage::tests` |
| JSON 往返与脱敏 | `history::export::tests` |
| Ctrl+C/取消长 Trial | `replay::runner::tests::cancellation_stops_a_long_running_trial` |
| Mock tool Agent Loop、消息/工具/Usage 历史 | `agent::loop_runner::tests` |
| Demo 全轨迹、恰好 `{5,6}`、两项消融 | `tests/demo_replay.rs` |
| init/history/export/import CLI | `tests/session_cli.rs` |

