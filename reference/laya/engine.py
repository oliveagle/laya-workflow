"""Shared Laya engine wrapper.

Loads the Laya RLAgent from the local model directory once, exposes a single
high-level `decide(state, questions, confidence_threshold)` API.

Pattern mirrors what we already built in Phase 39-40 of self_evolving
(`workflows/agent_permission.py` + CalibratedDecisionHead), but the decision
engine is a 421M neural encoder-decoder instead of compiled WASM.
"""
from __future__ import annotations

import json
import os
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, Iterable, List, Mapping, Optional, Tuple

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MODEL_DIR = REPO_ROOT / "models" / "convaiinnovations--laya"


def _resolve_model_dir() -> Path:
    """LAYA_MODEL_DIR env override, then repo-relative default, then ~/models/.."""
    env = os.environ.get("LAYA_MODEL_DIR")
    if env:
        return Path(env)
    if DEFAULT_MODEL_DIR.exists():
        return DEFAULT_MODEL_DIR
    for cand in (Path.home() / "models" / "convaiinnovations--laya",
                 Path.home() / ".cache" / "modelscope" / "models" / "convaiinnovations--laya"):
        if cand.exists():
            return cand
    return DEFAULT_MODEL_DIR


@dataclass
class Decision:
    """A single typed answer for one question."""
    answer: Any              # str for choice, float for score/noul
    probabilities: Dict[str, float]
    confidence: float
    raw: Dict[str, Any]


@dataclass
class Verdict:
    """The aggregated result from a single Laya forward pass."""
    answers: Dict[str, Decision]
    routing: Dict[str, Any]
    input_tokens: int
    output_tokens: int
    latency_ms: float

    def to_jsonable(self) -> Dict[str, Any]:
        return {
            "answers": {
                k: {"answer": v.answer, "probabilities": v.probabilities,
                    "confidence": v.confidence}
                for k, v in self.answers.items()
            },
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "latency_ms": round(self.latency_ms, 2),
        }


class LayaEngine:
    """Thin wrapper around rl_agent_api.RLAgent (English root checkpoint).

    All apps in code/laya/apps share a single instance to avoid 30s reload cost.
    """

    _shared: Optional["LayaEngine"] = None

    def __init__(self, checkpoint: str = "english", device: str = "cpu"):
        model_dir = _resolve_model_dir()
        if not model_dir.exists():
            raise FileNotFoundError(
                f"Laya model dir not found: {model_dir}\n"
                f"  Run: modelscope download --model convaiinnovations/laya --local_dir {model_dir}"
            )
        sub = "." if checkpoint == "english" else checkpoint
        sys.path.insert(0, str(model_dir))
        from rl_agent_api import RLAgent  # type: ignore  # noqa: E402
        t = time.time()
        self.agent = RLAgent(str(model_dir / sub), device=device)
        self.checkpoint = sub
        self.device = self.agent.device
        self.load_s = time.time() - t

    @classmethod
    def shared(cls) -> "LayaEngine":
        if cls._shared is None:
            cls._shared = cls()
        return cls._shared

    @classmethod
    def reset(cls) -> None:
        cls._shared = None

    def decide(self, state: Mapping[str, Any] | str,
               questions: Mapping[str, Dict[str, Any]]) -> Verdict:
        """Run a single forward pass over (state, questions); return parsed Verdict."""
        t = time.time()
        raw = self.agent.system_one(state, dict(questions))
        latency_ms = (time.time() - t) * 1000
        answers: Dict[str, Decision] = {}
        for qid, ans in raw["answers"].items():
            t = ans["type"]
            if t == "choice":
                a = ans["choice"]
                probs = ans["probabilities"]
            elif t == "score":
                a = ans["score"]
                probs = {str(k): v for k, v in ans["probabilities"].items()}
            else:  # noul
                a = ans["noul"]
                probs = {"false": 1.0 - ans["noul"], "true": ans["noul"]}
            answers[qid] = Decision(
                answer=a,
                probabilities=probs,
                confidence=ans.get("confidence", float(max(probs.values()) if probs else 0.0)),
                raw=ans,
            )
        return Verdict(
            answers=answers,
            routing={"model": "laya", "checkpoint": self.checkpoint, "device": str(self.device)},
            input_tokens=raw["usage"]["input_tokens"],
            output_tokens=raw["usage"]["output_tokens"],
            latency_ms=latency_ms,
        )


def gate_action(verdict: Verdict, key: str,
                allow_threshold: float = 0.85, block_threshold: float = 0.85) -> str:
    """Map a noul answer to a 3-way gate action (ALLOW / CONFIRM / BLOCK).

    Mirrors Phase 39-40 /gate endpoint. CONFIRM = model is uncertain; escalate.
    """
    d = verdict.answers[key]
    p_true = d.answer
    p_false = 1.0 - p_true if isinstance(p_true, float) else None
    if p_true >= allow_threshold:
        return "ALLOW"
    if (1 - p_true) >= block_threshold:
        return "BLOCK"
    return "CONFIRM"
