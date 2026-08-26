# FixTrace SQLite Migration 与事件存储

## 版本

`fixtrace-store` 使用：

```text
schema_migrations(version, applied_at, checksum)
```

当前 schema 为 v2：

- v1 代表原有 core history 表；
- v2 增加 `app_event_streams`、`app_events`、`tasks`、`approvals`、`client_sessions`、`ui_preferences` 和 `operations`。

已记录 migration 的 checksum 必须与二进制内置值一致。数据库版本高于程序支持版本、或已知 checksum 不匹配时，启动立即失败，不继续执行 DDL。

## 安全升级

对已有非空数据库从 v1 升级时：

1. 开启 WAL、foreign keys 和 busy timeout；
2. checkpoint WAL；
3. 使用 SQLite Online Backup API 生成同目录 `history.sqlite3.pre-ui-v2-v1.bak`；
4. 以 `BEGIN IMMEDIATE` 执行 migration；
5. DDL 与 migration 记录在同一事务提交；
6. 运行 `PRAGMA integrity_check`；
7. 任何错误回滚，原 core 表和备份保留。

备份不会覆盖已存在的同名备份。测试使用真实 v1 fixture 验证 Session 行在原库和备份中都存在，也验证篡改 checksum 时 v2 表不会创建。

## Event stream

每个 Session 一个 `app_event_streams` 行；全局任务使用 global stream。`append` 在 `BEGIN IMMEDIATE` 中：

1. 查找/创建 stream；
2. 读取 `next_sequence`；
3. 插入完整 AppEvent payload、event type、schema version 和上下文 ID；
4. 递增 `next_sequence`；
5. commit 后才 broadcast。

`UNIQUE(stream_id, sequence)` 保证同一流不重复。并发测试由 16 个线程同时 append，最终必须得到连续 `1..=16`。

Catch-up 查询 `sequence > after_sequence`。如果第一条可用事件不是 `after + 1`，返回 `EventGap`；客户端丢弃增量假设并请求 SessionSnapshot。

## Task 与 Approval

Task JSON 与可查询 kind/status/timestamp 同时保存。数据库 partial unique index禁止同一 Session 出现两个 `queued/running/waiting_for_approval/cancelling` Task；状态转换还会在 Rust `TaskStatus` 中复核。`operation_id` 唯一，重试不会创建第二个 Task。

Approval 初始状态只能是 pending。解析使用 `UPDATE ... WHERE status='pending'` 的事务 compare-and-set：首个客户端成功，后续响应得到 `approval_resolved`，不会重复执行。API Key 不属于这些表，也不属于 UI preferences。

## 当前与后续恢复

U2 已持久化 Task、最终事件和已完成的 core Trial；客户端断线不会取消后台 Task。App Server 进程重启时将遗留非终态 Task 标为 `Interrupted`、文件锁和独立 writer 进程协调在 U3/U8 完成。完成前不会把这些任务伪装成 Completed。
