import urllib.request, json, time, zmq, msgspec

print("================================================================")
print("   Cortex Live End-to-End Real Validation Benchmark")
print("================================================================")

# 1. Inspect cluster status
req = urllib.request.Request("http://117.160.123.99:9000/api/v1/cluster/status")
with urllib.request.urlopen(req, timeout=5) as resp:
    status = json.loads(resp.read().decode())
    print("[1] Cluster Status Live Query:")
    print("    Total Workers:", status["total_workers"])
    print("    Workers:", [w["id"] for w in status["workers"]])

