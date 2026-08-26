# FixTrace TUI

`fixtrace-tui` 是基于 Ratatui 0.30 的真实 App Service 客户端。它不打开 SQLite、不调用 replay/minimize/LLM 模块；所有读取、Task、取消和事件都经过 `fixtrace-client::AppClient` 与 `fixtrace/1` 协议。

## 启动

默认使用 InProcess transport，并持有与 App Server 相同的独占 writer lock：

```bash
cargo run -p fixtrace-tui
cargo run -p fixtrace-tui -- --session <session-id>
cargo run -p fixtrace-tui -- --state-dir /tmp/fixtrace-ui-test
```

仓库根目录的 `./demo/presentation.sh tui` 会自动准备、验证并离线分析一个真实 Session，然后直接打开该 Session，适合课堂展示。

连接已经启动的本地 App Server：

```bash
cargo run -p fixtrace-tui -- \
  --connect ws://127.0.0.1:4765 \
  --token-file ~/.fixtrace/app-server.token
```

Token 仅从本地 `0600` 文件读取，不进入 URL、日志、数据库或事件。

## 真实工作流

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

Session、任务、Action、Trial、Graph、Diff、Usage 和配置均来自 App Service。Diff 由后端实时比较只读 baseline 与 worktree；UTF-8 小文件包含 unified diff，大文件和二进制文件仍显示结构化变更类型。TUI 不读取项目或数据库来补造视图。

创建并完成一条录制会话的最短流程：

```text
/new ./broken-project --oracle 'cargo test --all' --title parser-fix
/record cargo fmt
/record cargo test
/record :verify
/record :done
/analyze
/diagnose Focus on the parser configuration.
```

`/record <command>` 在 Session worktree 中执行一个受控步骤并保存真实 Action；`cd`、`export KEY=value` 和 `unset KEY` 使用和旧 controlled shell 相同的结构化 Action。`/record :status`、`:verify`、`:done` 可查询、验证和完成录制；需要交互式 `:edit`/`:checkpoint` 时仍可使用 `fixtrace shell <session-id>`。

Agent Task 运行时，普通 Composer 输入会发送 `task/steer`。服务端在 Agent 的安全边界把它加入上下文；非 Agent Task 会明确拒绝 Steer，避免意外启动第二个写任务。

## 交互

- `Enter` 发送；`Alt+Enter` 或 `Ctrl+J` 换行；支持 bracketed paste、光标移动、选择和删除。
- 输入 `/` 实时过滤命令；`Ctrl+P` 打开命令面板，`Tab` 可补全；`Ctrl+N` 填入新建 Session 命令。
- `Ctrl+O` Session Picker；`Ctrl+B`/`Ctrl+I` 切换 Sidebar/Inspector。
- `Tab`/`Shift+Tab` 切换 Inspector；`PageUp`/`PageDown`、`g`/`G` 浏览时间线。
- 输入 `@` 搜索 Action、Trial 和 Artifact，`Tab` 插入结构化引用；Composer 非引用状态下按 `Ctrl+X` 会安全退出 alternate screen 并使用 `$VISUAL`/`$EDITOR` 编辑长 Prompt。
- Composer 为空时按 `x` 展开或折叠最近的 Tool Call。
- 第一次 `Ctrl+C` 取消活跃 Task 并提示；1.5 秒内再次按下才退出。
- `?` 显示帮助。

常用 Slash Command 参数：

```text
/fork [title]                 /archive
/verify                       /replay
/repeat <trial-id>            /demo
/model <model>                /effort <mode>
/permissions <policy>         /config <key> <value>
/export [path]                /import <path>
/theme [color|high-contrast|mono]
```

`/permissions` 允许 `read_only`、`ask_always`、`ask_for_opaque`、`auto_recorded_safe`。API Key 只能通过配置的环境变量传入，不能由 `/config` 写入。

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

快照覆盖 wide/medium/narrow/too-small 四种布局；状态测试覆盖 delta 合并、Steer、Slash Command、实体引用、主题和 Ctrl+C；垂直集成测试从真实 demo baseline 启动 `message/send` 和 `task/steer`，验证 User、Trial、Agent start/delta/complete 和 Task 完成事件。InProcess 集成测试还覆盖创建、单步录制、`:done`、真实 Diff、Trial run/repeat、fork、archive 和 Approval Policy 配置。
