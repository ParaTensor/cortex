# SGLang 真实集群联调与底层架构缺陷深度复盘报告

本文档记录了在 **8 卡 NVIDIA GeForce RTX 5090 (8 × 32GB VRAM)** 真实生产服务器上，部署 **SGLang 0.5.15** 双实例并与 **Cortex** 网关进行端到端全链路联调验证的全过程，深度剖析了 SGLang 官方在 KV-Cache 事件广播链路上的底层实现细节与现存缺陷。

---

## 1. 实机验证环境拓扑

* **服务器环境**：8 × NVIDIA GeForce RTX 5090 (Driver 595.84, CUDA 13.2, PyTorch 2.11, Ubuntu 22.04)
* **下游 Worker 部署**：
  * **Worker 1 (GPU 0)**: `python3 -m sglang.launch_server --model-path /home/bodesi/models/qwen/Qwen3.5-4B --host 127.0.0.1 --port 8001 --mem-fraction-static 0.75 --kv-events-config '{"publisher":"zmq","endpoint":"tcp://127.0.0.1:5557"}'`
  * **Worker 2 (GPU 1)**: `python3 -m sglang.launch_server --model-path /home/bodesi/models/qwen/Qwen3.5-4B --host 127.0.0.1 --port 8002 --mem-fraction-static 0.75 --kv-events-config '{"publisher":"zmq","endpoint":"tcp://127.0.0.1:5558"}'`
* **网关层部署**：
  * **Cortex Gateway**: 运行于 `0.0.0.0:9000`，挂载真实的 `tokenizer.json`，并通过 ZeroMQ 异步监听 `tcp://127.0.0.1:5557` 与 `tcp://127.0.0.1:5558`。

---

## 2. 验证结果与核心能力落地

### ✅ Cortex 端成功落地的生产级能力
1. **Fast Tokenizer 零等待分词**：挂载 Qwen 真实 `tokenizer.json`，在网关层 3.8 秒内完成 15 万词表与 Jinja 模板初始化，运行时毫秒级分词并计算递归 SHA-256 页哈希。
2. **OpenAI 兼容 Zero-Copy 流式反代**：支持 HTTP 与 Server-Sent Events (SSE) 流式传输，响应头自动注入 `x-cortex-assigned-worker`、`x-cortex-match-mode` 与 `x-cortex-cache-hit-tokens`。
3. **四级调度与 P2C 智能负载均衡**：在冷启动无显存命中时，自动触发 Power of Two Choices (P2C) 随机双选算法，在 Worker 1 与 Worker 2 之间实现 $O(1)$ 负载均衡。
4. **集群健康与诊断 API**：`/api/v1/cluster/status` 实时统计 Worker 状态、连接数与 Radix 树容量。

---

## 3. SGLang 官方底层架构缺陷深度剖析 (为什么普通推理未触发实时广播)

在实机联调中，我们通过对 SGLang 官方源码（`sglang.srt.managers.scheduler`、`sglang.srt.mem_cache`）的追踪，定位了导致实时 ZMQ 事件未正常吐出的 **3 个底层硬核根因**：

### 缺陷 1：主调度循环（`event_loop_overlap`）遗漏了实时事件 Flush 埋点
* **代码位置**：`python/sglang/srt/managers/scheduler.py`
* **原因分析**：
  * SGLang 内部维护了 `self.kv_events_publisher.publish_kv_events()` 方法，该方法负责调用 `self.tree_cache.take_events()` 并通过 ZMQ Publisher 推送。
  * **但官方代码仅在 `on_idle()`（完全空闲休眠阶段）调用了该方法**。
  * 当客户端持续发送推理请求时，调度器工作在重叠流水线模式（`event_loop_overlap`）中，永远不会进入 `on_idle()`，导致 Radix 树中累积的 `BlockStored` 事件队列（`self.kv_event_queue`）一直被阻塞在内存中，无法实时广播给上游网关。
* **修复验证**：在 `process_batch_result` 每批次计算结束时显式补齐 `self.kv_events_publisher.publish_kv_events()`，事件即可实时从队列中冲刷至 ZMQ。

### 缺陷 2：混合架构模型（GDN / Mamba / Linear Attention）对事件系统的兼容性断裂
* **代码位置**：`python/sglang/srt/mem_cache/mamba_radix_cache.py`
* **原因分析**：
  * 诸如 `Qwen3.5-4B` 等 GDN 架构模型使用了 `MambaRadixCache`。
  * 早期 SGLang 的 `KVCacheEventMixin` 主要是针对标准纯 Transformer 的 `RadixCache` / `UnifiedRadixCache` 设计的。
  * 在混合架构下，Mamba 状态的 Tombstone 机制与增量页哈希提取存在分支差异，普通短请求若未跨越 `page_size=16` 边界或未发生显存分配，不会向 `kv_event_queue` 记录事件。

### 缺陷 3：HiCache 原生 C++ 扩展对系统 OpenSSL 动态依赖缺失
* **代码位置**：`python/sglang/srt/mem_cache/cpp_utils/hash_binding.cpp`
* **原因分析**：
  * SGLang 在记录 `_record_store_event` 时，调用了 C++ 原生扩展 `hicache_hash_cpp` 计算 SHA-256。
  * 该扩展内部使用了 `#include <openssl/sha.h>`。如果在宿主机环境未安装 `libssl-dev`，在首次处理前缀时会导致 PyTorch JIT 编译失败抛出 `RuntimeError: Failed to load HiCache native hash extension`，进而引发 SGLang 子进程 Crash 并级联触发 `SIGQUIT`。
* **解决措施**：在服务器环境预装 `libssl-dev`，完成 `hicache_hash_cpp` 的编译与加载。

---

## 4. 架构工程建议与下阶段演进

1. **上游治理保障**：
   * 在 SGLang 官方修复 `event_loop_overlap` 的实时事件推送前，Cortex 网关的 **四级降级链（P2C + Load-Aware）** 是抵御下游引擎事件断流的核心安全阀，保证了业务流量 100% 不中断。
2. **双引擎隔离账本（Phase 2）**：
   * 鉴于 vLLM 与 SGLang 在显存管理、分页大小（vLLM 默认 16，SGLang 可变）以及哈希算法上的本质差异，Cortex 下一步将推进 vLLM 的独立 Radix 账本与 Token Trie 适配。
