# FixTrace TUI

`fixtrace-tui` 是基于 Ratatui 0.30 的真实 App Service 客户端。它不打开 SQLite、不调用 replay/minimize/LLM 模块；所有读取、Task、取消和事件都经过 `fixtrace-client::AppClient` 与 `fixtrace/1` 协议。

## 启动

默认使用 InProcess transport，并持有与 App Server 相同的独占 writer lock：

```bash
cargo run -p fixtrace-tui
cargo run -p fixtrace-tui -- --session <session-id>
cargo run -p fixtrace-tui -- --state-dir /tmp/fixtrace-ui-test
```

连接已经启动的本地 App Server：

```bash
cargo run -p fixtrace-tui -- \
  --connect ws://127.0.0.1:4765 \
  --token-file ~/.fixtrace/app-server.token
```

Token 仅从本地 `0600` 文件读取，不进入 URL、日志、数据库或事件。

## U4 垂直切片

当前端到端路径不是静态 mock：

```text
session/list + session/get_snapshot
→ event/subscribe(after_sequence)
→ message/send
→ Agent Task
→ UserMessage / Trial / ToolCall / AgentMessage delta
→ task/cancel 或 terminal Task event
→ snapshot refresh
```

发送普通消息会启动 evidence-bound Agent Turn。对于 `ReadyForAnalysis`/`Analyzed` Session，它运行真实 replay/minimization；配置模型凭证时使用受限 Agent 工具，无凭证时生成离线 Diagnosis。两种路径都产生完整最终 Agent item，delta 仅用于实时显示。取消沿 Task CancellationToken 到子进程监督器。

## 交互

- `Enter` 发送；`Alt+Enter` 或 `Ctrl+J` 换行；支持 bracketed paste、光标移动、选择和删除。
- 输入 `/` 实时过滤命令；`Ctrl+P` 打开命令面板。
- `Ctrl+O` Session Picker；`Ctrl+B`/`Ctrl+I` 切换 Sidebar/Inspector。
- `Tab`/`Shift+Tab` 切换 Inspector；`PageUp`/`PageDown`、`g`/`G` 浏览时间线。
- 第一次 `Ctrl+C` 取消活跃 Task 并提示；1.5 秒内再次按下才退出。
- `?` 显示帮助。

宽终端显示 Sessions + Transcript + Inspector；中等终端隐藏 Sidebar；窄终端一次显示 Transcript 或一个 Inspector tab。小于 `60×16` 时只显示安全的尺寸提示，不会 panic。

事件循环使用单向数据流：

```text
TuiEvent → update(Model) → Effect → AppClient → EffectResult/AppEvent
                         ↓
                  dirty flag → 30 FPS render
```

`view()` 无 IO。高频 delta 先合并进 Model，每个 33 ms 帧最多完整重绘一次；没有状态变化时不重绘。

## 终端安全

`TerminalGuard` 用 RAII 恢复 raw mode、alternate screen、bracketed paste 和 cursor。panic hook 也执行相同恢复；初始化任一步骤失败时会先恢复终端再返回错误。Resize 是事件源之一，不依赖特定 Terminal Emulator。

## 测试

```bash
cargo test -p fixtrace-tui
cargo test -p fixtrace-client --test tui_vertical
```

快照覆盖 wide/medium/narrow/too-small 四种布局；状态测试覆盖 delta 合并和 Ctrl+C；垂直集成测试从真实 demo baseline 启动 `message/send`，验证 User、Trial、Agent start/delta/complete 和 Task 完成事件。
