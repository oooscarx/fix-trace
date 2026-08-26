# FixTrace App Server

`fixtrace-server` 把同一个 `FixTraceAppService` 暴露为 stdio JSONL 或带认证的本地 WebSocket。两种传输都使用 [`fixtrace/1`](protocol.md) 的 `ClientFrame` / `ServerFrame`，连接的第一条请求必须是 `initialize`。

## stdio JSONL

开发环境启动：

```bash
cargo run -p fixtrace-server -- --listen stdio
```

每行是一个完整 JSON frame，普通 frame 最大 8 MiB。stdout 只写协议 JSONL，tracing 和错误诊断只写 stderr；无效 JSON 返回 `invalid_request` 后继续服务。stdin EOF、Ctrl+C 或父 CancellationToken 会触发 graceful shutdown。

## 本地 WebSocket

```bash
cargo run -p fixtrace-server -- \
  --listen ws://127.0.0.1:4765 \
  --token-file .local/fixtrace-server.token
```

若 token 文件不存在，服务端使用操作系统随机源创建 256-bit capability token；Unix 权限固定为 `0600`。已存在但可被 group/other 读取的 token 文件会被拒绝。服务端只记录 token 文件路径，永远不记录 token 内容。

客户端在握手头发送：

```text
Authorization: Bearer <token-file contents>
```

URL 中禁止 userinfo、query 和 fragment，因此 API Key 或 capability token 不能放在 URL。默认只允许 loopback；非回环绑定必须显式传 `--allow-remote`。当前服务器提供 `ws` 而非 TLS 终止，远程使用应放在可信隧道或受控反向代理之后，不能直接暴露到局域网或公网。

Axum 0.8 被选作 WebSocket HTTP upgrade 层，原因是它直接建立在项目已有 Tokio 栈之上，并允许设置 message/frame/write-buffer 上限。连接限制为：

- 单个 message/frame 最大 8 MiB；
- 256 KiB 读写缓冲目标；
- 16 MiB 最大写缓冲，超出时产生背压；
- App Service live broadcast 为 4096 个事件；
- `WebSocketClient` 到 UI 的事件 channel 为 256 个事件。

## 事件恢复和多客户端

订阅成功后，服务端先从 SQLite 读取 `after_sequence` 之后的持久化事件，再转入 live broadcast。广播接收者落后时再次从 SQLite 补发；持久化序列也无法连续时发送 `EventGap`，客户端应调用 `session/get_snapshot` 重建，而不能猜测缺失状态。

`fixtrace-client::WebSocketClient` 在连接中断后以 100 ms 起步、最大 5 s 的指数退避重连，并使用最后已经交给调用者的 sequence 重新订阅。请求保留调用者提供的 `operation_id`，因此网络重试不会重复启动同一 Task。

多个 TUI/GUI 客户端可以订阅同一 Session。事件在写入 SQLite 后才广播，因此它们看到相同 `event_id` 和 sequence。Approval resolution 使用数据库 compare-and-set，只允许第一个客户端成功。

## 单写者与状态目录

启动 binary 时会在数据库旁持有 `app-server.writer.lock` 独占文件锁；同一状态目录的第二个 App Server 会立即失败。默认状态目录仍为 `~/.fixtrace`，也可隔离：

```bash
cargo run -p fixtrace-server -- \
  --state-dir /tmp/fixtrace-ui-test \
  --listen ws://127.0.0.1:4765
```

默认 token 文件为同一状态目录下的 `app-server.token`。数据库迁移和备份规则见 [数据库迁移](database-migrations.md)。

## 验证

```bash
cargo test -p fixtrace-server
cargo test -p fixtrace-client
```

集成测试覆盖 initialize-first、错误 JSON 后继续服务、stdout 纯 JSONL、Bearer 拒绝/接受、loopback 限制、token 权限、单写者锁、两个客户端观察相同事件，以及停服期间写入事件后在同端口重启并补发。
