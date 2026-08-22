#!/usr/bin/env python3
"""Hash-level cross-check: SGLang event block_hashes vs independent
reimplementation of cortex's tokenize_and_hash pipeline."""
import hashlib
import json
import struct
import threading
import time
import urllib.request

import msgpack
import zmq
from transformers import AutoTokenizer

MODEL = "/model/Qwen1.5-MoE-A2.7B-Chat"
PAGE = 16
import random, string
SUFFIX = "UNIQ" + "".join(random.choices(string.ascii_letters, k=24))
MESSAGES = [
    {"role": "system", "content":
        "You are a helpful assistant expert in GPUs. Reference sheet: "
        "the RTX PRO 6000 Blackwell has 96GB GDDR7, 1.6 TB/s bandwidth, "
        "188 SMs, 24064 CUDA cores, NVLink4 at 144 GB/s per GPU. "
        f"Session tag {SUFFIX}. Decode attention is bandwidth-bound."},
    {"role": "user", "content": "Explain KV cache routing in one sentence."},
]
events = []

def sub_loop():
    ctx = zmq.Context()
    sock = ctx.socket(zmq.SUB)
    sock.connect("tcp://127.0.0.1:5558")
    sock.setsockopt(zmq.SUBSCRIBE, b"")
    poller = zmq.Poller(); poller.register(sock, zmq.POLLIN)
    deadline = time.time() + 30
    while time.time() < deadline:
        if sock in dict(poller.poll(timeout=500)):
            parts = sock.recv_multipart()
            try:
                d = msgpack.unpackb(parts[-1], raw=False)
            except Exception:
                continue
            # wire format: [ts, [["BlockStored", [hash], parent, [tokens], ...], ...]]
            batch = d[1] if isinstance(d, list) and len(d) > 1 else []
            for e in batch:
                if isinstance(e, list) and e and str(e[0]) == "BlockStored":
                    events.append(e)

t = threading.Thread(target=sub_loop, daemon=True); t.start()
time.sleep(1)

payload = {"model": MODEL, "messages": MESSAGES,
           "max_tokens": 4, "temperature": 0.0}
req = urllib.request.Request(
    "http://127.0.0.1:8002/v1/chat/completions",
    data=json.dumps(payload).encode(),
    headers={"Content-Type": "application/json"})
with urllib.request.urlopen(req, timeout=60) as r:
    json.loads(r.read().decode())
print("request done")
t.join(timeout=35)

print(f"captured {len(events)} BlockStored events")
ev_hashes, ev_tokens = [], []
for e in events:
    ev_hashes.append(e[1][0])
    ev_tokens.extend(e[3])
print(f"first 3 event hashes: {ev_hashes[:3]}")
print(f"event token_ids[:20]: {ev_tokens[:20]}")

# Independent reimplementation of gateway-side hashing
tok = AutoTokenizer.from_pretrained(MODEL, trust_remote_code=True)
text = tok.apply_chat_template(MESSAGES, tokenize=False, add_generation_prompt=True)
ids = tok.encode(text)
print(f"\ntemplated text[:120]: {text[:120]!r}")
print(f"templated ids[:20] : {ids[:20]}")

def chain(tokens):
    hs, prev = [], b""
    for i in range(len(tokens) // PAGE):
        h = hashlib.sha256(prev)
        for tok_i in tokens[i*PAGE:(i+1)*PAGE]:
            h.update(struct.pack("<I", tok_i))
        prev = h.digest()
        hs.append(int.from_bytes(prev[:8], "big", signed=True))
    return hs

mine = chain(ids)
common = set(mine) & set(ev_hashes)
print(f"\nmy pages={len(mine)}, event blocks={len(ev_hashes)}, overlap={len(common)}")
if not common:
    # find first divergence point
    for i in range(min(len(mine), len(ev_hashes))):
        if mine[i] != ev_hashes[i]:
            print(f"divergence at page {i}: my tokens[{i*PAGE}:(i+1)*PAGE]="
                  f"{ids[i*PAGE:(i+1)*PAGE]} vs event tokens="
                  f"{ev_tokens[i*PAGE:(i+1)*PAGE] if len(ev_tokens) > (i+1)*PAGE else '?'}")
            break
