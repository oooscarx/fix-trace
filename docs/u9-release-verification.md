# U9 课堂演示与发布验证

> 验证日期：2026-08-26（Asia/Shanghai）  
> 干净导出提交：`0bec5ff54908fff0e61d9c75f5e1b98fa9c1582e`

## 干净源码构建

验证从 `git archive HEAD` 导出到全新临时目录开始。导出后确认不存在 `target/` 和 `apps/fixtrace-desktop/node_modules/`，再执行：

```bash
cargo build --workspace --release
cd apps/fixtrace-desktop
npm ci
npm run tauri -- build
```

三步均成功：Rust release workspace 用时 3 分 37 秒；`npm ci` 安装 211 个锁定依赖并报告 0 个已知漏洞；Tauri 前端生产构建和 release bundle 用时 2 分 10 秒。干净导出目录验证后已移入系统废纸篓。

环境：macOS 26.3（25D125）arm64、rustc/cargo 1.93.0、Node.js 25.9.0、npm 11.12.1。

生成物：

```text
target/release/bundle/macos/FixTrace.app
16 MiB
Mach-O 64-bit executable arm64
executable SHA-256: 73044d6c3f59d8f8d850eb395fda7823c338917e3cbd67c95ae09bbfa1d4872e
```

后续 U9 修改仅增加测试、截图生成脚本和文档，没有改动生产 App Service、TUI 或 Tauri 应用源码。最终 HEAD 的全部质量门禁另记录在最终 DoD 审计中。

## 原生应用验收

在同一台 Mac 上启动打包后的 `FixTrace.app`，并令 `FIXTRACE_HOME` 指向由真实 CLI/App Service 离线准备的 Session。原生窗口正确读取：

- 1 个 `Analyzed` Session；
- 2 个真实录制 Action；
- 7 个 Trial；
- 最小 Action 集合 `1, 2`；
- Diagnosis 与 94 个持久化事件；
- Actions Inspector 中的 opaque `printf` 与结构化 `chmod` 资源证据。

这是对本地打包程序的人工 smoke test，不替代自动化 E2E，也不把浏览器 Mock 当作原生后端。证据截图为 `desktop-native-real.png` 和 `desktop-native-actions-real.png`。

## Demo 与截图

`./demo/presentation.sh cli` 在断网、无 API Key 模式下实际运行成功：原始 9 个 Actions，经真实重复重放和最小化得到 `[5, 6]`；完整/最小轨迹为 `StablePass`，分别移除 5 或 6 为 `StableFail`。

`./demo/presentation.sh tui` 实际准备并分析了真实临时 Session，再进入全屏 TUI。Codex 的自动化 PTY 不响应终端 cursor-position query，TUI 因此在初始化时明确失败；`TerminalGuard` 已恢复 raw mode、alternate screen 和 cursor。正常布局、Approval、终端恢复和真实垂直工作流由 TestBackend/集成测试覆盖。此限制不表述为普通终端失败。

截图来源：

- `tui-main.png`、`tui-approval.png`：Ratatui TestBackend 的受测快照，经确定性 SVG/PNG 渲染；
- `gui-main.png`、`gui-graph.png`、`gui-diff.png`、`desktop-approval-mock.png`：Playwright 显式 Mock 模式，顶部持续显示 `MOCK DATA`；
- `desktop-native-real.png`、`desktop-native-actions-real.png`：打包后的原生 App 和真实离线 Session。

运行 `./demo/capture_screenshots.sh` 会重新执行 TUI 快照转换和 GUI E2E，而不是复制手工设计稿。

## 签名与分发限制

当前 `.app` 是 Tauri/rustc 生成的 ad-hoc、linker-signed bundle：`TeamIdentifier` 未设置、资源未 sealed。`spctl` 因此不会把它认作可公开分发的 Developer ID 应用，并报告签名资源不完整。本机课堂演示可直接运行；对外分发前必须配置 Apple Developer ID、完整 codesign、notarization 和 stapling。FixTrace 不把当前制品描述为已公证或 Gatekeeper-ready。
