# FixTrace UI v2 实施与迁移计划

> 本文是 U0 的可执行设计。实现以小步提交推进，每个里程碑必须先测试、更新文档、提交并推送，再进入下一阶段。

## 1. 目标与不变量

最终由同一个 Rust App Service 同时服务 CLI、TUI、桌面 GUI 和本地 App Server。Rust core 和 SQLite 是唯一权威状态，客户端只发送命令、订阅事件并渲染共享 Presentation Model。

迁移全过程保持以下不变量：

1. 每个 Trial 仍在新建 baseline 副本中运行，并校验 root hash；
2. 路径、symlink、网络和进程边界不能被 UI 或审批绕过；
3. 只有 `StablePass` 能推动最小化，最终集合仍经 ablation 和 bypass-cache 验证；
4. API Key 不进入数据库、日志、事件、导出或 Git；
5. 旧 CLI 入口、退出码和 demo `[5, 6]` 保持回归覆盖；
6. 最终任务状态和完成 Item 不能因背压而丢失；
7. TUI 与 GUI 不自行解释 Trial、费用、置信度或审批状态。

## 2. 目标 Workspace

采用按依赖方向划分的 workspace，避免以 UI 技术栈切分业务：

```text
Cargo.toml                         # workspace + shared dependencies
crates/
  fixtrace-core/                   # domain, sandbox, replay, minimize, agent, LLM ports
  fixtrace-store/                  # SQLite migrations, repositories, artifacts, event store
  fixtrace-protocol/               # request/response/event/view DTO，TS 生成源
  fixtrace-presenter/              # domain -> shared SessionView/TimelineItemView
  fixtrace-app/                    # FixTraceApplication, actors, tasks, approval, budgets
  fixtrace-client/                 # InProcess/stdin/WebSocket client transports
apps/
  fixtrace-cli/                    # 兼容现有 fixtrace 命令
  fixtrace-server/                 # stdio JSONL 与本地 WebSocket
  fixtrace-tui/                    # Ratatui 客户端
  fixtrace-desktop/                # Tauri 2 + React/TypeScript/Vite
demo/                              # 共享确定性演示项目和 trace
docs/
```

依赖只能沿以下方向：

```text
core <- store
core + protocol + store + presenter <- app
protocol <- presenter
protocol + app <- client
client + protocol + presenter <- CLI / TUI / Server / Tauri shell
```

更精确地说，`core` 不依赖数据库、协议或 UI；`protocol` 不依赖 App Service；`store` 实现 `app` 所消费的持久化 port 时，通过一个小的 ports crate 或在 `core` 中定义接口来避免环。若实施时出现依赖环，优先下沉 trait，而不是引入全局状态。

### 2.1 分阶段移动策略

1. U1 先添加 `src/lib.rs`/应用 facade，让现有 binary 变薄，保留原路径和测试；
2. 抽出 `fixtrace-app` 接口、任务监督和 CLI adapter；
3. U2 建立 protocol/store/presenter/client crates，并通过 adapter 使用旧核心；
4. 每次只移动一组模块，使用 Git rename 保留历史；
5. U3 完成后再将根 package 收敛成真正 workspace apps，保持二进制名 `fixtrace`；
6. 所有阶段 `cargo test --workspace --all-targets` 必须通过。

这条桥接路线避免一次性重写，也允许 U1/U2 各自形成可审查提交。

## 3. App Service 设计

唯一公共应用入口：

```rust
#[async_trait]
pub trait FixTraceApplication: Send + Sync {
    async fn initialize(
        &self,
        request: InitializeRequest,
    ) -> Result<InitializeResponse, AppError>;

    async fn execute(&self, request: RequestEnvelope)
        -> Result<ResponseEnvelope, AppError>;

    async fn subscribe(
        &self,
        request: SubscribeRequest,
    ) -> Result<EventSubscription, AppError>;
}
```

`FixTraceAppService` 内部结构：

```text
bounded command mpsc
  -> AppService actor（路由、幂等、全局限制）
     -> SessionSupervisor[session_id]
        -> 当前 mutation task + CancellationToken + steer queue
        -> JoinSet 管理 Trial/Agent 子任务
     -> StoreWriter actor（SQLite 唯一写者）
     -> EventHub（持久化后 broadcast）
```

规则：

- 一个 Session 同时最多一个改变分析状态的 task；
- 不同 Session 可并行，读取和 artifact paging 可并发；
- 锁内只读写内存状态，不在锁内等待文件、DB、子进程或网络；
- `(client_id, operation_id)` 建唯一索引，重复命令返回原 response/task；
- 取消先转 `Cancelling`，触发 token、终止进程、等待清理，再持久化 `Cancelled`；
- steer 只进入下一个 Agent safe boundary，不能修改既有 Trial 事实；
- 客户端断开不自动取消任务，交互客户端关闭时自行提示。

## 4. 具体协议 Schema

协议常量第一版使用 `fixtrace/1`。破坏性变更提升 major；增加 optional 字段或事件提升 schema minor，并保持未知事件可跳过。

### 4.1 Frame 与初始化

```rust
pub const PROTOCOL_VERSION: &str = "fixtrace/1";

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientFrame {
    Request(RequestEnvelope),
}

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerFrame {
    Response(ResponseEnvelope),
    Event(EventEnvelope),
}

pub struct RequestEnvelope {
    pub id: Uuid,
    pub operation_id: Uuid,
    pub method: String,
    pub params: serde_json::Value,
}

pub struct ResponseEnvelope {
    pub id: Uuid,
    pub result: Option<serde_json::Value>,
    pub error: Option<AppErrorView>,
}
```

连接后的第一个请求必须是 `initialize`：

```rust
pub struct InitializeRequest {
    pub protocol_version: String,
    pub client: ClientInfo,
    pub capabilities: ClientCapabilities,
}

pub struct InitializeResponse {
    pub protocol_version: String,
    pub server_version: String,
    pub capabilities: ServerCapabilities,
    pub config_summary: PublicConfigSummary,
    pub client_id: Uuid,
}
```

不兼容 major 返回 `incompatible_protocol` 后关闭连接；重复 initialize 返回 `already_initialized`。stdio 每行一个 frame，最大普通 frame 8 MiB；更大内容必须走 artifact paging。

### 4.2 类型化请求

内部使用类型化 enum，transport 层将 method/params 解码到它：

```rust
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum AppCommand {
    SessionList(SessionListRequest),
    SessionCreate(SessionCreateRequest),
    SessionOpen(SessionIdRequest),
    SessionFork(SessionForkRequest),
    SessionArchive(SessionIdRequest),
    SessionDelete(SessionDeleteRequest),
    SessionGetSnapshot(SessionSnapshotRequest),

    TaskStart(TaskStartRequest),
    TaskSteer(TaskSteerRequest),
    TaskCancel(TaskIdRequest),
    TaskGet(TaskIdRequest),
    MessageSend(MessageSendRequest),

    ActionList(SessionPageRequest),
    ActionGet(ActionGetRequest),
    TrialList(SessionPageRequest),
    TrialGet(TrialGetRequest),
    TrialRun(TrialRunRequest),
    TrialRepeat(TrialRepeatRequest),
    DependencyGetGraph(SessionIdRequest),
    DiagnosisGet(SessionIdRequest),

    ArtifactList(SessionPageRequest),
    ArtifactRead(ArtifactReadRequest),
    ApprovalRespond(ApprovalRespondRequest),
    ConfigGet(EmptyRequest),
    ConfigUpdate(ConfigUpdateRequest),
    ConfigTestConnection(ConnectionTestRequest),
    UsageGet(UsageGetRequest),
    SessionExport(SessionExportRequest),
    SessionImport(SessionImportRequest),
    EventSubscribe(SubscribeRequest),
    EventUnsubscribe(UnsubscribeRequest),
}
```

所有 list 使用稳定 cursor/limit，默认 100、最大 500。`ArtifactReadRequest` 使用 `offset: u64` 和 `limit: u32`，单次最大 1 MiB，并返回 `next_offset`、`eof` 和内容 hash。

### 4.3 Task 状态机

```rust
pub enum TaskKind {
    AgentTurn,
    RecordTrace,
    VerifyBaseline,
    ReplayFullTrace,
    AnalyzeMinimalTrace,
    RepeatTrial,
    GenerateDiagnosis,
    ExportSession,
    Demo,
}

pub enum TaskStatus {
    Queued,
    Running,
    WaitingForApproval,
    Cancelling,
    Cancelled,
    Completed,
    Failed,
    Interrupted,
}
```

合法转换集中在 `TaskStateMachine::transition`：

```text
Queued -> Running | Cancelled | Failed
Running -> WaitingForApproval | Cancelling | Completed | Failed | Interrupted
WaitingForApproval -> Running | Cancelling | Failed | Interrupted
Cancelling -> Cancelled | Failed | Interrupted
terminal = Cancelled | Completed | Failed | Interrupted
```

`Interrupted` 是为满足崩溃恢复而明确增加的终态；它不伪装成失败或完成，并允许用户以新 task 从已保存证据继续。

### 4.4 Event 与 Timeline

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

同一个 `stream_id` 的 sequence 从 1 开始单调递增；服务端在数据库事务中分配 sequence、保存事件和状态变化，提交后才 broadcast。

```rust
pub enum AppEvent {
    SessionCreated(SessionSummary),
    SessionUpdated(SessionSummary),
    TaskStarted(TaskSummary),
    TaskProgress(TaskProgress),
    TaskCompleted(TaskResult),
    TaskFailed(TaskFailure),
    TaskCancelled(TaskSummary),
    ItemStarted(TimelineItem),
    ItemDelta(ItemDelta),
    ItemCompleted(TimelineItem),
    ApprovalRequested(ApprovalRequest),
    ApprovalResolved(ApprovalResolution),
    UsageUpdated(UsageSummary),
    BudgetWarning(BudgetWarning),
    DiagnosisUpdated(DiagnosisView),
    ArtifactCreated(ArtifactSummary),
    Notice(Notice),
    Error(AppErrorView),
    EventGap(EventGap),
}
```

Timeline 使用固定 `TimelineItemHeader { id, status, started_at, completed_at, parent_id, artifact_refs, entity_refs }`，payload 为：

```text
UserMessage | AgentMessage | PlanSummary | ToolCall | CommandExecution |
FilePatch | RecordedAction | Trial | Minimization | Diagnosis |
Approval | Usage | Notice | Error
```

`ItemDelta` 只允许白名单 delta（Agent text、command output chunk、progress/log batch），必须引用已开始 item。每 50 ms 或 8 KiB 合并一次；数据库不保存单字符 token，`ItemCompleted` 始终保存最终完整内容。任何隐藏 chain-of-thought 都不进入这些类型。

### 4.5 Event catch-up

```rust
pub struct SubscribeRequest {
    pub session_id: Uuid,
    pub after_sequence: Option<u64>,
}
```

订阅顺序为：在 Store 建立高水位 -> 读取 `(after, high_watermark]` -> 接入 live buffer -> 去重并连续投递。慢客户端的有界队列满时，连接收到 `EventGap { expected, available_from }`，随后调用 `session/get_snapshot` 恢复；它不能阻塞任务生产者。

### 4.6 错误与兼容性

```rust
pub struct AppErrorView {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub details: Option<serde_json::Value>,
}
```

稳定错误码至少包含：`invalid_request`、`not_initialized`、`incompatible_protocol`、`not_found`、`conflict`、`operation_in_progress`、`invalid_transition`、`approval_required`、`approval_resolved`、`cancelled`、`budget_exceeded`、`sandbox_denied`、`unauthorized`、`event_gap`、`internal`。内部错误链只写脱敏 stderr tracing，不原样返回客户端。

Rust serde 类型是唯一协议源头。采用 `ts-rs` 生成 `apps/fixtrace-desktop/src/generated/protocol.ts`，生成文件入库，CI 重新生成后要求 `git diff --exit-code`；Rust JSON snapshot 和 TypeScript compile test 防止漂移。

## 5. Approval 与安全边界

实现 `ReadOnly`、`AskAlways`、`AskForOpaque`、`AutoRecordedSafe` 四种 policy。判定输入为结构化 `ExecutionIntent`，包含 Action IDs、解析后 argv、cwd、路径集合、网络需求、replayability 和 shell opaque flags。

`ApproveEquivalentForSession` 保存的是 canonical rule，例如 action kind + executable hash + cwd scope + path scopes + network=false，不按字符串前缀匹配。Approval 解析使用 compare-and-set；第一份有效响应生效，其他客户端得到 `approval_resolved`。

安全层次：

```text
Approval policy（用户是否授权）
  -> Execution policy（结构化安全分类）
     -> Local-copy sandbox/path guard（技术边界）
        -> Process supervisor（取消、超时、进程组）
```

任何批准都不能放宽 baseline、路径/symlink 或 loopback 网络默认限制。旧 CLI 的已有工作流显式选择 `AutoRecordedSafe`，只对当前 Session 已录制且通过安全分类的 Action 自动执行，从而保持非交互脚本兼容。

## 6. 数据库迁移

### 6.1 版本与升级

引入 `schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT, checksum TEXT)`。启动时：

1. 获取 state-dir 独占 writer 文件锁；
2. `PRAGMA wal_checkpoint(TRUNCATE)`；
3. 识别现有表且无 migration 记录时登记为 schema v1；
4. 使用 SQLite backup API 创建带时间戳的 v1 备份；
5. 对每个 migration 执行 `BEGIN IMMEDIATE`、校验前置版本、DDL/DML、记录 checksum、commit；
6. 任一步失败 rollback，并保留原 DB 与备份；
7. 迁移后跑 foreign key/integrity check。

首次 UI v2 migration 为 v2：

```text
tasks
  id, session_id, operation_id, kind, status, request_json,
  result_json, error_json, created_at, started_at, finished_at, updated_at

app_event_streams
  id, session_id, next_sequence, created_at

app_events
  event_id, stream_id, sequence, session_id, task_id, timestamp,
  event_type, payload_json, schema_version

approvals
  id, session_id, task_id, status, request_json, resolution_json,
  resolved_by_client_id, created_at, resolved_at

client_sessions
  id, client_name, client_version, capabilities_json,
  connected_at, last_seen_at

ui_preferences
  client_scope, preference_key, value_json, updated_at

operations
  client_id, operation_id, request_hash, response_json, task_id, created_at
```

约束/索引：`UNIQUE(stream_id, sequence)`、`UNIQUE(client_id, operation_id)`、每个 session 只允许一个非终态 mutation task 的 partial unique index，所有外键启用。API Key 不属于 `ui_preferences`；桌面端放系统 keychain，CLI/TUI 继续使用环境变量或权限受限的本机秘密来源。

### 6.2 写入与恢复

StoreWriter 负责状态与 event 的原子提交，读请求通过只读连接并发。启动恢复把 `Queued/Running/WaitingForApproval/Cancelling` 更新为 `Interrupted`，为每个 task 追加终态事件，保留已完成 Trial 和 artifact。重复启动不会重复追加，靠 recovery operation ID 幂等。

旧 `progress_events` 保留供 v1 审计，不直接伪造成新协议事件；迁移后的新任务只写 `app_events`。v1 export 继续可导入，v2 export 增加 schema version、task/event/approval（脱敏后）并保持 artifact hash。

## 7. Presentation Model

`fixtrace-presenter` 是唯一业务派生层：

```rust
pub struct SessionView {
    pub summary: SessionSummaryView,
    pub task: Option<TaskView>,
    pub timeline: Vec<TimelineItemView>,
    pub actions: Vec<ActionView>,
    pub trials: Vec<TrialView>,
    pub diagnosis: Option<DiagnosisView>,
    pub usage: UsageView,
    pub approvals: Vec<ApprovalView>,
}
```

Rust 计算 `classification`、`confidence`、`budget_ratio`、`progress_ratio`、`is_cancellable`、`can_rerun`、`can_approve`、`trial_summary` 和 `diagnosis_summary`。客户端只选择布局、颜色、折叠和导航。

## 8. 客户端实现

### 8.1 TUI

使用 Ratatui + crossterm，采用 Elm/TEA：`Model -> update(Message) -> Effect -> AppClient -> Event -> Model`。默认 InProcess transport，也可连接 server。

- 单一 async event loop 合并键盘、resize、tick、app event；
- 最多 30 FPS；50 ms 合并高频 delta，最终事件立即刷新；
- 宽/中/窄三档布局，共用 Sidebar、Transcript、Inspector、Composer 组件；
- 只布局可见窗口附近 item，wrap cache 以 `(item_id, width, revision)` 为键；
- raw mode/alternate screen/panic hook 使用 RAII 恢复；
- Approval modal 显示命令、cwd、Action、路径、网络、风险和 scope；
- TestBackend + insta 覆盖三种宽度、任务/审批/错误/取消/恢复状态。

先在 U4 完成真实垂直切片，再在 U5 增加 Actions、Trials、Graph、Diff、Usage、Settings、History、Export 和 slash commands。

### 8.2 桌面 GUI

使用 Tauri 2 + React + TypeScript + Vite + pnpm workspace。Tauri shell 通过 InProcess AppClient 调用 App Service；长事件使用 Tauri channel，不用为每个 token 建 command。Web UI 不获得数据库路径、进程句柄或领域模块访问权。

- Timeline 使用虚拟列表，目标 10,000 item；
- Sidebar/Transcript/Inspector/Composer 与 TUI 展示同一 SessionView；
- Graph/Diff 只渲染 protocol view；
- API Key 由系统 keychain 插件保存，连接测试只返回脱敏结果；
- 文件选择、保存、通知使用 Tauri 原生能力；
- Mock backend 只用于组件开发/测试，release flow 必须走真实 App Service；
- Vitest/Testing Library 做 reducer/component test，Playwright + Tauri driver 做真实 E2E；
- U9 在 macOS 生成可运行 bundle，并记录平台/签名限制。

## 9. App Server 与 Transport

InProcess、stdio 和 WebSocket 实现同一个 `AppClient` trait。stdio stdout 只发 JSONL frame，tracing 写 stderr；坏 JSON 返回对应 request error，进程继续。收到 EOF/SIGINT 时停止接收、等待 frame flush，并按策略让后台任务完成或由 server supervisor 取消。

WebSocket 选 Axum：默认仅 `127.0.0.1:4765`，启动时创建 256-bit capability token 文件（权限 0600），认证放 Authorization header，禁止 URL query API Key。非 loopback 必须同时提供 `--allow-remote` 和 TLS 反向代理说明。每客户端有界队列，慢客户端走 EventGap；客户端使用带抖动指数退避并以 last sequence 重连。

state-dir 使用文件锁保证只有一个 server writer。TUI 和 GUI 可连接同一 server，订阅事件顺序来自持久化 stream，而不是各自的 wall clock。

## 10. 测试矩阵

| 层 | 必须覆盖 |
|---|---|
| Core | baseline/symlink/path、replay repetition、process cancel、dependency/ddmin/ablation、diagnosis evidence |
| App Service | 每种合法/非法 task 转换、单 session 排他、跨 session 并发、operation 幂等、cancel/steer、budget、approval CAS |
| Store | v1 fixture -> v2、失败回滚/备份、event sequence、writer lock、crash recovery、artifact range |
| Protocol | JSON snapshot、未知 optional 字段、错误 major、initialize-first、坏 JSON、TS generation freshness |
| Transport | InProcess 顺序、stdio stdout 纯净/graceful、WS auth/loopback/reconnect/gap |
| TUI | reducer、TestBackend snapshot、宽中窄、10k virtual window、raw mode panic recovery、approval/cancel |
| GUI unit | reducers、event merge、derived fields只读、virtual list、settings/approval/error |
| GUI E2E | create/open、streaming、Trial、cancel、resume、approval、history、export、offline demo |
| 跨客户端 | TUI 创建 GUI 恢复；GUI 启动 TUI 观察；相同 sequence/view；重复 approval 仅一次成功 |
| 安全 | secret scan/export/log/event、path escape、symlink、network、WS token、opaque command |
| 回归 | 现有所有测试、CLI help/脚本、demo 最小集合 `[5, 6]` |
| 性能 | 10,000 timeline item、慢客户端 gap、数百 MB artifact 分页、delta coalescing |

每个 Rust 里程碑运行：

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

前端加入后还运行 `pnpm lint`、`pnpm typecheck`、`pnpm test`、Playwright E2E 和 `pnpm tauri build`。只汇报真实执行结果；平台签名、系统依赖或 CI 限制必须明确列为限制。

## 11. U0-U9 执行顺序与提交

| 里程碑 | 交付与退出条件 | 计划提交主题 |
|---|---|---|
| U0 | 审计、workspace/protocol/migration/UI/test 设计；基线全绿 | `docs: audit UI v2 architecture and migration` |
| U1 | App Service facade、actor/task skeleton、CLI 全部改走 service；旧测试全绿 | `refactor: route CLI through the app service` |
| U2 | protocol/event/timeline/approval/presenter、TS generation、InProcess、event store/catch-up | 按 protocol/store 两个小提交 |
| U3 | stdio JSONL、WebSocket auth、文件锁、多客户端/重连测试 | `feat: add local FixTrace app server` |
| U4 | TUI 真实 open/message/stream/tool/trial/cancel 垂直切片 | `feat: add end-to-end FixTrace TUI slice` |
| U5 | 完整 TUI panels/commands/approval/history/settings/snapshots | 按 transcript/inspector 两个提交 |
| U6 | Tauri GUI 真实 connect/open/stream/trial/cancel/resume | `feat: add desktop GUI vertical slice` |
| U7 | 完整 GUI graph/diff/usage/settings/keychain/approval/search/E2E | 按功能和 E2E 分提交 |
| U8 | 跨客户端、gap/crash/10k/artifact/secret/security | `test: harden cross-client recovery and security` |
| U9 | 两端 demo/offline、截图、README、展示脚本、Tauri build/干净构建 | docs、demo、packaging 分提交 |

每个阶段遵循：实现 -> 定向测试 -> 全量质量检查 -> 文档/真实限制 -> secret scan -> commit -> push。禁止把多个未验证里程碑压成一个巨大提交。

## 12. 完成审计

U9 后逐条对照原始 Definition of Done：架构、TUI、GUI、安全、测试、文档六组全部有代码或测试证据才标记完成。最终报告包括：

- commit 列表和远程同步状态；
- 所有实际执行命令及结果；
- TUI/GUI 实际截图路径；
- demo `[5, 6]` 的真实输出；
- Tauri bundle 路径、平台和签名状态；
- 任何未通过项及可复现原因。

只完成静态 UI、只运行 Mock、或只构建其中一个客户端，都不满足本阶段目标。
