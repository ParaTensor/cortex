#!/usr/bin/env python3
"""Drive `zene acp` (JSON-RPC over stdio) against the Cortex gateway and
verify the agent↔gateway linkage end-to-end on real traffic."""
import json
import os
import subprocess
import sys
import threading
import time
import urllib.request

CORTEX = "http://127.0.0.1:9000"
MODEL = "/model/Qwen1.5-MoE-A2.7B-Chat"
WORKDIR = "/tmp/zene-e2e-workdir"

os.makedirs(WORKDIR, exist_ok=True)

env = dict(os.environ)
env.update({
    "ZENE_BASE_URL": f"{CORTEX}/v1",
    "ZENE_INFERENCE_GATEWAY_URL": CORTEX,
    "ZENE_PROVIDER": "openai",
    "ZENE_MODEL": MODEL,
    "ZENE_API_KEY": "sk-cortex-local",   # SGLang does not validate
    "RUST_LOG": "zene=warn",
})

proc = subprocess.Popen(
    ["/home/bodesi/zene/target/release/zene", "acp"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
    env=env, cwd=WORKDIR, text=True, bufsize=1,
)

pending = {}
updates = []
lock = threading.Lock()

def reader():
    for line in proc.stdout:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except Exception:
            continue
        with lock:
            if "id" in msg and ("result" in msg or "error" in msg):
                pending[msg["id"]].append(msg)
            elif msg.get("method") == "session/update":
                updates.append(msg)

threading.Thread(target=reader, daemon=True).start()

def rpc(method, params, timeout=300):
    rid = int(time.time() * 1000) % 100000000
    req = {"jsonrpc": "2.0", "id": rid, "method": method, "params": params}
    with lock:
        pending[rid] = []
    proc.stdin.write(json.dumps(req) + "\n")
    proc.stdin.flush()
    deadline = time.time() + timeout
    while time.time() < deadline:
        with lock:
            msgs = pending.get(rid, [])
            if msgs:
                return msgs[0]
        time.sleep(0.05)
    raise TimeoutError(method)

def collected_text():
    out = []
    with lock:
        for u in updates:
            upd = u.get("params", {}).get("update", {})
            if upd.get("sessionUpdate") == "agent_message_chunk":
                out.append(upd.get("content", {}).get("text", ""))
    return "".join(out)

def status_field():
    try:
        s = json.load(urllib.request.urlopen(f"{CORTEX}/api/v1/cluster/status", timeout=5))
        return s["routing_stats"], s.get("total_sessions")
    except Exception as e:
        return {"err": str(e)}, None

print("== initialize ==")
r = rpc("initialize", {"protocolVersion": 1, "clientCapabilities": {"fs": True}})
print("proto:", r["result"].get("protocolVersion"))

print("== session/new ==")
r = rpc("session/new", {"cwd": WORKDIR, "mcpServers": []})
sid = r["result"]["sessionId"]
print("sessionId:", sid)

TASKS = [
    "Create a file named kv_demo.py containing a function fib(n) that returns the nth fibonacci number using iteration. Do not run it yet.",
    "Run python3 kv_demo.py --help 2>/dev/null || echo no-cli; then append a __main__ block that prints fib(10), and run it to verify the output is 55.",
    "In one sentence, summarize what files this workspace now contains.",
]
for i, task in enumerate(TASKS, 1):
    stats_before, _ = status_field()
    updates.clear()
    t0 = time.time()
    r = rpc("session/prompt",
            {"sessionId": sid, "prompt": [{"type": "text", "text": task}]},
            timeout=600)
    dt = time.time() - t0
    stats_after, sessions = status_field()
    stop = r.get("result", {}).get("stopReason")
    text = collected_text()
    print(f"\n== turn {i} ({dt:.1f}s, stop={stop}) ==")
    print("assistant:", (text[:200] + "...") if len(text) > 200 else text or "(no text chunks)")
    if isinstance(stats_before, dict) and isinstance(stats_after, dict):
        delta = {k: stats_after[k] - stats_before.get(k, 0)
                 for k in stats_after if isinstance(stats_after.get(k), (int, float))
                 and stats_after[k] != stats_before.get(k, 0)}
        print("cortex routing delta:", delta, "| total_sessions:", sessions)

print("\n== final cluster state ==")
stats, sessions = status_field()
print(json.dumps(stats))
print("total_sessions:", sessions)
proc.terminate()
