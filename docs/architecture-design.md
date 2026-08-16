# Cortex 集群推理网关架构设计文档 (Architecture Design)

## 1. 概述与定位

### 1.1 系统定位
**Cortex** 是一个面向现代化大模型 GPU 推理集群（SGLang / vLLM）的**高性能、低延迟、集群内部真实显存 KV-Cache 状态感知与 Prefill-Decode (PD) 分离编排网关（Slim Cluster Inference Gateway）**。

Cortex 部署在 GPU 机房内网（Intranet / Cluster Edge），直接贴近推理 Worker。它不承担全局 API 治理（租户计费、多云路由等由上游 XRouter 负责），也不做胖网关业务（去 MCP、去 WASM、去复杂会话持久化），更不去重写数据中心分布式张量搬运（NIXL/KVBM）或 etcd/NATS 控制底座。

Cortex 专注于一件事：**以毫秒级吞吐实时摄取推理引擎的底层显存事件，维护真账本 Radix 树选路，并高效编排 Prefill-Decode 两阶段流量。**

### 1.2 架构分层与边界

```
[ 客户端 / Agent / IDE 插件 / SDK ]
                  │
                  ▼ (HTTPS / 鉴权 / 租户计费 / 全局路由)
┌─────────────────────────────────────────────────────────────┐
│                    XRouter (全局治理网关)                     │
│  - 多租户配额、协议转换 (OpenAI / Anthropic / Gemini)        │
│  - Prompt 清洗规范化、L1/L2 启发式指纹、请求对冲 (Hedged)     │
└──────────────────────┬──────────────────────────────────────┘
                       │ (OpenAI HTTP / VPC 内网)
                       ▼
┌─────────────────────────────────────────────────────────────┐
│                     CORTEX (集群推理网关)                    │
│  - 纯 Rust 打造，超低开销数据面 (< 1ms 选路时延)               │
│  - 双轨解耦：请求热路径 (Fast Path) vs 显存事件账本 (ZMQ Path)│
│  - 精准 Radix 树：Tokenizer + SHA256/Block Hash 状态树      │
│  - 智能打分：KV Overlap 收益 vs 节点并发负载防击穿 (Stampede) │
│  - PD 分离编排：Prefill/Decode 池调度 + 传输元数据握手透传     │
└──────────────┬───────────────────────────────┬──────────────┘
               │ (HTTP / gRPC)                 │ (ZMQ 旁路事件)
               ▼                               ▼
┌─────────────────────────────────────────────────────────────┐
│                 GPU Worker 集群 (自建算力池)                  │
│  - SGLang Worker 1..N (开启 --kv-events-config zmq)         │
│  - vLLM Worker 1..N   (开启 enable_kv_cache_events zmq)     │
│  - [可选集成] Dynamo Frontend                                │
└─────────────────────────────────────────────────────────────┘
```

### 1.3 设计状态与目标

本文描述 Cortex 的目标架构，当前仓库处于设计验证阶段。文中的 `< 1ms` 选路延迟、`< 2ms` Tokenizer 开销和“数秒内完成账本同步”均为待基准测试验证的目标，而非已经达成的服务等级承诺。

Cortex 不持久化 KV 账本，但它不是无运行时状态服务：每个实例都维护 Worker 生命周期、事件序号、同步状态和 Radix HashTree。生产部署必须显式处理账本重建与多副本账本差异，详见[账本同步与一致性](ledger-sync.md)和[高可用部署](ha-deployment.md)。

---

## 2. 核心架构与五大子系统

```mermaid
flowchart TD
    subgraph EventPlane[1. 事件摄取与账本平面 (Background Async)]
        ZMQ[ZMQ Subscriber Worker] --> Parser[多引擎协议解码器 SGLang / vLLM]
        Parser --> SeqCheck{Seq 乱序/丢包检查}
        SeqCheck -->|正常| Apply[Radix HashTree 内存账本更新]
        SeqCheck -->|掉线/乱序| Sync[触发状态重同步 / 降级摘除]
    end

    subgraph DataPlane[2. 请求热路径选路平面 (Fast Request Path)]
        Req[接收 OpenAI HTTP 请求] --> Tokenize[Fast Tokenizer & Hasher]
        Tokenize --> Match[Radix HashTree 最长公共前缀查询 LCP]
        Match --> Score[综合评分: KV命中块数 vs Worker排队水位]
        Score --> Plan{是否开启 PD 分离?}
        Plan -->|单阶段| DirectFwd[挑选最优单节点直接转发]
        Plan -->|PD 两阶段| PDSched[Prefill 池选路 -> 握手 -> Decode 池流式接力]
    end

    Apply -.->|无锁/只读原子视图| Match
    DirectFwd --> Workers[(GPU Workers)]
    PDSched --> Workers
```

### 2.1 显存事件摄取与真账本系统 (Async KV Ingestion Engine)
* **ZMQ 旁路异步订阅**：
  * 对每个 Worker 实例维护独立的 ZMQ `SUB` 链路（基于 Worker 的 `host:port_base + dp_rank`）。
  * 消费 `BlockStored`、`BlockRemoved`、`AllBlocksCleared` 三类核心事件。
* **多引擎协议隔离**：
  * **SGLang 协议栈**：处理基于 Token 序列与 `page_size` 计算的 SHA256 递归哈希链。
  * **vLLM 协议栈**：订阅包含 `block_hashes`、`token_ids`、`extra_keys`（LoRA/多模态/Salt）的事件体。
  * 严格隔离两套账本，禁止跨引擎共用一棵树。
* **数据一致性与生命周期管理**：
  * **Seq 序号校验**：每个 Worker 产生的事件带有连续递增 `seq`。发现重复事件时幂等丢弃；发现跳号、乱序或连接中断时，立即把该 Worker 标记为 `STALE`，停止基于其账本进行 KV 亲和选路，并触发全量重同步。
  * **节点上下线生命周期**：Worker 探针心跳死亡时，先从 Live Pool 摘除，再原子级剪枝清理 HashTree 上的关联节点，杜绝迟到事件造成脏写。
  * **同步门禁**：Worker 只有在健康检查通过且账本状态为 `READY` 时才能参与精确 KV 选路；`SYNCING` / `STALE` Worker 仅可按配置参与无 Cache 假设下的负载降级路由。
  * 冷启动、事件缺口与重连的完整状态机见[账本同步与一致性](ledger-sync.md)。

### 2.2 精确 Radix 树与哈希引擎 (Radix HashTree & Hasher Engine)
* **高性能 Radix HashTree 内存数据结构**：
  * 节点存储：`Block Hash`、`Prefix Length`、`Workers Bitset / Set`、`LRU 时间戳`。
  * **读写分离并发控制**：选路高频读取必须做到零锁或近无锁（基于 RCU / `crossbeam-epoch` / `ArcSwap`），ZMQ 事件流写入在后台批量原子合并更新。
* **网关侧 Tokenizer 对齐与 Cache**：
  * 内置针对各个模型的 Fast Tokenizer（通过本地 Tokenizer 库加载对应模型的 `tokenizer.json` 与 `chat_template.jinja`）。
  * 网关计算出请求的 Token IDs 后，按与 Worker 严格相同的分页算法（如 16 / 64 tokens per page）计算出前缀 Block Hash 序列，进 Radix 树执行 $O(\text{depth})$ 快速匹配。
  * Tokenizer、Chat Template、`page_size`、哈希算法和模型扩展键必须作为同一个不可分割的版本化配置发布；任一字段无法确认一致时，禁止使用精确 KV 路由。详见[Tokenizer 与块哈希对齐规范](tokenizer-hash-alignment.md)。

### 2.3 负载与亲和力协同调度器 (Locality & Load Aware Scheduler)
单纯选“命中 Cache 最多的 Worker”会导致请求倾倒（Cache Stampede / 热点过载）。调度器必须执行多维加权：
* **调度打分模型**：
  $$\text{Score}(w) = \alpha \cdot \text{Matched\_Tokens}(w) - \beta \cdot \text{Active\_Requests}(w) - \gamma \cdot \text{Queue\_Latency}(w)$$
* **过载避让与动态熔断**：
  * 当最优 Cache 节点的当前并发或排队超过动态水位上限（High-Watermark），判定为“KV 收益已无法弥补排队延迟”，强制放弃部分 Cache 亲和，退化为在次优节点或空闲节点冷启动。
* **优雅降级（Graceful Degradation）**：
  * 调度优先级固定为：`exact_kv`（账本可信且存在命中）→ `load_aware`（账本可信但无命中）→ `p2c`（负载指标部分缺失）→ `round_robin`（仅存活信息可用）。
  * 若 Tokenizer 计算失败、Template 错配、账本非 `READY` 或所有 Worker 均无命中，不得推测 Cache 命中，必须进入上述负载降级路径。

### 2.4 PD 分离编排器 (Prefill-Decode Disaggregation Orchestrator)
当集群部署专用的 Prefill 实例池与 Decode 实例池时，Cortex 承担编排大脑：
* **两阶段调度流程**：
  1. **Prefill 阶段选路**：根据 Prompt 真实 KV 账本，将请求派发到 Prefill 池中匹配度最高、算力最充裕的 Worker。
  2. **握手与元数据捕获**：
     * SGLang 体系：捕获响应头/报文中的 `bootstrap_info`（包含 Prefill 端建立的 RDMA/TCP 传输 Room 和端口）。
     * vLLM 体系：透传与挂载 `kv_transfer_params`。
  3. **Decode 阶段接力**：将握手元数据连同请求注入到挑选出的 Decode Worker，Decode 节点拉取 KV 张量后开始自回归生成，Cortex 将 SSE Token 流无缝透传回客户端。
* **失败边界**：
  * 在向客户端发送首个响应字节前，可按明确的幂等约束重选 Prefill / Decode Worker；开始流式响应后不得透明重放请求。
  * Prefill 成功但 Decode 握手或 KV 传输失败时，默认回退到单节点冷启动执行；若请求或引擎不支持安全重放，则直接返回可观测的上游错误。
  * 超时、重试和回退规则见[运行时与故障语义](runtime-failure-semantics.md)。

### 2.5 极速数据面与可观测性 (Data Plane & Metrics)
* **Zero-Cost 异步流式管道**：
  * 采用 Rust `tokio` + `hyper` / `axum`，实现真正的流式 Zero-Copy SSE 转发。
* **回传治理元数据（与 XRouter 协同）**：
  * 在响应 Header 中透传：
    * `x-cortex-cache-hit-tokens: 2048`
    * `x-cortex-assigned-worker: sgl-worker-03`
    * `x-cortex-match-mode: exact_kv_events | fallback_p2c`
  * 方便 XRouter 记录请求日志与进行 Cache Break 断裂归因分析。
* **最小生产可观测性**：
  * Phase 1 即提供存活与就绪探针，并暴露路由延迟、哈希耗时、事件缺口、账本同步状态、降级原因和 Worker 负载指标。
  * `ready` 只表示实例能够安全接收请求；账本尚未完成同步时允许以降级模式就绪，但必须通过指标和响应 Header 明确标识。

---

## 3. 关键设计原则 (Design Principles)

| 维度 | Cortex 的取舍 | 理由 |
| :--- | :--- | :--- |
| **内存/计算开销** | 网关侧跑 Fast Tokenizer 计算 Hash | 必须准确算出与引擎一致的页 Hash 才能命中树，通过 Rust 高性能分词将开销控制在 < 2ms |
| **KV 张量管理** | **不存、不碰、不搬运任何 KV Tensor** | 网关只做控制流与元数据握手，张量搬运由引擎底层的 NIXL / RDMA / NCCL 完成 |
| **持久化与状态** | **无外部持久化，运行时有状态** | 内存账本可丢弃并重建；重建期间关闭精确 KV 路由，不把未完成同步的状态当作真账 |
| **多副本一致性** | **副本独立消费事件并独立维护账本** | 初始实现不引入分布式共识；通过上游负载均衡、同步门禁和降级路由保证安全，接受短暂命中率损失 |
| **与 Dynamo 的关系** | **单一调度权威** | 同一流量路径上只允许一方执行 KV / PD 调度；Dynamo 已承担调度时，Cortex 仅作透明代理并关闭自身 KV / PD 决策 |

---

## 4. 实施演进路线 (Roadmap)

### Phase 0：协议与基线冻结
- [ ] 固化 Worker 注册、全量快照、增量事件及事件缺口恢复协议。
- [ ] 建立 Tokenizer / Chat Template / Page Hash 的版本清单与 Golden Fixtures。
- [ ] 明确单实例和多副本部署模式、Radix 树内存预算及故障降级语义。

### Phase 1：SGLang 单引擎真账对齐（MVP 验证）
- [ ] 搭建 Rust 异步 HTTP 基础反代服务与动态 Worker 注册池。
- [ ] 实现 SGLang ZMQ 事件订阅器（解码 `BlockStored`, `BlockRemoved`, `AllBlocksCleared`）。
- [ ] 实现高性能内存 `RadixHashTree`（支持按 SHA256 Block 挂载 Worker 引用）。
- [ ] 网关集成 Fast Tokenizer 与 SGLang 兼容的递归 SHA256 页哈希计算。
- [ ] 实现基础 LCP（最长公共前缀）选路与统一降级链（Load-Aware → P2C → Round-Robin）。
- [ ] 提供 `/health/live`、`/health/ready` 和最小 Prometheus 指标集。
- [ ] **验收**：双 Worker 共享 Prompt 压测，验证同类请求 100% 精准落到持有显存的那台；清空显存后命中精准消失。

### Phase 2：智能负载防击穿与 vLLM 引擎适配
- [ ] 引入 Overlap + Concurrency 混合打分器，实现高水位熔断防雪崩（Hotspot Prevention）。
- [ ] 适配 vLLM ZMQ 事件格式与基于 `token_ids` / `block_hashes` 的独立 Radix 账本。
- [ ] 完善 Worker 心跳探针与掉线自动剪枝回收机制。

### Phase 3：PD 分离两阶段编排
- [ ] 支持分组配置 Prefill Worker 池与 Decode Worker 池。
- [ ] 实现 SGLang `bootstrap_info` 与 vLLM `kv_transfer_params` 的两阶段状态机与元数据透传管道。

### Phase 4：集群化生产打磨与 XRouter 上游对接
- [ ] 完善 Prometheus 细粒度可观测性指标、告警规则和容量看板（真实 Cache 命中率、路由收益、Worker 负载偏置度）。
- [ ] 标准化与 XRouter 的对接契约（专用 Cache Header 回传与动态集群发现）。
