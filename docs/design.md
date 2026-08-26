# FixTrace 设计文档

## 1. 产品定位与痛点

Rust/Cargo 项目调试经常是一个累积过程：开发者会清理构建缓存、调整环境变量、修改配置、执行格式化、修改文件权限并反复构建。项目最后恢复成功时，终态 diff 只能说明“现在有什么不同”，无法证明哪些中间操作真正必要；普通聊天记录也不能证明某个建议在最初环境中可复现。

FixTrace 服务于希望得到可验证修复证据的 Rust 开发者、课程助教和代码审查者。它把问题从“模型猜测根因”改写为可实验的性质：从固定基线按顺序执行候选 Action 后，Oracle 是否重复得到 StablePass。

## 2. 通用 Agent 为什么做不好

通用 Coding Agent 可以阅读终态代码并给出合理解释，但通常缺少以下约束：

- 没有调试开始前的不可变基线，无法排除当前工作树中的残留状态；
- 不会为每个候选创建全新副本，实验之间可能相互污染；
- 容易把一次偶然成功描述为确定结论；
- 会根据语义直觉删除“看起来无关”的步骤，却不验证硬依赖和顺序；
- 最终解释可能引用不存在的实验或把相关性写成因果性；
- 任意 Shell 工具扩大了副作用和提示注入风险。

## 3. 场景定制设计

### 3.1 重放验证的 dependency-aware ddmin

FixTrace 为 Rust/Cargo 调试定义 Action、Snapshot、Oracle、Trial 和 TrialOutcome。资源规则识别 chmod、cp、mv、rm、Cargo、文件补丁、环境变量和工作目录，只把“基线不存在的文件由 A 创建、B 明确读取”等直接证据建立为硬依赖。ddmin 测试分组、子集和补集，所有候选保持原顺序并求依赖闭包；结束后逐项消融并绕过 cache 再验证。

这是第一项场景定制：最小化目标不是文本长度或 git diff，而是“从 Rust 项目基线重放后 Oracle 重复 StablePass”。

### 3.2 证据约束的 Agent 工具与 Diagnosis

模型不能访问任意 Shell，只能调用会话摘要、Action/Delta 检查、依赖图、候选 Trial、重复/比较 Trial、最小化和 Usage 等十个工具。最终 Diagnosis 必须区分 necessary/removable/uncertain/untested/non-replayable，并引用已存在的 Action/Trial ID；Rust 侧再次验证引用存在。

这是第二项场景定制：LLM 负责解释和选择补充实验，确定性 Rust 核心负责事实、执行权限和成功判据。

### 3.3 可恢复基线与证据存储

`init` 排除 `.git`、`target`、`.fixtrace` 后复制基线和工作树，保存原始权限 Manifest，再把物理基线设为只读。Trial 从只读副本复制后恢复 Manifest 权限并核对 root hash。输出超过 64 KiB 时写入带 SHA-256 的 artifact，避免 SQLite 无界增长。

## 4. 核心交互

1. `init` 验证项目、复制基线、三次验证 Oracle 稳定失败。
2. 用户在 controlled REPL 中执行调试步骤；每步保存前后快照、输出、退出码和耗时。
3. `:done` 从新基线重放完整轨迹，只有 StablePass 才进入分析。
4. `analyze` 验证空/全集前置条件，运行 ddmin、消融和最终复验。
5. 无 Key 时输出离线 Diagnosis；有 Key 时运行带预算的工具调用 Agent Loop。
6. `show/history/export` 提供完整可审计轨迹。

## 5. 关键数据结构

- `SnapshotManifest { root_hash, files }`：BTreeMap 保证确定顺序，root hash 包含类型、内容 hash、大小、权限和链接目标。
- `Action { id, original_order, cwd_before, kind, result }`：记录可重放语义与原始执行证据。
- `Trial { action_ids, repetitions, outcome, baseline_hash }`：每个 repetition 独立复制基线。
- `DependencyGraph { hard_dependencies, accesses }`：硬依赖用于闭包，Opaque 只用于报告。
- `TrialCacheKey`：包含 baseline、Oracle hash、Action IDs 和 repetitions。
- `Diagnosis`：结构化结论、最小 Action、证据分类、引用、限制和 Usage。

## 6. 技术选型

| 技术 | 用途与理由 |
|---|---|
| Tokio / tokio-util | 异步子进程、超时、mpsc 进度和 CancellationToken |
| clap | 类型化子命令和帮助信息 |
| serde / JSON / TOML | 会话、轨迹、Provider 和配置格式 |
| rusqlite bundled | 单文件历史，无需用户安装系统 SQLite |
| sha2 / hex | Snapshot、artifact 和 cache 配置 hash |
| walkdir / tempfile | 可控目录遍历与每次全新 Trial |
| reqwest + rustls | OpenAI-compatible HTTPS，无系统 OpenSSL 依赖 |
| async-trait | Provider 和 Agent tool 的异步统一接口 |
| thiserror | 明确的库层错误语义 |
| tracing | 可配置诊断日志，不记录 API Key |

## 7. 已知限制

- 受控 REPL 不等于内核沙箱；用户录入 Shell 仍需遵守项目内、无外部副作用的范围。
- Shell 资源推断只覆盖可靠的小型规则集，复合或未知命令标记 Opaque。
- UTF-8 FilePatch 不支持二进制和符号链接编辑。
- 最小化是 dependency-constrained 1-minimal，不保证最小基数、唯一或全局最优。
- 结果依赖 Oracle 的确定性；Flaky 只会被报告，不会被折叠为成功/失败。
- Windows 权限、符号链接和进程组支持不完整。

