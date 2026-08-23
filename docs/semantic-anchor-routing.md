# 语义锚点感知路由（Semantic-Anchored Routing）

> 状态：设计草案（Draft）。本文档扩展 [`ledger-sync.md`](ledger-sync.md) 的账本模型与
> [`tokenizer-hash-alignment.md`](tokenizer-hash-alignment.md) 的分词对齐契约，
> 为 agentic 多轮工作负载引入"前缀存活概率"维度的调度打分。

---

## 1. 问题定义

### 1.1 传统前缀复用假设在 agentic 负载下失效

KV-Cache 复用的前提是请求间前缀逐字节相同。多轮 chat 场景下上下文单调追加，该假设天然成立；
但 agent 框架（Claude Code、OpenCode、SWE-agent 等）为控制上下文长度，**每轮都会主动修改历史**：

| Harness | 编辑行为 | 切口位置 |
| --- | --- | --- |
| OpenClaw | 剥离旧轮次的 thinking 块 | `│ 起始处 |
| SWE-agent | 省略除最近 n 条外的观察 | 观察块起始处 |

由于 KV 按位置敏感（每个位置的 KV 是其全部前缀的函数），**第一个被改动 token 之后的所有缓存全部失效**。
实测形态：一轮编辑后，文本相似度仍有 90%+ 的两个请求，可复用前缀可能从数万 token 塌缩到数千 token。

### 1.2 关键观察

Agent 框架的编辑是**结构化的**：切口永远落在特殊 token 标定的语义块边界上（thinking 块、tool call、
工具输出、对话轮次），不存在"修改某段 thinking 中间第 37 个 token"的编辑形态。
因此：

> **若两次请求的公共前缀恰好终止于一个语义块边界，则该边界之前的匹配在下轮编辑后大概率原样存活；
> 若公共前缀断在某个块中间（如 tool output 中段），则下轮大概率从此处塌缩。**

本设计将该观察转化为网关控制面的调度信号：匹配深度相同的情况下，优先选择"断点落在语义边界"的
路由决策，并对匹配本身的长期价值做存活折扣。

## 2. 目标与非目标

### 2.1 目标

1. 在分词阶段识别请求 token 流中的语义锚点位置，零额外引擎依赖；
2. 将锚点信息纳入 Tier-1 精确 KV 匹配的排序打分，不改变四级降级链的结构语义；
3. 通过响应头与指标暴露锚点命中情况，供上层观测与调优；
4. 为混合架构（GDN / linear attention）模型的匹配置信度折扣预留接口。

### 2.2 非目标（边界原则）

- **不在网关侧保存或恢复任何 KV Tensor / recurrent state**。快照式状态检查点是引擎层职责
  （参考 FreeToken 的 semantic-aware state cache）；Cortex 只做元数据与控制流。
- 不改变账本一致性语义：锚点只影响**同层级候选间的排序**，不影响 Worker 是否可参与精确路由的判定。
- 不做引擎内专家缓存、显存弹性划分等执行层优化。

## 3. 锚点识别

### 3.1 锚点集合的构建（静态，启动时）

```text
anchor_token_ids =
    tokenizer.json added_tokens 中的结构性控制符
  ∪ chat template 渲染产生的分段控制符（<|im_start|>/<|im_end|>/ 等）
  ∪ 配置文件追加的 harness 私有标记（如 <tool_output> 包裹符）
```

判定标准唯一：**该 token 是否标定一个"整块替换/删除操作的可能切口"**。
锚点集合必须随 `config_fingerprint`（见 `tokenizer-hash-alignment.md` §版本指纹）一并纳入哈希对齐校验。

### 3.2 运行时标注（分词管线内联完成）

扩展 `TokenizationOutput`，在既有零分配管线上顺带产出页级锚点标志，不引入第二遍扫描：

```rust
pub struct TokenizationOutput {
    pub token_ids: Arc<Vec<u32>>,
    pub page_hashes: Arc<Vec<i64>>,
    /// 新增：page_is_anchor[i] == true 表示第 i 页的末尾 token 属于锚点集合，
    /// 即"第 i+1 页起点是一个合法的恢复/切口边界"
    pub page_is_anchor: Arc<Vec<bool>>,
}
```

LRU 缓存 key 无需变更（锚点标志由 model_id + text + page_size 唯一决定，与哈希同源）。

## 4. 调度打分

### 4.1 存活因子

对 Tier-1 精确匹配返回的 `matched_pages = m`，查询第 m 页（最后一个命中页）的锚点标志：

```text
σ(m) = σ_anchor   若 page_is_anchor[m-1] == true   （断点即语义边界）
     = σ_plain    否则                              （断点在块中间）
```

默认值：`σ_anchor = 1.0`，`σ_plain = 0.6`。两值均可配置；`σ_plain = 1.0` 可整体退化为现行行为。

### 4.2 打分公式（替换 Tier-1 内部排序键）

```text
effective_pages(w) = matched_pages(w) × σ(matched_pages(w))
score(w)           = kv_weight × effective_pages(w) − load_weight × active_requests(w)
```

约束：

- 该公式仅用于 **READY 且均有精确命中的候选之间的排序**；无命中候选仍走 P2C/负载感知降级链，
  降级链结构与触发条件不变；
- 高水位过载规避检查（`max_active_requests_per_worker`）保持在打分之前执行，语义不变。

### 4.3 示例

Worker A 命中 400 页但断在 tool output 中段（σ=0.6 → 有效 240 页）；
Worker B 命中 360 页且断点为 `│ 边界（σ=1.0 → 有效 360 页）。
现行公式选 A，本设计选 B——B 的匹配在下一轮编辑后更可能继续成立。

## 5. 可观测性

### 5.1 响应头

| Header | 含义 |
| --- | --- |
| `x-cortex-match-mode` | 维持现状（`exact_kv_events` 等），不受本设计影响 |
| `x-cortex-cache-hit-tokens` | 维持现状：`matched_pages × page_size` |
| `x-cortex-anchor-aligned`（新增） | `true`/`false`：本次精确命中的断点是否落在语义边界 |

### 5.2 指标

```text
cortex_scheduler_anchor_aligned_total      断点为锚点的精确路由次数
cortex_scheduler_exact_total               全部精确路由次数
cortex_scheduler_anchor_survival_estimate  滚动窗口内的加权存活因子均值
```

Admin 集群总览可增加"锚点命中率"面板，用于评估不同 agent 流量的前缀稳定性。

## 6. 配置

```yaml
scheduler:
  # ... 既有字段 ...
  anchor_routing:
    enabled: true          # 总开关，false 时完全等价于现行行为
    sigma_anchor: 1.0
    sigma_plain: 0.6

anchors:
  # 追加 harness 私有标记（tokenizer 词表之外的包裹符/分隔符）
  extra_tokens:
    - "<tool_output>"
    - "</tool_output>"
```

## 7. 与混合架构模型的衔接（Phase 2 接口）

对采用 recurrent 层（GDN / linear attention / sliding-window hybrid）的模型，
full-attention 层的 KV 命中不代表整模型可复用——recurrent state 无法部分恢复。
预留以下折扣接口，待双引擎隔离账本落地时启用：

```text
σ_engine(engine_type) ≤ 1.0
最终 σ = σ(page) × σ_engine(model.engine)
```

引擎侧存在已确认的状态检查点时（未来通过引擎遥测暴露），可将 `σ_engine` 上调至 1.0。

## 8. 实施分期

| 阶段 | 内容 | 依赖 |
| --- | --- | --- |
| M1 | 锚点集合构建 + `TokenizationOutput.page_is_anchor` + Tier-1 打分改造 + 响应头 | 无（纯控制面） |
| M2 | Admin 面板与指标；harness 私有标记配置通道热更新 | M1 |
| M3 | `σ_engine` 引擎置信度折扣；与 PD 角色路由联动 | 双引擎隔离账本 |

## 9. 测试要求

遵循测试驱动与 Golden Fixture 原则：

1. **锚点识别单测**：基于真实 Worker 录制的 token 流 Fixture，验证 Qwen 系模板下
   `<|im_end|>`/`│`/工具调用边界的标注正确性，拒绝臆想词表规则；
2. **打分排序单测**：构造 §4.3 场景（深而脆 vs 浅而稳），断言排序翻转行为及
   `enabled=false` 时的退化等价性；
3. **端到端验证**：扩展 `tests/verify_kv_awareness.py`，增加
   "同前缀 + 不同 user 后缀"多轮序列，断言 `x-cortex-anchor-aligned` 行为符合预期；
4. **回归护栏**：`sigma_plain = 1.0 && enabled = false` 时，所有既有路由结果逐位一致。
