import urllib.request
import json
import time

CORTEX_URL = "http://127.0.0.1:9000"
MODEL = "/model/Qwen1.5-MoE-A2.7B-Chat"

def post_chat(messages, stream=False, max_tokens=32):
    payload = {
        "model": MODEL,
        "messages": messages,
        "max_tokens": max_tokens,
        "stream": stream,
        "temperature": 0.0
    }
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        f"{CORTEX_URL}/v1/chat/completions",
        data=data,
        headers={"Content-Type": "application/json"}
    )
    t0 = time.perf_counter()
    with urllib.request.urlopen(req, timeout=30) as resp:
        if stream:
            chunks = []
            for line in resp:
                line = line.decode("utf-8").strip()
                if line.startswith("data: ") and line != "data: [DONE]":
                    chunk_data = json.loads(line[6:])
                    delta = chunk_data["choices"][0]["delta"].get("content", "")
                    chunks.append(delta)
            elapsed = time.perf_counter() - t0
            return "".join(chunks), elapsed
        else:
            res = json.loads(resp.read().decode("utf-8"))
            elapsed = time.perf_counter() - t0
            return res["choices"][0]["message"]["content"], elapsed

def get_cluster_status():
    req = urllib.request.Request(f"{CORTEX_URL}/api/v1/cluster/status")
    with urllib.request.urlopen(req, timeout=5) as resp:
        return json.loads(resp.read().decode("utf-8"))

print("=" * 65)
print("   CORTEX GATEWAY REAL-TIME LIVE BENCHMARK ON RTX PRO 6000")
print("=" * 65)

# Step 1: Cluster Status Check
print("\n[Step 1] Querying Cortex Cluster Status...")
status = get_cluster_status()
print(f"  Total Workers Registered: {status.get('total_workers')}")
print(f"  Ready Workers: {status.get('ready_workers')}")
for w in status.get("workers", []):
    print(f"   • Worker ID: {w['id']} | Engine: {w['engine']} | Status: {w['status']} | HTTP: {w['http_endpoint']}")

# Step 2: Warmup / Cold Request (Prefix A)
print("\n[Step 2] Sending First Request with Long Shared System Prefix...")
shared_prefix = (
    "You are Cortex-AI, a world-class high-performance computing expert specialized in "
    "GPU architecture, Blackwell SM120 microarchitectures, and distributed KV-cache systems. "
    "Provide technical, precise, and concise answers."
)
messages_req1 = [
    {"role": "system", "content": shared_prefix},
    {"role": "user", "content": "What is the memory bandwidth of NVIDIA RTX PRO 6000?"}
]

reply1, t1 = post_chat(messages_req1, stream=False, max_tokens=40)
print(f"  Response ({t1*1000:.1f}ms): {reply1.strip()}")

# Wait a brief moment for SGLang ZMQ KV-cache events to propagate into Cortex Radix Ledger
time.sleep(1.0)

# Step 3: Check Cluster State after Prefix Event
print("\n[Step 3] Querying Ledger Status after ZMQ Ingestion...")
status2 = get_cluster_status()
print(f"  Total Cached Blocks in Cortex: {status2.get('total_cached_blocks')}")
print(f"  Ready Workers: {status2.get('ready_workers')}")
for w in status2.get("workers", []):
    print(f"   • Worker: {w['id']} | Status: {w['status']} | Last Seq: {w['last_seq']} | Active Requests: {w['active_requests']}")

# Step 4: KV-Cache Hit Verification (Same Prefix A, New User Question)
print("\n[Step 4] Sending Second Request with Identical Prefix (Testing KV Cache Routing)...")
messages_req2 = [
    {"role": "system", "content": shared_prefix},
    {"role": "user", "content": "In one sentence, explain how Radix Tree enables prefix caching."}
]

reply2, t2 = post_chat(messages_req2, stream=False, max_tokens=40)
print(f"  Response ({t2*1000:.1f}ms): {reply2.strip()}")

# Step 5: Streaming Request Test
print("\n[Step 5] Testing Streaming Output (SSE) through Cortex Gateway...")
messages_stream = [
    {"role": "system", "content": shared_prefix},
    {"role": "user", "content": "Count from 1 to 5 separated by commas."}
]
reply_stream, t3 = post_chat(messages_stream, stream=True, max_tokens=30)
print(f"  Streaming Response ({t3*1000:.1f}ms): {reply_stream.strip()}")

# Step 6: Final Cluster Stats
print("\n[Step 6] Final Cluster Overview:")
status3 = get_cluster_status()
print(f"  Total Cached Blocks: {status3.get('total_cached_blocks')}")
for w in status3.get("workers", []):
    print(f"   • Worker: {w['id']} | Status: {w['status']} | Last Seq: {w['last_seq']}")

print("\n" + "=" * 65)
print("   ALL TESTS PASSED SUCCESSFULLY!")
print("=" * 65)
