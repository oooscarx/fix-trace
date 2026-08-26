# 常见问题

## `/models` 返回 HTTP 400

先检查环境变量是否含前导/尾随空白：

```bash
printf '<%s>\n' "$FIXTRACE_API_KEY"
```

重新从可信来源设置变量，不要把密钥写入配置、命令历史、截图或 Issue。FixTrace 配置只填写 `model.api_key_env = "FIXTRACE_API_KEY"`。

## `request failed: builder error`

通常表示 endpoint、header 值或 URL 在 HTTP 请求构建阶段无效。确认 endpoint 是完整 `http(s)://.../v1` URL，环境变量不含空格/换行，再用 Settings 的 Test connection 或 `/models` 做最小验证。模型存在不等于账号有调用权限，应从 `/models` 返回中选择实际允许的模型。

## App Server / Desktop 报 writer lock

同一状态目录只允许一个权威写者。关闭占用该目录的 Desktop、TUI InProcess 或 `fixtrace-server`，或者通过 WebSocket 连接已有 server；不要删除数据库锁来强制双写。

## TUI 启动后终端异常

正常退出和 panic 都由 `TerminalGuard` 恢复 raw mode、alternate screen、bracketed paste 和 cursor。若宿主 PTY 不支持 cursor-position query，TUI 会恢复后返回明确错误；改用常规 Terminal.app/iTerm2/Kitty。极端情况下运行 `reset` 恢复当前 shell。

## GUI 只显示 Mock 数据

只有 `npm run dev:mock` / `VITE_FIXTRACE_MOCK=1` 会启用 Mock，页面顶部必须显示 `MOCK DATA`。真实应用使用 `npm run tauri -- dev` 或打包后的 `FixTrace.app`；生产模式原生调用失败不会静默切换 Mock。

## macOS 拒绝公开分发包

仓库默认 build 是 ad-hoc 签名，适合本机开发/课堂演示，不是 Developer ID 公证包。公开分发需配置签名证书、notarization 和 stapling；详见 [U9 发布验证](u9-release-verification.md)。

## Trial 与预期不同

确认 Oracle 是确定、非交互命令；检查工作目录、环境变量、权限和超时。FixTrace 会把重复结果不一致标为 `Flaky`，不会把它当作成功；外部网络/系统服务副作用不在本地 baseline 副本的完整控制范围内。
