# FixTrace Protocol `fixtrace/1`

`fixtrace-protocol` 是 CLI、TUI、GUI、App Server 和 InProcess transport 共用的传输中立协议。Rust 类型是唯一 schema 源头；桌面端 TypeScript 由 `ts-rs` 生成，不手写 DTO。

## 连接与 frame

每条请求有两个 ID：

- `id` 关联一次 request/response；
- `operation_id` 表示幂等操作，同一客户端重试时保持不变。

```json
{
  "kind": "request",
  "data": {
    "id": "...",
    "operation_id": "...",
    "method": "session/list",
    "params": { "page": { "cursor": null, "limit": 100 }, "include_archived": false }
  }
}
```

服务端 frame 是 `response` 或 `event`。Response 必须恰有一个 `result`/`error` 分支。stdio transport 每行一个 frame；WebSocket 每个 text message 一个 frame。

连接后首个请求必须是 `initialize`，版本为 `fixtrace/1`。当前 major 不匹配会返回 `incompatible_protocol`，重复 initialize 返回 `already_initialized`。握手交换客户端/服务端 streaming、approval、diff、graph、artifact、catch-up 和多客户端能力，不兼容时不会静默降级。

## 请求方法

当前 Rust `AppRequest` 覆盖：

```text
initialize
session/list, session/create, session/open, session/fork,
session/archive, session/delete, session/get_snapshot
task/start, task/steer, task/cancel, task/get
message/send
action/list, action/get
trial/list, trial/get, trial/run, trial/repeat
dependency/get_graph, diagnosis/get
artifact/list, artifact/read
approval/respond
config/get, config/update, config/test_connection
usage/get
session/export, session/import
event/subscribe, event/unsubscribe
```

List 请求统一用 opaque cursor，默认 100、最大 500。Artifact 按 byte range 读取，单次最大 1 MiB；事件中只放 `ArtifactSummary`，不放大内容。连接测试只传 `credential_id`，不允许 API Key 成为协议字段。

## Task 状态机

Task kind：Agent turn、录制、baseline 验证、全量 replay、最小化、repeat trial、诊断、导出和 demo。

```text
Queued -> Running | Cancelled | Failed
Running -> WaitingForApproval | Cancelling | Completed | Failed | Interrupted
WaitingForApproval -> Running | Cancelling | Failed | Interrupted
Cancelling -> Cancelled | Failed | Interrupted
```

`Cancelled`、`Completed`、`Failed`、`Interrupted` 是终态。合法转换由 `TaskStatus::can_transition_to` 集中定义并单元测试；客户端不得自行推断另一套状态机。

## Event stream

```rust
pub struct EventEnvelope {
    pub schema_version: u16,
    pub stream_id: Uuid,
    pub sequence: u64,
    pub event_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub session_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub payload: AppEvent,
}
```

同一 stream 的 sequence 从 1 单调递增。JSON/TypeScript 使用 number，服务端保证不会超过 JavaScript safe integer。`event_id` 用于跨补发/live 边界去重。

`AppEvent` 覆盖 Session/Task 生命周期、Timeline Item start/delta/complete、Approval、Usage/Budget、Diagnosis、Artifact、Notice/Error 和 EventGap。`ItemDelta`/`TaskProgress` 可合并，其余事件立即持久化；`ItemCompleted` 必须含完整最终内容。

订阅传 `after_sequence`。服务端先读取持久化 catch-up，再接 live buffer。如果无法连续补发则返回 `EventGap`，客户端必须调用 `session/get_snapshot`，不能把缺失事件猜成最终状态。

InProcess transport 已实现相同语义：先注册有界 live receiver，再读取订阅时的 SQLite high watermark，最后按 sequence 合并并去重，因此 catch-up 与 live 之间没有竞态窗口。客户端未 initialize 时拒绝请求；同一 `operation_id` 的 Task 重试返回原 Task。

## Timeline 与可显示内容

Timeline item 是结构化 tagged enum，不是大段 Markdown。每项有稳定 ID、状态、起止时间、父项、artifact 和实体引用。类型包括：

```text
UserMessage, AgentMessage, PlanSummary, ToolCall, CommandExecution,
FilePatch, RecordedAction, Trial, Minimization, Diagnosis,
Approval, Usage, Notice, Error
```

Agent 增量只含用户可见文本。协议没有隐藏 chain-of-thought 字段；允许的解释内容只有明确计划、公开 reasoning summary、工具选择理由和实验依据。

## Approval

Policy 为 `read_only`、`ask_always`、`ask_for_opaque`、`auto_recorded_safe`。Request 显示操作、原因、风险、命令、cwd、路径、Action IDs、网络、Trial sandbox 和请求 scope。Choice 为单次批准、task 批准、当前 Session 结构化等价规则批准、拒绝或取消任务。

审批是授权层，不替代 local-copy sandbox、路径/symlink guard 或进程监督。多个客户端只能有一个响应通过 compare-and-set，其余收到 `approval_resolved`。

## 共享 Presentation Model

`SessionView` 同时包含 summary、active task、timeline、actions、trials、diagnosis、usage、approvals、dependency graph 和 diff。后端负责 `classification`、`confidence`、`budget_ratio`、`progress_ratio`、`is_cancellable`、`can_rerun`、`can_approve` 及人类可读 summary；客户端只决定布局、颜色和交互。

上述派生位于独立 `fixtrace-presenter` crate；协议 DTO 不依赖内部领域对象，App Service 负责从 Action/Trial/Diagnosis/Usage 转换。

## TypeScript 生成

生成命令：

```bash
cargo run -p fixtrace-protocol --example export_types -- \
  apps/fixtrace-desktop/src/generated/protocol
```

输出文件带 “Do not edit” 标记并提交到 Git。测试 `checked_in_typescript_bindings_are_current` 会在临时目录重新生成并逐字节比较；Rust schema 变化但忘记更新 TypeScript 时，测试会失败。

## 兼容与测试

- 破坏性变更提升 `fixtrace/<major>`；
- 新 optional 字段和可忽略事件保持同 major；
- 未知 method/错误参数映射到稳定 `invalid_request`；
- event JSON 有 insta snapshot；
- request method、response 单分支、Task 转换、秘密字段缺失和 TS freshness 都有单元测试；
- 大整数只在协议约定的 safe integer 范围内发送。
