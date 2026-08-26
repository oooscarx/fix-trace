# FixTrace UI v2 现状审计

> 审计日期：2026-08-26  
> 审计基线：`b009804 fix: accept compatible structured diagnoses`  
> 范围：现有 Rust 源码、测试、CLI、SQLite、进度/取消、LLM Agent、演示和 Git 状态

## 1. 结论摘要

FixTrace 已经具备可复用的核心调试能力：受控录制、隔离副本、可重复 Trial、依赖闭包、ddmin、逐项消融、证据约束的 Agent、历史和脱敏导出。现有 demo 能稳定地从失败重放到成功，并得到最小动作集合 `[5, 6]`。

UI v2 不能直接在这些模块旁边再加一套编排逻辑。当前 `app.rs`、`workflow.rs` 和 `recorder/repl.rs` 由 CLI 直接调用数据库、Runner、Minimizer 和 Provider；进度事件只是临时 mpsc 消息，SQLite 也没有显式 schema 版本。这会让 TUI、GUI 和 App Server 各自形成不同状态，无法可靠处理重连、补发、审批和崩溃恢复。

迁移应保留算法和安全边界，逐步抽取唯一的 App Service、统一协议、事件存储和共享 Presentation Model。旧 CLI 命令和输出约定在迁移期间保持兼容。

## 2. 基线验证

### 2.1 环境与仓库

| 项目 | 结果 |
|---|---|
| Rust | `rustc 1.93.0` |
| Cargo | `cargo 1.93.0` |
| Node.js | `v25.9.0` |
| pnpm | `11.19.0` |
| 分支 | `main` |
| 远程 | `origin` 指向项目 GitHub 仓库 |
| 工作区 | 审计开始时干净 |
| Cargo 结构 | 一个名为 `fixtrace` 的 binary package，不是 workspace |

本地 API 密钥不属于仓库配置，未写入源码、文档、数据库或 Git。后续真实模型测试只从进程环境读取凭据。

### 2.2 已执行测试

```text
cargo test --all-targets
```

真实结果：

- 17 个单元测试通过；
- `bundled_demo_replays_from_failure_to_success` 通过，耗时 41.81 秒；
- `init_history_export_and_import_work_through_cli` 通过；
- 0 个失败、0 个忽略。

U0 文档完成后还会运行 workspace 形式的 fmt、check、test 和 clippy，结果记录在里程碑提交中。

### 2.3 当前 CLI 合约

入口为：

```text
fixtrace [--config FILE] [--state-dir DIR] [-v] <COMMAND>
```

已有命令：

| 命令 | 当前行为与兼容要求 |
|---|---|
| `init` | 创建 Session、不可变 baseline 并验证初始 Oracle 失败；继续输出 `session_id=<uuid>` |
| `shell` | 进入阻塞式受控 REPL；继续支持命令、编辑、checkpoint、verify 和 done |
| `analyze` | 重放、最小化并可运行 Agent；退出码和人类可读输出保持兼容 |
| `show` | 展示活动或已完成 Session |
| `history` | list/show 历史；现有脚本行为保持不变 |
| `export` / `import` | 脱敏 JSON 往返；旧导出格式仍可导入 |
| `config` | 查看和修改非秘密配置 |
| `demo` | 运行确定性演示；最小集合继续为 `[5, 6]` |

集成测试还固定了 `init -> history list -> export -> import -> history list` 路径。CLI 迁移后必须继续由相同测试验证，不能只验证新 UI。

## 3. 模块逐项审计

### 3.1 入口与编排

| 模块 | 当前职责 | 可复用性 | UI v2 处理 |
|---|---|---|---|
| `main.rs` | clap 入口、日志、全局 `CancellationToken`、Ctrl+C | 部分 | 保留薄入口；业务改由 App Service 命令驱动 |
| `cli.rs` | CLI 参数和子命令类型 | 高 | 作为兼容适配器，不进入 App Service 核心 |
| `app.rs` | 装配配置/DB/Provider，并直接分发工作流 | 低 | 拆为 composition root 和 CLI adapter |
| `workflow.rs` | init/analyze/runner 等跨模块流程 | 中 | 逻辑迁入应用用例/Session actor，禁止 UI 直接调用 |
| `demo.rs` | 确定性 fixture 和 MockProvider 演示 | 高 | 变成 `TaskKind::Demo`，同时供 CLI/TUI/GUI 使用 |

主要问题：当前没有可由不同客户端安全共享的应用边界，也没有任务幂等、审批或操作状态机。

### 3.2 领域与算法

| 模块 | 当前能力 | 结论 |
|---|---|---|
| `domain/action.rs` | 结构化 Action、资源访问、可重放性 | 直接复用；补充 Presentation 转换，不塞入 UI 字段 |
| `domain/snapshot.rs` | 文件元数据、内容 hash、根 hash | 直接复用 |
| `domain/trial.rs` | attempt、分类、证据、artifact 引用 | 直接复用；为协议建立稳定 view |
| `domain/session.rs` | Session 状态和分析结果 | 扩展 archive/fork 关系；Session 状态与 Task 状态分离 |
| `minimize/*` | dependency closure、cache、ddmin、ablation | 算法复用；通过任务上下文发事件和检查取消 |

算法层不应依赖传输、终端或 Tauri。协议中暴露的是 view/DTO，而不是直接承诺内部领域结构永远不变。

### 3.3 沙箱、录制与重放

| 模块 | 已有保证 | 缺口与迁移动作 |
|---|---|---|
| `sandbox/local_copy.rs` | 忽略规则、复制、相对路径校验、拒绝 symlink escape、baseline 只读 | 保留为唯一文件边界；审批不能替代技术沙箱 |
| `recorder/patch.rs` | patch 捕获和文件变化记录 | 抽成无 UI 的应用操作 |
| `recorder/repl.rs` | 解析并执行 REPL、编辑器、verify/done | 解析/执行从阻塞 stdin 循环分离，CLI/TUI 只提交命令 |
| `replay/executor.rs` | 子进程组、超时/取消、stdout/stderr | 复用进程监督；增加结构化风险描述和分块 artifact |
| `replay/runner.rs` | 每个 repetition 新副本、baseline hash 验证 | 复用；Trial 启动前走审批策略和 Task supervisor |
| `replay/oracle.rs` | repetition 聚合和稳定性分类 | 直接复用 |

当前进程输出先完整收集在内存，保存时才将大字段外置。UI v2 必须限制实时块大小，并通过 `artifact/read(offset, limit)` 分页，避免把大日志放进事件。

### 3.4 LLM 与 Agent

| 模块 | 当前能力 | 缺口与迁移动作 |
|---|---|---|
| `llm/provider.rs` | 异步非流式 Provider trait | 增加兼容的流式接口/适配器；Mock 仍可确定性测试 |
| `llm/openai_compatible.rs` | Chat Completions、tool calls、Usage | 增加流式 delta、连接测试、取消和脱敏错误映射 |
| `llm/usage.rs` | token/价格/预算 | 由 App Service 集中核算并发 `UsageUpdated`/`BudgetWarning` |
| `agent/tools.rs` | 10 个受限证据工具，限制 Action ID | 工具继续只访问应用能力，不让 UI 获得执行句柄 |
| `agent/loop_runner.rs` | 有限步数工具循环、消息/调用持久化 | 变成 AgentTurn task；支持安全边界 steer、item lifecycle 和取消 |
| `agent/diagnosis.rs` | 结构化诊断及证据校验 | 直接复用并映射到 `DiagnosisView` |

当前模型响应只有最终内容，没有 token delta；也没有自然语言 `message/send`、steer 队列和实时 Timeline Item。隐藏思维链不会作为协议或数据库字段引入，只展示用户可见回答、明确计划和证据摘要。

### 3.5 历史和持久化

`history/database.rs` 当前通过 `CREATE TABLE IF NOT EXISTS` 建表，使用 WAL，每次操作打开连接。已有表：

```text
sessions, actions, trials, trial_attempts,
messages, tool_calls, api_usage, progress_events, diagnoses
```

已有优点：

- Session、Trial、Agent 记录可恢复；
- 大于阈值的 stdout/stderr 外置为 artifact；
- 导入导出会脱敏，API Key 不落库；
- Trial 保存使用事务。

关键缺口：

- 没有 `schema_migrations` 或 `PRAGMA user_version`；
- 没有权威的 `tasks`、`app_events`、`approvals` 和客户端游标；
- `progress_events` 不是可重连协议事件，缺少 stream sequence/schema version；
- 没有独立 App Server 文件锁或单写者 actor；
- 无法标记重启前未完成任务为 `Interrupted`；
- UI 偏好和秘密存储边界尚未定义。

数据库不能由 GUI/TUI 直接打开写入。所有写入将汇聚到 Store actor；读取可通过受控快照并发。

### 3.6 进度、取消和错误

`progress` 当前有一个有界 mpsc 和 CLI renderer。生产者使用 `try_send`，队列满时事件会被静默丢弃。它适合即时 CLI 提示，不适合作为权威事件流，因为最终状态也可能缺失。

现有 `CancellationToken` 已贯穿 Trial/进程，是可复用基础；但任务只有调用栈，没有 `Queued -> Running -> ...` 状态机、任务 ID 或取消完成事件。Ctrl+C 只是取消当前顶层命令。

`AppError` 是内部错误枚举，包含适合 CLI 的上下文。协议需要额外的稳定 `AppErrorView { code, message, details, retryable }`，避免泄漏本地路径、HTTP headers 或秘密。

## 4. 可直接复用、需要抽取和必须重构的边界

### 4.1 可直接复用

- Action/Snapshot/Trial/Diagnosis/Usage 领域语义；
- 本地副本、hash 和路径/symlink 安全规则；
- 进程组取消、Oracle 重复分类；
- dependency graph、ddmin、cache 和 ablation；
- 受限 Agent tools 和证据 ID 校验；
- artifact 外置、脱敏导出导入；
- 确定性 demo fixture 与 MockProvider。

### 4.2 需要抽取

- `workflow` 抽为 App Service 用例；
- `recorder/repl` 抽出无终端的命令解析/执行器；
- `history/database` 抽为 Store 接口和单写者；
- Provider 增加流式事件而不破坏非流式实现；
- 所有领域对象通过 Presenter 生成共享 view；
- CLI 渲染变成事件订阅者，不能再代表权威状态。

### 4.3 必须重构

- 单 binary crate 变为 Cargo workspace；
- 临时 `ProgressEvent` 变成统一 `EventEnvelope<AppEvent>` 适配器；
- 隐式建表变为显式、可备份、可回滚 migration；
- 分散的长任务调用变成 Session supervisor/actor；
- TUI、GUI、server 共用协议和 InProcess transport；
- 审批策略和沙箱能力明确分层。

## 5. 兼容性与破坏性风险清单

| 风险 | 可能影响 | 控制措施 |
|---|---|---|
| crate/module 路径移动 | 单元测试和内部引用 | 先增加 library facade，再逐 crate 移动；每步全量测试 |
| CLI 改走 App Service | 输出、退出码、Ctrl+C | 保留 clap 类型和 golden/integration tests |
| Session/Task 状态拆分 | 旧 JSON/DB 反序列化 | serde 默认值、显式 schema version、迁移测试 |
| 事件替换 progress | CLI 实时输出顺序 | 写 Progress-to-AppEvent bridge，最终事件可靠投递 |
| Provider streaming | Mock 与旧 provider | 默认非流式 fallback，新增流式 capability |
| 数据库单写者 | 并发时序和性能 | 短事务、读连接池、文件锁、并发集成测试 |
| export v2 | 旧课程材料和历史文件 | 导入支持 v1/v2；导出标版本且保持脱敏 |
| 审批默认值 | 旧脚本可能等待输入 | legacy CLI 显式使用等价的 `AutoRecordedSafe`；交互 UI 可配置 |
| WebSocket | 意外暴露/秘密泄漏 | loopback 默认、token file、URL 禁止 API Key、非 loopback 显式开关 |
| 高频 delta | DB/前端膨胀 | 内存合并、持久化节流、最终 ItemCompleted 全量落库 |

## 6. 外部设计依据

本次设计参考了以下当前官方资料：

- OpenAI Codex App Server 的 initialize、thread resume/fork/read、结构化 item/event 和审批模型：<https://developers.openai.com/codex/app-server>
- OpenAI Codex sandbox 与 approval policy 的职责分离：<https://developers.openai.com/codex/sandboxing>
- Ratatui 的 Elm Architecture 和 component template：<https://ratatui.rs/concepts/application-patterns/the-elm-architecture/>、<https://ratatui.rs/templates/component/tui-rs/>
- Tauri 2 Rust command 与 streaming channel：<https://v2.tauri.app/develop/calling-rust/>
- 清华课程 Agent 架构与项目要求：<https://lab.cs.tsinghua.edu.cn/rust/projects/agent/agent-architecture/>、<https://lab.cs.tsinghua.edu.cn/rust/projects/agent/requirements/>

FixTrace 不复制 Codex 私有协议；只采用初始化握手、结构化生命周期、显式审批和可恢复订阅这些适合本项目的设计原则。

## 7. U0 退出条件

- [x] 审计 Git、工具链、项目结构和现有 CLI；
- [x] 运行现有全部测试并记录真实结果；
- [x] 审计领域、沙箱、重放、最小化、Agent、历史、进度和 demo；
- [x] 列出可复用、需抽取和需重构模块；
- [x] 识别 CLI、协议、数据库和安全兼容风险；
- [x] 在配套实施计划中给出 workspace、协议 schema、migration、TUI/GUI 和测试方案；
- [x] U0 文档通过全部质量检查；提交和推送由本里程碑收尾步骤完成。
