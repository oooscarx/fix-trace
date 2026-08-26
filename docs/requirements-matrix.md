# 课程要求与 UI v2 验收映射

本表把课程 R1-R6、UI v2 Definition of Done 和可重复验证证据对应起来。界面只消费 App Service 的类型化协议；表中“真实”表示使用 Rust App Service/SQLite/Replay，而非浏览器 Mock。

| 要求 | 当前实现 | 可验证证据 |
|---|---|---|
| R1：Rust 核心业务逻辑 | Snapshot、隔离复制、Action 重放、Oracle、Trial、依赖图、ddmin、Agent Loop、Provider、SQLite、App Service 和事件存储均由 Rust 实现 | `src/domain`、`src/replay`、`src/minimize`、`src/agent`、`src/app_service.rs`、`crates/fixtrace-store`；`cargo test --workspace --all-targets` |
| R2：TUI 和桌面 GUI | Ratatui TUI、Tauri 2 + React GUI 与旧 CLI 共用 `FixTraceAppService`；TUI/原生 GUI 均可操作真实 Session，Mock GUI 始终标记 `MOCK DATA` | `apps/fixtrace-tui`、`apps/fixtrace-desktop`、`crates/fixtrace-client`；`tests/app_service.rs`、`crates/fixtrace-client/tests/tui_vertical.rs`、原生截图 |
| R3：两端 Settings | TUI `/model`、`/effort`、`/permissions`、`/config` 和 GUI Settings 都发 `config/get` / `config/update`；API Key 只保存环境变量名 | `apps/fixtrace-tui/src/commands.rs`、`apps/fixtrace-desktop/src/components/Inspector.tsx`、`src/config.rs`；TUI/GUI 单测 |
| R4：事件流、实时进度和 Cancel | 持久化 `EventEnvelope`、catch-up/live subscription、Agent delta、Trial attempt、Usage 与 Task 事件；取消沿 `CancellationToken` 到子进程组 | `crates/fixtrace-protocol`、`crates/fixtrace-store`、`src/app_service.rs`、`src/replay/executor.rs`；server reconnect、TUI vertical、GUI reducer/E2E 测试 |
| R5：Sidebar、Resume、历史和导入导出 | 两端可列出/搜索/恢复 Session；事件断线按 sequence catch-up，gap 后取 snapshot；支持 fork/archive 及脱敏 JSON import/export | `session/list`、`session/get_snapshot`、`event/subscribe`、`session/import`、`session/export`；`tests/session_cli.rs`、`apps/fixtrace-server/tests/websocket_reconnect.rs` |
| R6：Header、Usage、预算和自动中断 | Header 展示模型、推理模式、审批策略、Token/费用与 Task；Usage 面板展示精确聚合；预算预警后在下一工具/模型步骤前停止 | `src/llm/usage.rs`、`src/agent/loop_runner.rs`、`src/app_service.rs`、TUI Header、GUI `UsageInspector`；预算/Usage 单测 |

## UI v2 Definition of Done

| 分组 | 验收点 | 证据 |
|---|---|---|
| 架构 | CLI/TUI/GUI 共用一个 App Service；Rust 协议生成 TypeScript；数据库/任务单一权威写者 | `docs/architecture.md`、`docs/protocol.md`、`crates/fixtrace-protocol/examples/export_types.rs`、writer-lock 集成测试 |
| TUI | Streaming、Tool、Trial、Approval、Cancel、History/Resume、Settings、Usage、Graph、Diff、Demo、终端恢复 | `docs/tui.md`、`apps/fixtrace-tui/tests/render_snapshots.rs`、`crates/fixtrace-client/tests/tui_vertical.rs`、`docs/screenshots/tui-main.png`、`tui-approval.png` |
| GUI | 真实工作流、流式事件、Cancel/Approval、历史/配置/费用/Graph/Diff、可运行 Tauri 包 | `docs/desktop.md`、Vitest/Playwright、`docs/screenshots/desktop-native-real.png`、`docs/u9-release-verification.md` |
| 安全 | UI 不直接执行命令；secret 不持久化；WS loopback + token；结构化 Approval scope；Trial 基线副本；路径/symlink 边界 | `docs/security.md`、`docs/u8-hardening.md`、协议/App Service/store/server 安全测试 |
| 测试 | 旧测试、workspace、Clippy、TUI snapshot、GUI unit/E2E、protocol、跨客户端和 Demo `[5, 6]` | `docs/u9-release-verification.md` 与最终 DoD 审计 |
| 文档 | 架构/命令与实现一致；真实与 Mock 截图明确标注；不估造 Token、费用或开发开销 | README、`docs/`、截图顶部标识、`docs/development-cost-template.csv` |

## 核心算法与安全测试点

| 测试点 | 位置 |
|---|---|
| Snapshot 稳定 hash、创建/删除/内容/权限 | `domain::snapshot::tests` |
| 路径逃逸与 symlink | `sandbox::local_copy::tests`、App Service 安全测试 |
| 环境设置/撤销 | `replay::executor::tests` |
| 硬依赖闭包、人工 ddmin、cache key | `minimize` 模块测试 |
| Flaky 不视为 Pass、取消长 Trial | `replay::runner::tests` |
| Token 费用和预算边界 | `llm::usage::tests`、`agent::loop_runner::tests` |
| JSON 往返与脱敏 | `history::export::tests`、`tests/session_cli.rs` |
| Mock tool Agent Loop、消息/工具/Usage | `agent::loop_runner::tests` |
| Demo 全轨迹、恰好 `{5,6}`、两项消融 | `tests/demo_replay.rs` |
| Protocol event/snapshot/未知事件兼容 | `fixtrace-protocol` 单测与 snapshot |
| 多客户端、重连、gap/catch-up、审批 CAS | `apps/fixtrace-server/tests`、`tests/app_service.rs`、`crates/fixtrace-store/tests` |
| 10,000 timeline、大 Artifact range | `tests/app_service.rs`、TUI snapshot test、GUI Playwright |

## 诚实边界

- GUI 的确定性 E2E 使用显式 Mock transport；打包后的原生 GUI 另以真实离线 Session 做人工验收，两者截图不混用。
- OS Keychain 未在当前构建启用；界面明确显示环境变量引用回退，secret 值不进入 React/TOML/SQLite/event/export。
- 用户亲自录入的普通 Shell 不是完整 OS 沙箱；模型只能调用证据工具，不能生成任意 Shell。
- 当前 macOS bundle 是 ad-hoc 签名，未做 Developer ID 签名和 notarization，不冒充可公开分发制品。
