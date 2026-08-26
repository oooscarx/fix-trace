# FixTrace Desktop

FixTrace Desktop 是 Tauri 2、React 和 TypeScript 客户端。原生窗口内的 Rust Shell 只创建 `InProcessClient`，所有会话、任务、Trial、诊断、用量和持久化状态仍由 `FixTraceAppService` 维护。React 不直接访问 SQLite、执行命令或调用分析模块。

## 开发启动

安装前端依赖并启动真实桌面窗口：

```bash
cd apps/fixtrace-desktop
npm ci
npm run tauri -- dev
```

Tauri 启动时会获取单写者锁并发现与 CLI/TUI 相同的 FixTrace 状态目录。另一个 App Server 或桌面进程已经持有写锁时，启动会明确失败，不会产生第二个数据库写者。

浏览器中的确定性 UI 开发模式：

```bash
npm run dev:mock
```

打开 `http://127.0.0.1:1420/`。Mock 必须由 `VITE_FIXTRACE_MOCK=1` 显式启用；生产构建不会在原生调用失败后静默回退。页面顶部持续显示 `MOCK DATA`，固定事件流只用于组件测试、E2E 和截图，不会启动真实分析。

## 垂直工作流

当前桌面客户端通过类型化 Tauri commands 和 `Channel<EventEnvelope>` 支持：

1. 初始化协议并加载 Session 列表；
2. 从 Rust snapshot 恢复选中的 Session；
3. 启动 Verify、Replay 或 Analyze Task；
4. 增量合并 User、Agent、Tool Call 和 Trial timeline 事件；
5. 在运行中发送 Steer 或 Cancel；
6. 从完成、取消、失败和 Gap 事件恢复权威 snapshot；
7. 查看 Actions、Trials、依赖图、Diff、Usage 和连接配置摘要。

事件 reducer 以 `(stream_id, sequence)` 水位去重；服务端报告 Gap 时，原生客户端重新加载 snapshot。取消任务时，仍处于运行态的 UI item 会标记为 cancelled，避免残留伪运行状态。

## Rust / TypeScript 边界

Rust 权威协议位于 `crates/fixtrace-protocol`。以下命令更新生成文件：

```bash
cargo run -p fixtrace-protocol --example export_types -- \
  apps/fixtrace-desktop/src/generated/protocol
```

前端只从生成的协议类型构造 `AppRequest`、处理 `AppResponsePayload` 和消费 `EventEnvelope`。Tauri 暴露四个窄接口：

- `initialize_client`
- `execute_request`
- `subscribe_events`
- `unsubscribe_events`

订阅使用独立 ID 和 cancellation token；React 卸载或切换 Session 时显式退订。

## 安全边界

- Tauri capability 只启用 `core:default`，不授予任意 Shell、文件系统或网络权限。
- CSP 只允许应用自身资源、Tauri IPC 和本地 asset scheme。
- Markdown 经过 `rehype-sanitize`，不会执行消息中的 HTML 或脚本。
- API Key 值不进入前端状态；初始化只返回 `has_api_key` 和环境变量来源摘要。
- Mock endpoint 是不可路由的 `example.invalid`，也不会执行外部请求。
- App Service 和 WriterLock 保持所有业务写入的单一权威路径。

## 检查

```bash
cd apps/fixtrace-desktop
npm run build
npm test
npm run tauri -- build --no-bundle

cd ../..
cargo fmt --all --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

前端测试包含 reducer 的 streaming、sequence 去重、Gap 和取消语义，以及一个打开 Session、收到 Tool/Trial 流并取消 Task 的完整组件流程。完整 GUI 功能与 Playwright E2E 在 U7 扩展。
