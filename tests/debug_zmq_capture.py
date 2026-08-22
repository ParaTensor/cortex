#!/usr/bin/env python3
"""Capture real BlockStored events from SGLang and cross-check hashes
against an independent reimplementation of the documented chain-hash."""
import json
import struct
import threading
import time
import urllib.request
import hashlib

import zmq

PORT = 5558  # worker-02, idle
PROMPT_TEXT = (
    "You are Cortex-AI, an expert in high-performance GPU computing.\n"
    "The NVIDIA RTX PRO 6000 Blackwell features 96GB GDDR7 memory and "
    "1.6 TB/s bandwidth across 188 SMs and 24064 CUDA cores. "
    "Decode attention is memory-bandwidth-bound. " * 6
)

events = []

def sub_loop():
    ctx = zmq.Context()
    sock = ctx.socket(zmq.SUB)
    sock.connect(f"tcp://127.0.0.1:{PORT}")
    sock.setsockopt(zmq.SUBSCRIBE, b"")
    poller = zmq.Poller()
    poller.register(sock, zmq.POLLIN)
    deadline = time.time() + 25
    while time.time() < deadline:
        socks = dict(poller.poll(timeout=500))
        if sock in socks:
            parts = sock.recv_multipart()
            topic = parts[0].decode(errors="replace") if len(parts) > 1 else ""
            raw = parts[-1]
            try:
                import msgpack
                d = msgpack.unpackb(raw, raw=False)
            except Exception:
                try:
                    d = json.loads(raw.decode())
                except Exception:
                    d = {"raw_len": len(raw)}
            events.append((topic, d))

t = threading.Thread(target=sub_loop, daemon=True)
t.start()
time.sleep(1)

# Send direct request to worker so SGLang allocates blocks & emits events
payload = {
    "model": "/model/Qwen1.5-MoE-A2.7B-Chat",
    "prompt": PROMPT_TEXT,
    "max_tokens": 4,
    "temperature": 0.0,
}
req = urllib.request.Request("http://127.0.0.1:8002/v1/completions",
                             data=json.dumps(payload).encode(),
                             headers={"Content-Type": "application/json"})
t0 = time.time()
with urllib.request.urlopen(req, timeout=30) as r:
    body = json.loads(r.read().decode())
print(f"direct completion done in {(time.time()-t0)*1000:.0f}ms, "
      f"prompt_tokens={body['usage']['prompt_tokens']}")
print(f"prompt_token_ids[:40]={body.get('prompt_token_ids', [])[:40]}")

t.join(timeout=30)

print(f"\ncaptured {len(events)} messages")
stored = []
for topic, d in events:
    if isinstance(d, dict):
        batch = d.get("batch") or []
        evs = [e if isinstance(e, dict) else None for e in ([d] if not batch else batch)]
        for e in evs:
            if e and str(e.get("type", "")).endswith("Stored"):
                stored.append(e)

if not stored:
    # dump structure of first message for inspection
    if events:
        print("first msg:", json.dumps(events[0][1], default=str)[:2000])
else:
    print(f"{len(stored)} BlockStored events")
    for e in stored[:3]:
        print(json.dumps(e, default=str)[:400])

# independent chain-hash of full prompt token ids (page_size=16)
tok_ids = body.get("prompt_token_ids")
if tok_ids and stored:
    ps = 16
    def chain(tokens):
        hs, prev = [], b""
        for i in range(len(tokens) // ps):
            h = hashlib.sha256(prev)
            for tok in tokens[i*ps:(i+1)*ps]:
                h.update(struct.pack("<I", tok))
            prev = h.digest()
            hs.append(int.from_bytes(prev[:8], "big", signed=True))
        return hs
    mine = chain(tok_ids)
    ev_hashes = [h for e in stored for h in e.get("block_hashes", [])]
    print("\nmy chain hashes[:5] :", mine[:5])
    print("event block hashes   :", ev_hashes[:5])
    common = set(mine) & set(ev_hashes)
    print(f"overlap: {len(common)}/{len(mine)}")
