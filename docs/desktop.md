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

课堂演示可从仓库根目录运行 `./demo/presentation.sh desktop`；脚本会在临时状态目录中准备并离线分析一个真实 Session，再打开 Tauri dev window。`./demo/presentation.sh mock-gui` 只用于固定视觉流程。

## 完整工作流

桌面客户端通过类型化 Tauri commands 和 `Channel<EventEnvelope>` 支持：

1. 初始化协议并加载 Session 列表；
2. 从 Rust snapshot 恢复选中的 Session；
3. 启动 Verify、Replay 或 Analyze Task；
4. 增量合并 User、Agent、Tool Call 和 Trial timeline 事件；
5. 在运行中发送 Steer 或 Cancel；
6. 从完成、取消、失败和 Gap 事件恢复权威 snapshot；
7. 处理带风险、命令预览、工作目录、影响路径和作用域的 Approval；
8. 新建、导入、Fork、导出和归档 Session；
9. 查看 Actions、Trials、资源依赖图、Diff、Artifact、Usage 和连接配置；
10. 使用 `⌘/Ctrl+K` 搜索 Session、`⌘/Ctrl+Shift+N` 新建 Session、`⌘/Ctrl+1…8` 切换 Inspector。

事件 reducer 以 `(stream_id, sequence)` 水位去重；服务端报告 Gap 时，原生客户端重新加载 snapshot。取消任务时，仍处于运行态的 UI item 会标记为 cancelled，避免残留伪运行状态。

时间线使用 `react-virtuoso`，卡片保留结构化 Item，不把 Tool、Trial、Diff 或诊断压成 Markdown。Agent Markdown 经过过滤；命令输出会中和 C0、DEL/CSI 与双向文本控制字符。Inspector 提供：

- Action 搜索、排序、类型过滤、多选候选运行和双 Action 资源比较；
- Trial 结果分布、每次尝试、耗时与重跑；
- Action/Resource 依赖图、实验归因说明、缩放和 SVG 导出；
- 文件树、unified/side-by-side Diff；
- 有界 Artifact 读取；
- 精确 Token/成本、预算比例和 CSV 导出；
- Model、Pricing/Budget、Analysis、Safety 和 Appearance 分组设置。

Session 列表使用服务端分页；左右分隔条、主题、字体、紧凑模式和 reduced motion 都保存在本机 UI 状态中。Composer 支持多行、输入历史、Slash Command、`@action`、`@trial`、`@artifact`、路径拖入、Steer 和 Cancel。

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

- Tauri capability 只启用 `core:default` 和原生 dialog 默认权限，不授予任意 Shell、通用文件系统或网络权限。
- CSP 只允许应用自身资源、Tauri IPC 和本地 asset scheme。
- Markdown 经过 `rehype-sanitize`，不会执行消息中的 HTML 或脚本。
- API Key 值不进入前端状态；初始化只返回 `has_api_key` 和环境变量名称。连接测试由 Rust 读取环境变量并调用 `/models`，结果没有虚构 Token 用量。
- 当前构建不启用 OS Keychain，Settings 会明确显示平台回退；凭证继续只通过 `api_key_env` 引用，绝不写入 TOML、SQLite、事件或导出。
- Artifact ID 由 Rust 解析；读取前会 canonicalize 且必须位于该 Session 的 `artifacts/`，单次响应最多 1 MiB。
- Mock endpoint 是不可路由的 `example.invalid`，也不会执行外部请求。
- App Service 和 WriterLock 保持所有业务写入的单一权威路径。

## 检查

```bash
cd apps/fixtrace-desktop
npm run build
npm run typecheck
npm run lint
npm test
npm run e2e
npm run tauri -- build --no-bundle

cd ../..
cargo fmt --all --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Vitest/Testing Library 覆盖 Session list、Transcript、Streaming、Trial、Approval、Cancel、Diff、Usage、Settings、搜索、快捷键、主题、安全输出和 Error Boundary。Playwright 在显式 Mock Backend 下执行新建 Session、消息、Streaming、Tool 展开、Approval、Trial、Cancel、恢复历史、模型连接测试和导出报告的完整流程。Mock 页面始终显示醒目的 `MOCK DATA` 标识。
