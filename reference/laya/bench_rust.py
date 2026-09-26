#!/usr/bin/env python3
"""Benchmark laya-rs vs Python ONNX vs PyTorch inference latency."""
import json, time, sys, subprocess, os, signal
from pathlib import Path

REPO = str(Path(__file__).resolve().parents[3])
RUST_BIN = f"{REPO}/code/laya-rs/target/release/laya-rs"
ONNX_MODEL = f"{REPO}/code/laya/laya_english.onnx"
PORT = 8210

STATE = {"subject": "Duplicate charge", "body": "We were billed twice for March. Refund please.", "from": "user@acme.com"}

QUESTIONS_1 = {"dept": {"type": "choice", "instructions": "Dept?", "criteria": {"billing": "money", "tech": "bugs", "other": "misc"}}}

QUESTIONS_3 = {
    "dept": {"type": "choice", "instructions": "Dept?", "criteria": {"billing": "money", "tech": "bugs", "other": "misc"}},
    "urg": {"type": "score", "instructions": "Urgency?", "criteria": ["low", "med", "high"]},
    "spam": {"type": "noul", "instructions": "Spam?"},
}

QUESTIONS_5 = {
    "dept": {"type": "choice", "instructions": "Dept?", "criteria": {"billing": "money", "tech": "bugs", "security": "threats", "other": "misc"}},
    "urg": {"type": "score", "instructions": "Urgency?", "criteria": ["low", "med", "high"]},
    "spam": {"type": "noul", "instructions": "Spam?"},
    "action": {"type": "choice", "instructions": "Action?", "criteria": {"refund": "process refund", "ticket": "create ticket", "ignore": "no action"}},
    "priority": {"type": "score", "instructions": "Priority?", "criteria": ["p1", "p2", "p3", "p4", "p5"]},
}


def bench_rust(n_warmup=2, n_runs=10):
    """Benchmark Rust binary via HTTP."""
    import urllib.request

    # Start server
    proc = subprocess.Popen(
        [RUST_BIN, "--model", ONNX_MODEL, "--port", str(PORT)],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    time.sleep(8)  # wait for model load

    # Warmup
    for _ in range(n_warmup):
        _rust_request(QUESTIONS_3)

    results = {}
    for name, qs in [("1q", QUESTIONS_1), ("3q", QUESTIONS_3), ("5q", QUESTIONS_5)]:
        times = []
        for _ in range(n_runs):
            t0 = time.perf_counter()
            _rust_request(qs)
            times.append((time.perf_counter() - t0) * 1000)
        times.sort()
        results[name] = {
            "median": times[len(times)//2],
            "p50": times[len(times)//2],
            "p90": times[int(len(times)*0.9)],
            "min": times[0],
            "max": times[-1],
        }

    proc.terminate()
    proc.wait()
    return results


def _rust_request(questions):
    import urllib.request
    data = json.dumps({"state": STATE, "questions": questions}).encode()
    req = urllib.request.Request(
        f"http://127.0.0.1:{PORT}/v1/systemone",
        data=data, headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.loads(resp.read())


def bench_python_onnx(n_warmup=2, n_runs=10):
    """Benchmark Python ONNX server via HTTP."""
    proc = subprocess.Popen(
        [sys.executable, f"{REPO}/code/laya/onnx_server.py", "--port", str(PORT+1)],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        env={**os.environ, "PYTHONPATH": REPO},
    )
    time.sleep(8)

    import urllib.request
    for _ in range(n_warmup):
        data = json.dumps({"state": STATE, "questions": QUESTIONS_3}).encode()
        req = urllib.request.Request(f"http://127.0.0.1:{PORT+1}/v1/systemone",
            data=data, headers={"Content-Type": "application/json"})
        urllib.request.urlopen(req, timeout=30).read()

    times = []
    for _ in range(n_runs):
        data = json.dumps({"state": STATE, "questions": QUESTIONS_3}).encode()
        t0 = time.perf_counter()
        req = urllib.request.Request(f"http://127.0.0.1:{PORT+1}/v1/systemone",
            data=data, headers={"Content-Type": "application/json"})
        urllib.request.urlopen(req, timeout=30).read()
        times.append((time.perf_counter() - t0) * 1000)

    proc.terminate()
    proc.wait()
    times.sort()
    return {"median": times[len(times)//2], "min": times[0], "max": times[-1]}


def bench_pytorch(n_runs=5):
    """Benchmark PyTorch (RLAgent) direct inference."""
    py_code = f'''
import sys, time, json
sys.path.insert(0, "{REPO}")
sys.path.insert(0, os.environ.get("LAYA_MODEL_DIR", str(Path.home() / "models" / "convaiinnovations--laya")))
from rl_agent_api import RLAgent

agent = RLAgent(os.environ.get("LAYA_MODEL_DIR", str(Path.home() / "models" / "convaiinnovations--laya")), device="cpu")
state = {json.dumps(STATE)}
questions = {json.dumps(QUESTIONS_3)}

# warmup
for _ in range(2):
    agent.system_one(state, questions)

times = []
for _ in range({n_runs}):
    t0 = time.perf_counter()
    agent.system_one(state, questions)
    times.append((time.perf_counter() - t0) * 1000)

times.sort()
print(json.dumps({{"median": times[len(times)//2], "min": times[0], "max": times[-1]}}))
'''
    result = subprocess.run(
        [sys.executable, "-c", py_code],
        capture_output=True, text=True, timeout=120,
    )
    return json.loads(result.stdout.strip())


def main():
    print("=" * 60)
    print("  Laya Inference Benchmark")
    print("=" * 60)
    print()

    # 1. Rust binary
    print("[1/3] Benchmarking Rust binary (ort 2.0 ONNX Runtime)...")
    rust_results = bench_rust(n_warmup=3, n_runs=10)
    print(f"  Rust: {json.dumps(rust_results, indent=4)}")
    print()

    # 2. Python ONNX
    print("[2/3] Benchmarking Python ONNX server...")
    py_onnx_results = bench_python_onnx(n_warmup=3, n_runs=10)
    print(f"  Python ONNX: {json.dumps(py_onnx_results, indent=4)}")
    print()

    # 3. PyTorch
    print("[3/3] Benchmarking PyTorch (RLAgent)...")
    pytorch_results = bench_pytorch(n_runs=5)
    print(f"  PyTorch: {json.dumps(pytorch_results, indent=4)}")
    print()

    # Summary
    print("=" * 60)
    print("  Summary (3 questions, median latency)")
    print("=" * 60)
    print(f"  Rust (ort ONNX):  {rust_results['3q']['median']:.0f} ms")
    print(f"  Python ONNX:      {py_onnx_results['median']:.0f} ms")
    print(f"  PyTorch:          {pytorch_results['median']:.0f} ms")
    print()
    if pytorch_results['median'] > 0:
        speedup_vs_pytorch = pytorch_results['median'] / rust_results['3q']['median']
        print(f"  Rust vs PyTorch:  {speedup_vs_pytorch:.1f}x faster")
    if py_onnx_results['median'] > 0:
        speedup_vs_python = py_onnx_results['median'] / rust_results['3q']['median']
        print(f"  Rust vs Python:   {speedup_vs_python:.1f}x faster")


if __name__ == "__main__":
    main()
