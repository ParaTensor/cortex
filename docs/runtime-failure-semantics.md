# 运行时与故障语义

## 1. 请求阶段

Cortex 将请求划分为以下阶段，以确定超时、重试和可观测边界：

```text
ACCEPTED
  -> ROUTING
  -> SINGLE_FORWARDING
  -> PREFILL_FORWARDING
  -> DECODE_CONNECTING
  -> STREAMING
  -> COMPLETED | FAILED
```

`ROUTING` 只读取已发布账本快照。单阶段模式从 `ROUTING` 进入 `SINGLE_FORWARDING`；PD 模式依次进入 `PREFILL_FORWARDING` 和 `DECODE_CONNECTING`。一旦向客户端发送响应头或首个 SSE 字节，请求进入不可透明重放的 `STREAMING`。

## 2. 超时预算

必须使用请求总截止时间，而不是让每个阶段分别获得完整超时。各阶段预算从总截止时间中扣减：

```text
route_timeout
worker_connect_timeout
prefill_timeout
kv_transfer_timeout
decode_first_token_timeout
stream_idle_timeout
request_deadline
```

客户端断开时，Cortex 应取消上游请求和未完成的 PD 操作；无法取消的引擎任务必须记录为孤儿任务并计入指标。

## 3. 重试原则

透明重试必须同时满足：

- 尚未向客户端发送任何响应字节；
- 能确定目标 Worker 未开始产生不可撤销副作用，或请求携带引擎支持的幂等键；
- 剩余总截止时间足够；
- 重试次数未超过配置上限；
- 新目标 Worker 健康且满足请求能力。

生成请求通常不能仅凭 HTTP 方法判定幂等。默认配置最多执行一次连接级重选；若无法证明安全则不透明重试，由客户端或 XRouter 决定是否重放。

## 4. 单阶段故障

| 故障点 | 行为 |
| --- | --- |
| 路由无可用 Worker | 快速返回服务不可用，不排入无界队列 |
| 建连失败且未发送请求体 | 在安全条件满足时重选一次 Worker |
| 已发送请求但未收到响应 | 默认不透明重试，返回上游错误 |
| 已开始 SSE 后断流 | 结束客户端流并记录中断原因，不拼接另一 Worker 的输出 |
| 客户端取消 | 传播取消，释放并发计数和连接资源 |

无论请求成功或失败，都必须在结束路径中准确释放 Worker 的 `Active_Requests` 计数，避免负载视图永久漂移。

## 5. PD 故障与回退

PD 编排需要一个仅存在于单次请求内的短生命周期状态机，不写入持久化存储。

| 故障点 | 默认行为 |
| --- | --- |
| Prefill 建连失败 | 满足重试原则时重选 Prefill Worker |
| Prefill 超时或响应无法解析 | 取消请求；能证明安全时回退单节点执行 |
| `bootstrap_info` / `kv_transfer_params` 缺失 | 视为协议错误，隔离该 PD 能力并回退单节点 |
| Decode Worker 选择失败 | 在剩余截止时间内回退单节点，否则失败 |
| KV 传输失败或超时 | 取消两端传输；能证明安全时由 Decode 冷启动，否则失败 |
| Decode 首 Token 前失败 | 满足重试原则时可重选 Decode 或回退单节点 |
| 已开始 SSE 后 Decode 失败 | 终止流，不透明重放 |
| Prefill Worker 在传输后下线 | Decode 能独立继续则不受影响；否则按 KV 传输失败处理 |

“回退单节点”必须重新评估总截止时间、请求幂等性和容量水位。不能为了隐藏 PD 故障而造成重复生成或突破并发限制。

## 6. 调度降级链

调度模式按以下顺序选择，模式和原因必须写入响应 Header、日志和指标：

| 模式 | 使用条件 |
| --- | --- |
| `exact_kv_events` | Tokenizer / Hash 配置匹配，账本 `READY`，存在有效 KV 命中 |
| `load_aware` | 无有效 KV 命中，但并发和排队指标完整 |
| `fallback_p2c` | 至少有两个健康 Worker，但精细负载数据不完整 |
| `fallback_round_robin` | 仅有健康存活信息或只有一个 Worker |

任何模式都必须先过滤不健康、熔断、角色不匹配、模型不匹配和能力不匹配的 Worker。降级是降低路由精度，不是绕过安全条件。

## 7. 熔断与恢复

熔断状态按 Worker 和能力维度维护，至少区分推理 HTTP、事件流、快照和 PD 传输。单个能力故障不应无条件摘除 Worker 的全部能力，例如快照失败可以关闭精确 KV 路由，但仍允许健康的单阶段推理参与负载降级。

熔断器采用 `closed -> open -> half_open` 状态机。半开探测必须限流；恢复事件连接后仍需完成账本重同步，不能直接恢复精确 KV 路由。

## 8. 错误与可观测性

对客户端返回稳定、低信息量的错误类型，对内部记录完整原因链。至少区分：

```text
no_healthy_worker
route_timeout
upstream_connect_failed
upstream_protocol_error
ledger_not_ready
prefill_failed
kv_transfer_failed
decode_failed
stream_interrupted
request_cancelled
deadline_exceeded
overloaded
```

建议的核心指标：

```text
cortex_request_duration_seconds{mode,status}
cortex_route_duration_seconds{mode}
cortex_route_fallback_total{reason}
cortex_upstream_failure_total{worker_id,stage,reason}
cortex_pd_fallback_total{stage,reason}
cortex_stream_interrupted_total{reason}
cortex_worker_active_requests{worker_id}
cortex_worker_circuit_state{worker_id,capability,state}
```
