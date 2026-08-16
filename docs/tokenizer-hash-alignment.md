# Tokenizer 与块哈希对齐规范

## 1. 安全原则

精确 KV 路由成立的前提是 Cortex 与 Worker 对同一请求产生完全一致的 Token ID、分页边界和 Block Hash。任一输入无法确认一致时，Cortex 必须关闭该请求的精确 KV 匹配并进入负载降级路径，不能用近似值推测命中。

## 2. 版本化配置单元

下列字段组成不可分割的 `HashConfig`，以规范序列化后的 SHA-256 作为 `config_fingerprint`：

```yaml
schema_version: 1
engine: sglang
engine_version: "<exact-version>"
model_id: "<deployment-model-id>"
tokenizer_digest: "sha256:<digest>"
chat_template_digest: "sha256:<digest>"
special_tokens_digest: "sha256:<digest>"
page_size: 16
hash_algorithm: "sglang_recursive_sha256_v1"
hash_seed_or_salt: null
extra_keys_schema: "none"
```

Worker 注册、快照和增量事件都必须携带该指纹。Cortex 只允许相同 `model_id`、引擎协议和 `config_fingerprint` 的请求与账本进行匹配。配置热更新应生成新的部署版本，不得在原账本上原地切换。

## 3. 规范化与 Tokenization 边界

XRouter 负责协议转换和 Prompt 规范化；Cortex 负责最终输入的 Tokenization。二者的契约是：XRouter 传入已经确定语义的 OpenAI 兼容请求，Cortex 不再次修改消息内容、工具定义、图片顺序或系统提示。

若 XRouter 传递预计算 Token ID，Cortex 只有在同时收到可验证的 `config_fingerprint` 且本地重新抽样校验通过后才能复用。否则仍以 Cortex 本地 Tokenizer 结果为准。XRouter 的 L1/L2 指纹只用于全局路由，不作为 Block Hash 输入。

以下字段必须纳入 Tokenization 输入或明确禁止：

```text
messages / prompt
tools / tool_choice
response_format
add_generation_prompt
special tokens
LoRA adapter identity
多模态内容及其稳定摘要
引擎定义的 cache salt / extra_keys
```

## 4. 引擎隔离

SGLang 与 vLLM 使用独立协议适配器、独立 `HashConfig` 和独立 Radix HashTree。禁止仅因 Token ID 相同就在引擎间共享 Block Hash。

SGLang 适配器应严格复现目标版本的递归页哈希算法。vLLM 适配器应优先消费 Worker 事件给出的 `block_hashes`，并以 `token_ids`、`extra_keys` 及其顺序验证查询侧计算。具体字节序、长度编码和递归输入必须通过对应版本的 Golden Fixture 固化，不能仅依赖文字描述。

## 5. 对齐校验

每个支持的“引擎版本 × 模型 × Tokenizer × Page Size”组合都必须有 Golden Fixture，至少覆盖：

| 场景 | 断言 |
| --- | --- |
| ASCII、中文和混合文本 | Token ID 与 Worker 完全一致 |
| Chat Template 与系统提示 | 渲染结果和 Token ID 完全一致 |
| 跨页、整页和不足一页 Prompt | 页边界及每页 Hash 完全一致 |
| Special Token | 添加规则与 Worker 一致 |
| LoRA / 多模态 / Salt | `extra_keys` 与最终 Hash 一致 |
| 空输入和超长输入 | 错误或截断语义一致 |
| Tokenizer / Template 升级 | 指纹变化，旧账本不可复用 |

集成测试应从真实 Worker 录制请求、Token ID、Block Hash 和事件样本。CI 对每个 Fixture 同时运行 Cortex 适配器；出现一个字节差异即失败。

运行时可对少量请求执行 Worker 对齐探针。若检测到 Hash 不一致，立即隔离对应 `config_fingerprint` 的全部账本，发出告警并回退到负载路由。

## 6. 失败与降级原因

以下情况不得进入 `exact_kv`：

```text
unknown_model
missing_hash_config
config_fingerprint_mismatch
tokenizer_load_failed
template_render_failed
unsupported_extra_keys
hash_probe_mismatch
ledger_not_ready
```

降级原因应写入结构化日志和指标 `cortex_route_fallback_total{reason}`。对外仅暴露稳定枚举，避免响应 Header 泄露模型文件路径或内部异常细节。

## 7. 性能目标

`< 2ms` 是待验证目标，必须注明输入长度和硬件条件，并分别测量 Template 渲染、Tokenization、分页及 Hash。Tokenizer 结果可按 `config_fingerprint + normalized_input_digest` 做有界缓存，但缓存命中不能绕过配置指纹校验；缓存应设置条目数与内存硬上限。
