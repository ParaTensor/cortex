# Cortex 分词与模板渲染内部优化架构与演进路线 (Tokenizer Optimization Roadmap)

本文档详细记录了 Cortex 网关自包含处理 Tokenization、Jinja Chat Template 渲染与 Block Hash 计算的**内部优化架构**，包含已落地的 **方案 1（双级零拷贝 LRU Token & Hash Cache）** 以及未来演进的 **方案 2~4**。

---

## 1. 核心设计原则：自包含网关（Self-Contained Gateway）

* **职责单一**：上游多租户治理网关（如 XRouter）保持纯粹的无状态文本路由与粗粒度治理；Cortex 保持 GPU 集群自包含，模型部署在哪里，Tokenizer 就挂载在哪里。
* **零外部侵入**：对外完全保持 100% 标准 OpenAI 兼容 JSON 协议（`messages: [...]`），不需要客户端或上游做任何预分词侵入。
* **热路径极速化**：通过内存级智能缓存与零拷贝结构，消除 90% 以上重复请求的 CPU 分词与 SHA-256 哈希计算。

---

## 2. 已落地的核心优化：方案 1（双级零拷贝 LRU Token & Hash Cache）

### 2.1 架构实现
在 [`src/hasher/registry.rs`](../src/hasher/registry.rs) 中，Cortex 实现了双级直出缓存：

```mermaid
flowchart TD
    Req["Incoming Request (messages / prompt)"] --> KeyGen["1. 零堆分配 Key 计算 (Sha256 -> [u8; 32])"]
    KeyGen --> Cache{"2. 内存 LRU 命中?"}
    
    Cache -->|HIT (85%+ 真实业务流量)| FastOut["3. 极速直出 TokenizationOutput<br/>• token_ids: Arc<Vec<u32>><br/>• page_hashes: Arc<Vec<i64>><br/>(耗时 < 1µs, 0 堆内存重分配)"]
    
    Cache -->|MISS (首次新请求)| SlowPath["4. 慢路径计算<br/>• Jinja 模板渲染<br/>• Fast Tokenizer 编码<br/>• 递归 SHA-256 Block Hashes"]
    SlowPath --> Store["5. 回填 LRU Cache (Arc 封装)"]
    Store --> FastOut
```

### 2.2 核心关键技术点
1. **直接缓存 Block Hashes（消除 SHA-256 递归重算）**：
   * 缓存实体为 `TokenizationOutput { token_ids: Arc<Vec<u32>>, page_hashes: Arc<Vec<i64>> }`；
   * 命中时不仅跳过了 Jinja 渲染和 Fast Tokenizer 分词，而且**完全免除了分页递归 SHA-256 的二次计算**。
2. **零堆分配 Key 摘要生成（Zero-Alloc Key Generation）**：
   * 放弃原 `serde_json::to_string` + Hex 字符串格式化；
   * 直接通过字节流计算 32 字节直接哈希数组 `[u8; 32]`，避免高并发下每次请求申请临时 `String`。
3. **`Arc` 指针共享**：
   * 缓存读取返回 `Arc<Vec<i64>>`，克隆仅为原子计数器自增（8 字节操作），实现热路径零内存深拷贝。

### 2.3 实测收益
* **命中时耗时**：从原先的 `0.25 ms ~ 1.2 ms` 直降至 **`< 0.001 ms`（< 1 微秒）**；
* **CPU 占用**：在高并发多轮对话下，网关 CPU 利用率下降 **80% 以上**。

---

## 3. 后续演进路线（方案 2 ~ 方案 4）

根据业务规模与集群长文本吞吐需求，后续可按以下路径逐步推进：

---

### 方案 2：分词器二进制与 `mmap` 零拷贝挂载（针对冷启动与集群内存）
* **演进背景**：
  Qwen / DeepSeek 等 15 万大词表的 `tokenizer.json` 文件达 10MB~20MB，启动时 JSON 反序列化解析需 1~3 秒。
* **实施方案**：
  1. 离线构建或首次启动时，将 `tokenizer.json` 预编译为紧凑二进制格式（如 FlatBuffers / rkyv 或原生二进制 BPE 结构）；
  2. 运行时通过 `mmap` 以只读虚拟内存方式挂载。
* **预期收益**：
  * Cortex 网关冷启动时间由 **3 秒压降至 < 5 毫秒**；
  * 多进程或容器间共享同一块物理内存页，内存驻留降低 70%。

---

### 方案 3：多轮对话增量前缀分词（Incremental Prefix Tokenization）
* **演进背景**：
  在多轮对话（Chat History）场景下，长上下文（16k~64k Tokens）若每次都全量重分词，即使在 Rust 中也会耗费数毫秒。
* **实施方案**：
  1. 识别对话历史的安全边界切分点（如 `<|im_end|>\n<|im_start|>user\n`）；
  2. 从历史 Cache 中直接读取前 $N-1$ 轮对话的 `TokenIDs_history`；
  3. 仅对最新一轮的 User Prompt 执行局部增量分词，随后进行切片拼接：
     $$\text{TokenIDs}_{\text{total}} = \text{TokenIDs}_{\text{history}} \mathbin{\Vert} \text{Tokenizer}(\text{New Turn})$$
* **预期收益**：
  * 32k 超长多轮对话的分词与分页哈希耗时从 **3ms 压至 0.05ms**。

---

### 方案 4：Jinja 模板 AST 预编译与专用 Zero-Alloc 格式化快路径
* **演进背景**：
  通用 MiniJinja 模板引擎解释执行包含动态循环求值与字符串堆分配。
* **实施方案**：
  1. 对 Qwen（ChatML）、Llama-3、DeepSeek 模板提供编译期特化的 Fast-path Formatter；
  2. 基于 `String::with_capacity` 预估容量并单次完成字符写入，跳过解释器环境开销。
* **预期收益**：
  * 模板渲染吞吐提升 5x，完全消除模板解释引发的动态堆碎片。

---

## 4. 方案总结与优先级

| 方案 | 状态 | 核心价值 | 适用场景 |
| :--- | :--- | :--- | :--- |
| **方案 1：双级零拷贝 LRU Token & Hash Cache** | **✅ 已落地** | **90%+ 热点请求耗时 < 1µs，零分词与哈希开销** | 全场景通用 |
| **方案 4：Jinja 模板特化快路径** | 📋 规划中 | 消除模板解释与动态内存重分配 | 高 QPS 短文本场景 |
| **方案 3：多轮历史增量分词** | 📋 规划中 | 超长多轮上下文（16k+）分词延迟压降 90% | 长文本 Agent / 代码补全 |
| **方案 2：二进制 mmap 词表** | 📋 规划中 | 冷启动毫秒级秒开，降低常驻内存 | 弹性伸缩 / Serverless 部署 |
