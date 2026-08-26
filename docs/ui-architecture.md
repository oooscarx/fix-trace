# UI 架构索引

UI v2 的权威架构说明位于 [系统架构](architecture.md)，协议与生成边界位于 [UI 协议](protocol.md)。

```text
CLI ─┐
TUI ─┼─ AppClient / typed request ─ FixTraceAppService ─ Core + SQLite/Event Store
GUI ─┘                         └── catch-up/live events ─┘
```

三个客户端不直接调用 replay/minimize 数据库内部接口。Rust `fixtrace-protocol` 是唯一 schema 源；TypeScript 文件由 `ts-rs` 生成。InProcess、stdio JSONL、WebSocket 和 Tauri command 只是 transport，Task/Approval/sequence 的权威状态始终在同一 App Service。

实现入口：`src/app_service.rs`、`crates/fixtrace-protocol`、`crates/fixtrace-client`、`apps/fixtrace-server`、`apps/fixtrace-tui`、`apps/fixtrace-desktop/src-tauri`。
