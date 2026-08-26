# 安全模型

FixTrace 的 UI 是 App Service 客户端，不是第二个命令执行器。CLI、TUI 和 GUI 发出类型化请求；重放、路径解析、审批、事件持久化和 secret 处理全部在 Rust 边界内完成。

## 凭证与传输

- 配置只保存 `model.api_key_env`，拒绝保存 `model.api_key`；初始化只暴露环境变量名和 `has_api_key`。
- API Key 值不得进入 React state、TOML、SQLite、日志、event、artifact 或 Session export；连接测试由 Rust 读取环境变量。
- 当前 GUI 明确使用环境变量引用回退，未启用 OS Keychain。
- WebSocket 默认只绑定 loopback，capability token 从权限受限文件读取，不进入 URL 或日志；跨客户端仍经过单一 writer lock。

## 执行与审批

- 模型只能选择已记录 Action 或调用十个证据工具，不能生成并执行任意 Shell。
- Approval 展示 kind、风险、完整命令预览、cwd、sandbox、路径、Action IDs、网络标记和 scope；一次/Task/Session 等价复用由 Rust 比较结构化字段，Deny/Cancel 不执行目标动作。
- Trial 总从只读 baseline 的新副本开始；候选前后校验 baseline hash。
- FilePatch、Artifact 和内部路径 API canonicalize 后必须留在 Session 根内，并拒绝 `..`、绝对路径及 symlink 逃逸。
- 子进程位于独立进程组；取消先 SIGTERM，超时后 SIGKILL。

## 输出与前端

- React Markdown 经过 `rehype-sanitize`；命令输出中和 C0、DEL/CSI 和双向文本控制字符。
- Tauri capability 不授予任意 Shell、通用文件系统或网络权限；CSP 仅允许应用资源、IPC 和本地 asset scheme。
- 大输出外置为带 SHA-256 的 artifact；预览、事件帧和 artifact range 均有大小上限。

## 明确限制

用户本人在录制 REPL 中输入的普通 Shell 不是完整 Shell AST 或 OS 级沙箱，仍可能产生项目外副作用。当前优先支持 Linux/macOS；Windows 进程组和权限语义未完整验证。详见 [U8 审计](u8-hardening.md) 和 README 的“安全边界与限制”。
