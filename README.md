# Cortex

Cortex 是一个专为大模型 GPU 推理集群（SGLang / vLLM）打造的高性能、低延迟集群内部网关，聚焦于：
- 真实 GPU 显存 KV-Cache 事件对齐（ZMQ 旁路账本）
- 精确 Radix 树选路与负载感知
- Prefill-Decode (PD) 分离调度与元数据编排

## 文档索引

建议按以下顺序阅读：

- [总体架构设计](docs/architecture-design.md)：系统定位、组件边界、数据流和演进路线。
- [工程结构与模块说明](docs/project-structure.md)：Rust 后端与 React 控制台代码组织、子系统职责及数据流图。
- [账本同步与一致性](docs/ledger-sync.md)：Worker 生命周期、冷启动快照、Seq Gap 恢复和同步门禁。
- [Tokenizer 与块哈希对齐规范](docs/tokenizer-hash-alignment.md)：版本指纹、引擎隔离、Golden Fixture 和安全降级。
- [高可用部署](docs/ha-deployment.md)：多副本模型、启动门禁、滚动升级和容量边界。
- [运行时与故障语义](docs/runtime-failure-semantics.md)：请求状态机、超时、重试、PD 回退和熔断。
- [Admin UI 设计规范](docs/design.md)：控制台设计基调、十六条硬性纪律与审查清单。

当前仓库处于设计验证阶段，文档中的性能数据均为待实现和基准测试验证的目标。