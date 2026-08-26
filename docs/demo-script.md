# 课堂演示脚本

所有演示均可在无网络、无 API Key 环境运行。推荐先执行：

```bash
./demo/presentation.sh cli
```

讲解顺序：9 个原始 Actions → 完整轨迹 `StablePass` → ddmin 得到 `[5, 6]` → 最小轨迹 `StablePass` → 分别移除 5/6 均 `StableFail`。这是实际 baseline copy、Action replay 和 Oracle repetition，不是写死的 UI 结论。

随后按课堂环境选择：

```bash
./demo/presentation.sh tui       # 真实临时 Session + TUI
./demo/presentation.sh desktop   # 真实临时 Session + Tauri dev window
./demo/presentation.sh mock-gui  # 固定视觉流程，始终显示 MOCK DATA
```

TUI/Desktop 会创建、录制、`:done`、离线 `analyze` 一个临时 Session，再打开同一 App Service 视图；退出后默认清理。设置 `FIXTRACE_PRESENTATION_ROOT=/path/to/keep` 可保留现场。Mock GUI 只用于稳定展示 Sidebar、Streaming、Tool、Approval、Trial、Graph、Diff、Usage 和 Settings，不宣称发生外部 API 调用。

截图重建：

```bash
./demo/capture_screenshots.sh
```

若课堂机器不能启动窗口，使用 CLI 结果和仓库内截图；不要临时填写真实 API Key，也不要把固定 Mock Usage 描述成实测费用。
