# Cortex

Cortex 是一个专为大模型 GPU 推理集群（SGLang / vLLM）打造的高性能、低延迟集群内部网关，聚焦于：
- 真实 GPU 显存 KV-Cache 事件对齐（ZMQ 旁路账本）
- 精确 Radix 树选路与负载感知
- Prefill-Decode (PD) 分离调度与元数据编排

## 文档索引

- [架构设计文档 (Architecture Design)](docs/architecture-design.md)