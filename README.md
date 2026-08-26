# FixTrace

FixTrace 是一个面向本地 Rust/Cargo 调试会话的可验证最小修复 Agent。它把用户的调试操作记录为有序 Action，在全新基线副本中重复重放不同候选集合，并使用用户定义的 Oracle 判断候选是否足以修复项目。

FixTrace 的准确结论是：

> 在指定基线、成功判据、环境和重复次数下，找到经过重放验证的 dependency-constrained 1-minimal sufficient repair trace。

它不会声称找到了哲学意义上的真实根因、全局最小集合或唯一最小集合。

## 主要能力

- 为项目创建隔离、只读的基线和独立工作副本。
- 通过受控 REPL 记录 Shell、文件替换、环境变量和工作目录操作。
- 每个 Trial 都从全新基线副本开始，并验证 baseline hash。
- 重要候选默认重复三次，区分 StablePass、StableFail、Flaky、Unresolved 和 Cancelled。
- 推断有直接证据的资源依赖，执行 dependency-aware ddmin 和逐项消融。
- OpenAI-compatible Chat Completions Provider 与不依赖网络的 MockProvider。
- Agent 只能调用十个证据工具，不能执行模型生成的任意 Shell。
- SQLite 历史、JSON 导入导出、Token/费用统计、预算停止和 Ctrl+C 取消。
- 大于 64 KiB 的 stdout/stderr 自动保存为带 SHA-256 的 artifact，SQLite 只保留截断预览和索引。

## 环境要求

- Rust 1.93 或兼容的更新稳定工具链
- macOS 或 Linux
- 本地可运行的 Cargo 项目
- 一个确定性的非交互式 Oracle 命令

构建：

```bash
cargo build --release
```

安装到 Cargo bin 目录：

```bash
cargo install --path .
```

## 五分钟演示

默认 Demo 使用 MockProvider，不访问网络，也不需要 API Key：

```bash
cargo run -- demo
```

完全跳过模型循环，只运行确定性核心：

```bash
cargo run -- demo --no-llm
```

Demo 会验证：

1. 空操作集合三次均失败；
2. 九步完整轨迹三次均成功；
3. 最小充分集合恰好为 `{5, 6}`；
4. 去掉 5 或 6 的逐项消融均三次失败；
5. 最终集合再次三次通过；
6. 默认模式下 MockProvider 完成一次工具调用 Agent Loop，并输出带 action/trial 引用的 Diagnosis 和 Usage。

也可以运行 [`demo/run.sh`](demo/run.sh)。

## 实际项目流程

创建会话：

```bash
fixtrace init /path/to/rust-project --oracle 'cargo test --test acceptance'
```

命令会输出 session ID。进入受控 REPL：

```bash
fixtrace shell <session-id>
```

REPL 支持：

```text
普通非交互式 Shell 命令
cd <project-relative-path>
export KEY=value
unset KEY
:edit <relative-path>
:checkpoint <note>
:verify
:status
:done
:quit
```

`:done` 会从全新基线重复重放完整轨迹。只有 StablePass 会让会话进入可分析状态。

运行最小化与诊断：

```bash
fixtrace analyze <session-id>
```

没有配置 API Key 时，`analyze` 自动输出确定性离线 Diagnosis。显式禁用模型：

```bash
fixtrace analyze <session-id> --no-llm
```

## 模型配置

默认配置位于 `~/.fixtrace/config.toml`。API Key 的值不能写入配置；这里只保存环境变量名。

```toml
[model]
provider = "openai-compatible"
endpoint = "https://api.openai.com/v1"
api_key_env = "FIXTRACE_API_KEY"
model = "gpt-5-mini"
api_style = "chat-completions"
context_length = 32768
reasoning_mode = "medium"
max_agent_steps = 12

[pricing]
input_per_million_usd = 0.0
output_per_million_usd = 0.0

[budget]
max_total_tokens = 100000
max_cost_usd = 1.0

[replay]
repetitions = 3
oracle_timeout_secs = 120
include_target = false
```

设置密钥和配置：

```bash
export FIXTRACE_API_KEY='your-key'
fixtrace config set model.endpoint https://example.com/v1
fixtrace config set model.model your-model
fixtrace config set pricing.input_per_million_usd 1.5
fixtrace config show
```

`model.api_key` 会被拒绝；请设置 `model.api_key_env`。

## 历史与迁移

```bash
fixtrace history list
fixtrace history show <session-id>
fixtrace show <session-id>
fixtrace export <session-id> --output session.json
fixtrace import session.json
```

导出包含会话、Actions、Trials、尝试证据、消息、工具调用、Usage、进度事件和 Diagnosis。名称包含 API_KEY、TOKEN、SECRET、PASSWORD 或 AUTHORIZATION 的环境值会被脱敏。

使用独立状态目录进行测试或隔离：

```bash
fixtrace --state-dir /tmp/my-fixtrace history list
```

也可以设置 `FIXTRACE_HOME`。

## 测试与质量检查

```bash
cargo fmt --all --check
cargo check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

测试覆盖稳定 Snapshot、内容/权限差异、路径逃逸、环境重放、硬依赖闭包、ddmin、Flaky、cache key、费用公式、JSON 往返、取消、Mock Agent、历史记录以及完整 Demo。

## 安全边界与限制

- Trial 和候选实验只在基线副本中运行；FixTrace 不自动修改用户原项目。
- FilePatch、工作目录和内部路径 API 拒绝绝对路径、`..` 逃逸和通过符号链接写出根目录。
- Unix 子进程使用独立进程组，取消时先 SIGTERM、超时后 SIGKILL。
- LLM 只能选择当前会话已有的 Action ID，不能生成或执行新 Shell。
- 普通 Shell 由用户本人录入。MVP 不实现完整 Shell AST 或操作系统级沙箱，因此用户仍需避免系统修改、网络副作用以及指向项目外部的 Shell 重定向。
- FilePatch 当前只保存 UTF-8 普通文件；符号链接补丁和二进制编辑会报告不可重放。
- Linux/macOS 是优先平台；Windows 权限和进程组语义未完整支持。
- 不支持 SSH、apt/systemd/内核修改、GUI 录制、Docker 强依赖、strace/eBPF 或不可撤销外部 API。

## 设计资料

- [实施计划](docs/implementation-plan.md)
- [设计文档](docs/design.md)
- [系统架构](docs/architecture.md)
- [课程要求映射](docs/requirements-matrix.md)
- [AI 开发开销空白模板](docs/development-cost-template.csv)

## License

MIT
