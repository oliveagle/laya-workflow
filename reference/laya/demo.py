"""Demo: Laya non-autoregressive System 1 decision model.

Usage:
    python demo.py                           # English root checkpoint, default email example
    python demo.py --checkpoint multilingual   # 100+ language support
    python demo.py --checkpoint typed-decisions
    python demo.py --bench                   # CPU latency benchmark (1/10/50 q)
    python demo.py --device cuda             # GPU (need CUDA + free GPU)

Reference: docs/research/laya_system_one_model_research_20260924.md
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path

# Laya ships its inference API + shared model code alongside the weights.
# We point sys.path at the local model directory so we don't need pip install laya.
REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MODEL_DIR = REPO_ROOT / "models" / "convaiinnovations--laya"


def _load(checkpoint: str, model_dir: Path, device: str):
    """checkpoint in {'.', 'multilingual', 'typed-decisions'}."""
    sub = checkpoint if checkpoint != "english" else "."
    sys.path.insert(0, str(model_dir))
    from rl_agent_api import RLAgent  # noqa: E402

    t = time.time()
    agent = RLAgent(str(model_dir / sub), device=device)
    print(f"[load] checkpoint={sub} device={agent.device} dtype={agent.dtype} load_s={time.time() - t:.2f}")
    return agent


def _email_state():
    return {
        "from": "user@acme.com",
        "subject": "Duplicate charge on invoice #4411",
        "body": "Hi, we were billed twice for March. Please refund the duplicate today or we will cancel our plan.",
    }


def _email_questions():
    return {
        "department": {
            "type": "choice",
            "instructions": "Which department should handle this request?",
            "criteria": {
                "billing": "invoices, payments, refunds",
                "technical": "bugs, outages, system errors",
                "sales": "pricing, new contracts",
                "other": "everything else",
            },
        },
        "urgency": {
            "type": "score",
            "instructions": "How urgent is this request?",
            "criteria": ["not urgent", "soon", "critical deadline or blocking issue"],
        },
        "churn_risk": {"type": "noul", "instructions": "Does the user threaten to cancel or leave?"},
        "refund_requested": {"type": "noul", "instructions": "Does the user explicitly request a refund?"},
        "is_phishing": {
            "type": "noul",
            "instructions": "Is this email a phishing or scam attempt?",
            "criteria": {"true": "phishing, scam, or fraud", "false": "a legitimate email"},
        },
    }


def _multilingual_state():
    return {"document": "两因素身份验证必须在下周五前对所有用户启用，否则审计将失败。"}


def _multilingual_questions():
    return {
        "is_security": {
            "type": "choice",
            "instructions": "Which workflow should handle this?",
            "criteria": {
                "security": "security incident or policy",
                "compliance": "audit, governance, regulation",
                "operations": "everyday operations",
                "ignore": "not actionable",
            },
        },
        "needs_action": {"type": "noul", "instructions": "Is there a concrete action required?"},
    }


def _pick_state_questions(checkpoint: str):
    if checkpoint == "multilingual":
        return _multilingual_state(), _multilingual_questions()
    return _email_state(), _email_questions()


def cmd_predict(args):
    agent = _load(args.checkpoint, args.model_dir, args.device)
    state, qs = _pick_state_questions(args.checkpoint)
    t = time.time()
    res = agent.system_one(state, qs)
    ms = (time.time() - t) * 1000
    print(f"[predict] {ms:.1f} ms  input_tokens={res['usage']['input_tokens']}  output_tokens={res['usage']['output_tokens']}")
    print(json.dumps(res, ensure_ascii=False, indent=2))


def cmd_bench(args):
    agent = _load(args.checkpoint, args.model_dir, args.device)
    state, _ = _pick_state_questions(args.checkpoint)
    departments = {
        "billing": "invoices, payments, refunds",
        "technical": "bugs, outages",
        "sales": "pricing, contracts",
        "account": "login, access",
        "other": "none",
    }
    print(f"{'n':>2}  {'latency_ms':>12}  {'ms/q':>10}  input_tokens")
    for n in [1, 10, 50]:
        qs = {
            f"q{i}": {"type": "choice", "instructions": "Which department?", "criteria": departments}
            for i in range(n)
        }
        t = time.time()
        res = agent.system_one(state, qs)
        ms = (time.time() - t) * 1000
        print(f"{n:>2}  {ms:>12.1f}  {ms / n:>10.1f}  {res['usage']['input_tokens']}")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--checkpoint", choices=["english", "multilingual", "typed-decisions"], default="english")
    p.add_argument("--model-dir", type=Path, default=DEFAULT_MODEL_DIR,
                   help="Path to the convaiinnovations--laya local model directory.")
    p.add_argument("--device", choices=["cpu", "cuda"], default="cpu")
    p.add_argument("--bench", action="store_true", help="Run CPU latency benchmark instead of predict.")
    args = p.parse_args()
    if not args.model_dir.exists():
        sys.exit(f"Model dir not found: {args.model_dir}\n  Run: modelscope download --model convaiinnovations/laya --local_dir {args.model_dir}")
    if args.bench:
        cmd_bench(args)
    else:
        cmd_predict(args)


if __name__ == "__main__":
    main()
