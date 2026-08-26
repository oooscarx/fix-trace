# FixTrace 实施计划

## 目标与验收表述

FixTrace 面向本地 Rust/Cargo 项目的调试会话，在隔离的基线副本上重放并最小化用户操作。项目只声称：在指定基线、Oracle、环境和重复次数下，找到了经过重放验证的、受依赖约束的 1-minimal 充分修复序列；不声称发现理论上的真实根因或全局唯一最小集合。

课程规格来源：

- [快速入门](https://lab.cs.tsinghua.edu.cn/rust/projects/agent/quick-start/)
- [Agent 架构](https://lab.cs.tsinghua.edu.cn/rust/projects/agent/agent-architecture/)
- [作业要求](https://lab.cs.tsinghua.edu.cn/rust/projects/agent/requirements/)

## 里程碑

### M0：项目骨架

- 建立单 binary crate，固定 Rust 工具链兼容范围并提交 `Cargo.lock`。
- 建立 CLI、配置、错误、日志、领域模块以及 Demo fixture。
- 为配置解析、CLI 结构和 Demo 基线补充基础测试。
- 验收：`cargo fmt --check`、`cargo check`、`cargo test`、严格 Clippy 全部通过。

### M1：确定性重放核心

- 实现只使用项目相对路径的稳定 Snapshot manifest 与 SHA-256 根哈希。
- 实现排除 `.git/**`、`target/**`、`.fixtrace/**` 的本地基线复制。
- 实现 ShellCommand、FilePatch、Set/UnsetEnvironment、ChangeDirectory 五类 Action。
- 在全新基线副本中按原顺序应用候选 Action，并按重复次数运行 Oracle。
- 保存每次执行的退出码、耗时、标准输出/错误和文件状态差异。
- 从 `demo/trace.json` 重放完整轨迹，证明基线稳定失败、完整轨迹稳定成功。
- 验收：上述四项质量检查全部通过，集成测试覆盖 Demo 完整重放。

### M2：最小化

- 实现资源依赖推断、硬依赖闭包、稳定 Trial cache key、dependency-aware ddmin。
- Flaky/Unresolved 不得作为成功；ddmin 后做逐项消融和最终重复验证。
- Demo 的充分序列必须恰好为 Action `{5, 6}`，并保存两项必要性证据。

### M3：录制和 CLI

- 实现 `init`、受控 REPL、编辑/检查点、进度事件、Ctrl+C 取消。
- 使用 SQLite 持久化会话、动作、Trial、证据和 artifact 索引。
- 实现 history、JSON 导入导出；凭据和未获授权的敏感环境变量不得导出。

### M4：LLM Agent

- 实现 OpenAI-compatible Provider、MockProvider 和标准 tool-call Agent Loop。
- 只暴露课程指定的受限分析工具，不允许模型执行任意 Shell。
- 实现结构化 Diagnosis、Token/费用统计、步数/Token/费用/失败预算停止条件。

### M5：文档与展示

- 完成 README、架构与设计文档、R1-R6 映射、演示脚本和开发开销模板。
- 验证无 API Key 的 `--no-llm`/Mock 演示路径与干净构建。
- 不伪造对话历史、Token、费用、开发时长或时间戳。

## 关键风险与应对

1. **Shell 重放边界**：MVP 不实现完整 Shell AST。普通命令由用户录入并在隔离副本中执行；仅对可可靠识别的命令做资源推断，未知命令标记 Opaque。
2. **外部副作用**：候选试验仅允许在项目副本中运行，但任意 Shell 仍可能访问网络或系统。课程 MVP 明确排除系统级、远程和不可撤销操作；后续录制层将拒绝高风险命令，并在文档中明确残余风险。
3. **路径与符号链接逃逸**：所有持久化路径使用项目相对路径；规范化后校验根目录边界；复制和补丁应用不跟随可逃逸根目录的符号链接。
4. **确定性与 Flaky**：重要候选默认重复三次；混合结果标记 Flaky，绝不按成功处理。Oracle 本身不稳定时拒绝有效分析。
5. **文件元数据可移植性**：Unix 保存并重放权限；其他平台保留内容语义，并显式报告不支持的元数据。
6. **进程取消**：Tokio CancellationToken 贯穿 Trial、Oracle 和 Agent；Unix 优先终止进程组，任何平台至少终止直接子进程。
7. **存储膨胀与敏感信息**：大 stdout/stderr 使用有 hash 的 artifact；API Key 只从环境变量读取，日志、数据库和导出前统一脱敏。
8. **最小化复杂度**：缓存以 baseline、Oracle、Action IDs 和重复次数共同定址；依赖闭包减少无效候选，预算和取消防止无界试验。
9. **LLM 不可靠性**：重放与最小化由确定性 Rust 核心完成；LLM 仅通过受限工具读取证据、安排补充实验和解释，不成为成功判据。

## 范围边界

MVP 支持本地 Rust/Cargo 项目、Linux/macOS、项目内文件与权限、会话环境变量、工作目录、普通非交互式命令和确定性 Oracle。它不支持远程 SSH、系统包/服务/内核修改、GUI 录制、Windows 完整兼容、Docker 强依赖、strace/eBPF、不可逆外部 API 副作用，也不会自动修改用户原项目。

## 提交策略

每个里程碑在质量检查通过后形成独立提交；里程碑内若包含可独立验证的基础设施，会用更小的 Conventional Commit 提交。提交信息描述已经验证的事实，不把未完成能力写成完成状态。
