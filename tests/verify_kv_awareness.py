#!/usr/bin/env python3
"""Cortex 真 KV-Cache 感知调度端到端验证 (RTX PRO 6000 实机)。

验证目标：
  V1. 冷启动请求经 Cortex 反代成功，账本吸收 BlockStored 事件
  V2. 相同前缀二次请求被精确路由到持有缓存的 Worker (exact kv match)
      且响应头报告 cache hit tokens > 0
  V3. 命中缓存后 TTFT 显著低于冷启动
  V4. 流式 (SSE) 链路正常
"""
import json
import time
import urllib.request

CORTEX = "http://127.0.0.1:9000"
MODEL = "/model/Qwen1.5-MoE-A2.7B-Chat"

# 长共享前缀 (~600+ token)，确保跨多个 page_size=16 的块
SHARED_PREFIX = (
    "You are Cortex-AI, an expert in high-performance GPU computing, CUDA kernels, "
    "Blackwell SM120 microarchitecture, NVLink topology, and distributed KV-cache systems. "
    "Below is reference material you must internalize before answering any question.\n\n"
    "REFERENCE MATERIAL:\n"
    "The NVIDIA RTX PRO 6000 Blackwell Server Edition features 96GB of GDDR7 ECC memory "
    "with a peak bandwidth of 1.6 TB/s across a 512-bit bus, driven by 24 Gbps memory "
    "modules. It contains 24064 CUDA cores organized into 188 SMs, with a boost clock of "
    "approximately 2.62 GHz delivering roughly 125 FP32 TFLOPS. Tensor core throughput "
    "reaches 503 dense FP16 TFLOPS with FP8 accumulation doubling effective rates. "
    "Fourth-generation NVLINK provides 144 GB/s bidirectional per-GPU bandwidth for "
    "multi-GPU tensor parallelism. The card supports MIG partitioning into up to 4 "
    "isolated instances, PCIe Gen5 x16 host connectivity at 64 GB/s, and a 600W power "
    "envelope. Decode-phase attention is memory-bandwidth-bound: reading the full KV "
    "cache of a 32K-token sequence at FP8 precision dominates per-token latency.\n\n"
    "KV-cache-aware scheduling means the gateway tracks which physical GPU holds cached "
    "prefix blocks, routes repeat-prefix requests to those GPUs, and thereby skips most "
    "of the prefill compute. This is the fundamental economic advantage of radix-tree "
    "ledger based routing over naive round-robin load balancing.\n\n"
)

results = []


def chat(question, stream=False, timeout=60):
    payload = {
        "model": MODEL,
        "messages": [
            {"role": "system", "content": SHARED_PREFIX},
            {"role": "user", "content": question},
        ],
        "max_tokens": 24,
        "temperature": 0.0,
        "stream": stream,
    }
    req = urllib.request.Request(
        f"{CORTEX}/v1/chat/completions",
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
    )
    t0 = time.perf_counter()
    ttft = None
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        hdrs = {k.lower(): v for k, v in resp.headers.items()}
        if not stream:
            body = json.loads(resp.read().decode())
            elapsed = time.perf_counter() - t0
            text = body["choices"][0]["message"]["content"]
            return {"latency": elapsed, "ttft": elapsed, "headers": hdrs, "text": text}
        chunks = []
        for raw in resp:
            line = raw.decode().strip()
            if line.startswith("data: ") and line != "data: [DONE]":
                if ttft is None:
                    ttft = time.perf_counter() - t0
                try:
                    d = json.loads(line[6:])
                    chunks.append(d["choices"][0]["delta"].get("content", "") or "")
                except Exception:
                    pass
        return {"latency": time.perf_counter() - t0, "ttft": ttft,
                "headers": hdrs, "text": "".join(chunks)}


def status():
    with urllib.request.urlopen(f"{CORTEX}/api/v1/cluster/status", timeout=5) as r:
        return json.load(r)


def record(name, ok, detail):
    results.append((name, ok, detail))
    print(f"  [{'PASS' if ok else 'FAIL'}] {name}: {detail}")


print("=" * 70)
print(" CORTEX REAL-KV-CACHE AWARE ROUTING VERIFICATION (RTX PRO 6000)")
print("=" * 70)

s0 = status()
blocks_before = s0.get("total_cached_blocks", 0)
print(f"\n[Phase 1] Cold request (unique long prefix) ...")
r1 = chat("In one sentence, what determines decode latency?")
w1 = r1["headers"].get("x-cortex-assigned-worker", "?")
mode1 = r1["headers"].get("x-cortex-match-mode", "?")
hit1 = int(r1["headers"].get("x-cortex-cache-hit-tokens", "0") or 0)
record("V1a cold request proxied", bool(r1["text"]), f"worker={w1} mode={mode1} "
       f"hit_tokens={hit1} e2e={r1['latency']*1000:.0f}ms")

print("\n[Phase 2] Waiting for ZMQ BlockStored ingestion ...")
deadline = time.time() + 30
while time.time() < deadline:
    s1 = status()
    if s1.get("total_cached_blocks", 0) > blocks_before + 20:
        break
    time.sleep(1)
record("V1b ledger ingested KV events",
       s1.get("total_cached_blocks", 0) > blocks_before + 20,
       f"cached_blocks {blocks_before} -> {s1.get('total_cached_blocks')}")
for w in s1["workers"]:
    print(f"        {w['id']}: status={w['status']} last_seq={w['last_seq']}")

print("\n[Phase 3] Repeat request with IDENTICAL prefix (KV-aware routing test)...")
time.sleep(1)
r2 = chat("And what about prefill latency? One sentence.")
w2 = r2["headers"].get("x-cortex-assigned-worker", "?")
mode2 = r2["headers"].get("x-cortex-match-mode", "?")
hit2 = int(r2["headers"].get("x-cortex-cache-hit-tokens", "0") or 0)
record("V2a routed to cache-holding worker", w2 == w1,
       f"first={w1} second={w2} mode={mode2}")
record("V2b gateway reported prefix cache hit", hit2 >= 300,
       f"cache_hit_tokens={hit2} (expect >=300)")
speedup = (r1["latency"] / r2["latency"]) if r2["latency"] else 0
record("V3 warm faster than cold", r2["latency"] < r1["latency"],
       f"cold={r1['latency']*1000:.1f}ms warm={r2['latency']*1000:.1f}ms speedup={speedup:.2f}x")

print("\n[Phase 4] Third repeat + streaming TTFT measurement ...")
r3 = chat("Summarize the reference material in five words.", stream=True)
w3 = r3["headers"].get("x-cortex-assigned-worker", "?")
hit3 = int(r3["headers"].get("x-cortex-cache-hit-tokens", "0") or 0)
mode3 = r3["headers"].get("x-cortex-match-mode", "?")
record("V4a streaming works & hits cache", bool(r3["text"]) and hit3 >= 300,
       f"worker={w3} mode={mode3} hit_tokens={hit3} TTFT={r3['ttft']*1000:.1f}ms "
       f"e2e={r3['latency']*1000:.1f}ms out='{r3['text'][:60]}'")

print("\n" + "=" * 70)
passed = sum(1 for _, ok, _ in results if ok)
print(f" RESULT: {passed}/{len(results)} checks passed")
for name, ok, d in results:
    print(f"   {'PASS' if ok else 'FAIL'}  {name}")
print("=" * 70)
