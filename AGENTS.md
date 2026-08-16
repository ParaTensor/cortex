# Cortex 智能体协作与工程规范 (AGENTS.md)

本文档是 AI 编程助手与开发者在 `cortex` 项目中协作的常驻核心契约与工作流指南。

---

## 1. 项目定位与核心原则

* **定位**：Cortex 是专为 GPU 推理集群（SGLang / vLLM）打造的高性能、低延迟、集群内部真实显存 KV-Cache 状态感知与 Prefill-Decode (PD) 分离编排网关。
* **边界原则**：
  * **厂商中立治理归上游**：多租户配额、计费、多云路由归 XRouter；Cortex 专精集群内真 KV 账本与 PD 编排。
  * **不存不搬 KV 张量**：张量传输由引擎底层 RDMA/NIXL/NCCL 完成，网关只做控制流、元数据握手与 HTTP/gRPC 反代。
  * **无外部持久化，运行时有状态**：无外部数据库依赖，内存即账本；通过快照与增量 ZMQ 事件快速同步。
  * **单一调度权威**：与 Dynamo 共存时只做透明反代，不双开 ZMQ 争抢账本。

---

## 2. 按需技能地图 (Skills Routing)

在执行具体任务前，AI **必须**先查阅下表。若当前任务命中对应场景，必须使用 `view_file` 读取对应 `SKILL.md` 并严格按规约执行：

| 触发场景 | AI 必须自动阅读并执行的 Skill | 核心关注点 |
| :--- | :--- | :--- |
| **初始化工程骨架 / 新增 Rust crate / 架构重构** | [`.agents/skills/scaffold-ai-project/SKILL.md`](file:///.agents/skills/scaffold-ai-project/SKILL.md) | Rust 2024 edition 规范、workspace 模块解耦、零拷贝流水线与基准测试 |
| **修改 Admin 控制台 / 可视化监控大盘 / UI 组件** | [`.agents/skills/admin-ui-change/SKILL.md`](file:///.agents/skills/admin-ui-change/SKILL.md) | UI 设计规范、组件一致性、Tailwind / 状态管理、Token 统一 |
| **代码临时暂存 / 清理工作区 / 跑测试前分支切换** | [`.agents/skills/git-stash-safe/SKILL.md`](file:///.agents/skills/git-stash-safe/SKILL.md) | 防止误删未跟踪文件与破坏构建依赖 |
| **解决复杂坑点 / 发现高频误判 / 架构决策沉淀** | [`.agents/skills/promote-lesson/SKILL.md`](file:///.agents/skills/promote-lesson/SKILL.md) | 经验元代码化，更新 `AGENTS.md` 或创建新 Skill |

---

## 3. 开发与设计准则

1. **Rust 2024 Edition 规范**：
   * 采用现代异步并发体系（Tokio, Axum/Hyper, Tower, Tracing）。
   * 性能敏感路径（选路、Radix 树遍历、流式转发）严格追求 Zero-Allocation / Zero-Copy。
2. **规范与文档对齐**：
   * 账本同步与生命周期必须严格遵循 [`docs/ledger-sync.md`](docs/ledger-sync.md)。
   * Tokenizer、Chat Template 与 Block Hash 计算必须严格遵循 [`docs/tokenizer-hash-alignment.md`](docs/tokenizer-hash-alignment.md)。
   * 故障降级与 PD 状态机必须严格遵循 [`docs/runtime-failure-semantics.md`](docs/runtime-failure-semantics.md)。
   * 内存预算与部署模型遵循 [`docs/ha-deployment.md`](docs/ha-deployment.md)。
3. **测试驱动与 Golden Fixture**：
   * 对分词、分页、哈希算法，必须编写基于真实 Worker 录制的 Golden Fixtures 单测，拒绝凭空臆想哈希规则。
