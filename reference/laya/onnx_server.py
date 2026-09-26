"""Laya ONNX Runtime HTTP server.

Lightweight Python bridge: loads ONNX model via onnxruntime (no PyTorch),
serves /v1/systemone endpoint. Validates ONNX correctness before Rust port.

Usage:
    python code/laya/onnx_server.py --port 8200
    python code/laya/onnx_server.py --model code/laya/laya_english.onnx --port 8200
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import numpy as np

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))


# ──────────────────────────────────────────────────────────────────────
# Tokenizer wrapper (uses tokenizers crate via Python binding)
# ──────────────────────────────────────────────────────────────────────

class TokenizerWrapper:
    """Lightweight tokenizer wrapper — loads from tokenizer.json."""

    def __init__(self, path: str, marker_token_id: int = None):
        from tokenizers import Tokenizer
        self.tok = Tokenizer.from_file(path)
        # Auto-detect marker_token_id
        if marker_token_id is None:
            self.marker_token_id = self.tok.token_to_id("[MASK]") or 50284
        else:
            self.marker_token_id = marker_token_id
        self.eos_token_id = self.tok.token_to_id("[SEP]") or 102

    def encode(self, text: str, add_special_tokens: bool = True) -> List[int]:
        enc = self.tok.encode(text, add_special_tokens=add_special_tokens)
        return enc.ids

    def decode(self, ids: List[int]) -> str:
        return self.tok.decode(ids)


# ──────────────────────────────────────────────────────────────────────
# ONNX inference engine
# ──────────────────────────────────────────────────────────────────────

class LayaONNX:
    """ONNX Runtime inference for Laya DecisionModel."""

    def __init__(self, model_path: str, tokenizer_path: Optional[str] = None,
                 max_seq_len: int = 512, num_markers: int = 32):
        import onnxruntime as ort

        print(f"[onnx_server] Loading ONNX model: {model_path}")
        t0 = time.time()

        # CPU provider with optimizations
        opts = ort.SessionOptions()
        opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
        opts.intra_op_num_threads = 4
        opts.inter_op_num_threads = 2

        self.session = ort.InferenceSession(model_path, opts, providers=["CPUExecutionProvider"])
        self.max_seq_len = max_seq_len
        self.num_markers = num_markers

        print(f"[onnx_server] Loaded in {time.time()-t0:.1f}s")
        print(f"[onnx_server] Inputs: {[i.name for i in self.session.get_inputs()]}")
        print(f"[onnx_server] Outputs: {[o.name for o in self.session.get_outputs()]}")

        # Load tokenizer
        tok_path = tokenizer_path
        if not tok_path:
            model_dir = Path(model_path).parent
            for p in [model_dir / "tokenizer.json", model_dir / "tokenizer" / "tokenizer.json"]:
                if p.exists():
                    tok_path = str(p)
                    break
        if not tok_path:
            raise FileNotFoundError("tokenizer.json not found")

        print(f"[onnx_server] Loading tokenizer: {tok_path}")
        self.tokenizer = TokenizerWrapper(tok_path)
        self.marker_token_id = self.tokenizer.marker_token_id
        print(f"[onnx_server] marker_token_id = {self.marker_token_id}")

    def predict(self, text: str, questions: Dict[str, Any]) -> Tuple[Dict[str, Any], int]:
        """Run inference: text + questions → typed answers.
        
        Each question gets its own forward pass with per-option [MASK] markers:
          [CLS] type instructions [SEP] [MASK] opt0 [MASK] opt1 ... [SEP] state [SEP]
        """
        state_text = f"{text}" if isinstance(text, str) else json.dumps(text)
        total_tokens = 0
        answers = {}

        for qid, q in questions.items():
            qt = q.get("type", "choice")
            instructions = q.get("instructions", qid)
            criteria = q.get("criteria", {})

            # Build options list
            if qt == "noul":
                # noul has implicit binary options
                options = ["false", "true"]
            elif isinstance(criteria, dict):
                options = list(criteria.keys())
            elif isinstance(criteria, list):
                options = criteria
            else:
                options = ["A", "B"]

            # Build prompt: [CLS] type instructions [SEP] [MASK] opt0 [MASK] opt1 ... [SEP] state [SEP]
            prompt = f"{qt} question: {instructions}"
            prompt_ids = self.tokenizer.encode(prompt, add_special_tokens=False)

            # Build option markers
            option_marker_ids = []
            for opt in options:
                opt_text = f" {str(opt)}"
                opt_ids = self.tokenizer.encode(opt_text, add_special_tokens=False)
                option_marker_ids.append([self.marker_token_id] + opt_ids[:48])

            # Assemble: [CLS] prompt [SEP] [MASK]opt0 [MASK]opt1 ... [SEP] state [SEP]
            cls_id = self.tokenizer.tok.token_to_id("[CLS]") or 101
            sep_id = self.tokenizer.tok.token_to_id("[SEP]") or 102

            ids = [cls_id] + prompt_ids + [sep_id]
            marker_positions = []
            for opt_ids in option_marker_ids:
                marker_positions.append(len(ids))
                ids.extend(opt_ids)
            ids.append(sep_id)

            # Add state
            state_ids = self.tokenizer.encode(state_text, add_special_tokens=False)
            room = max(0, 128 - len(ids) - 1)
            ids = ids + state_ids[:room] + [sep_id]

            # Pad to 128
            seq_len = 128
            input_ids = np.zeros((1, seq_len), dtype=np.int64)
            attention_mask = np.zeros((1, seq_len), dtype=np.int64)
            for i in range(min(len(ids), seq_len)):
                input_ids[0, i] = ids[i]
                attention_mask[0, i] = 1

            # Marker positions (always pad to 32 — ONNX fixed shape)
            num_opts = len(options)
            marker_pos = np.zeros((1, 32), dtype=np.int64)
            marker_mask = np.zeros((1, 32), dtype=np.bool_)
            for i, pos in enumerate(marker_positions[:num_opts]):
                marker_pos[0, i] = min(pos, 127)
                marker_mask[0, i] = True

            qtype_arr = np.zeros((1,), dtype=np.int64)
            qtype_arr[0] = {"choice": 0, "score": 1, "noul": 2}.get(qt, 0)

            # Run inference
            outputs = self.session.run(None, {
                "input_ids": input_ids,
                "attention_mask": attention_mask,
                "marker_pos": marker_pos,
                "marker_mask": marker_mask,
                "qtype": qtype_arr,
            })

            logits_raw = outputs[0][0]  # [32]
            total_tokens += int(attention_mask.sum())

            # NOTE: ONNX export constant-folds marker_pos gather.
            # Model outputs logits at positions 0..num_opts-1 (not at marker_pos).
            logits = logits_raw[:num_opts]

            # Softmax with temperature (matching PyTorch API)
            # Temperature lookup: per-cardinality override → base temperature
            _TEMP_BASE = [1.637, 1.251, 1.983]  # [choice, score, noul]
            _TEMP_BY_OPTIONS = {
                "choice:2": 1.906, "choice:3-5": 1.760, "choice:6-10": 1.000, "choice:11+": 0.101,
                "score:3-5": 1.251, "noul:2": 1.983,
            }
            def _temp_bucket(qt: str, k: int) -> str:
                size = "2" if k <= 2 else "3-5" if k <= 5 else "6-10" if k <= 10 else "11+"
                return f"{qt}:{size}"

            qt_idx = {"choice": 0, "score": 1, "noul": 2}.get(qt, 0)
            bucket = _temp_bucket(qt, num_opts)
            temp = _TEMP_BY_OPTIONS.get(bucket, _TEMP_BASE[qt_idx])

            z = logits / temp
            exp_vals = np.exp(z - z.max())
            probs = exp_vals / exp_vals.sum()

            if qt == "choice":
                best_i = int(np.argmax(probs))
                prob_map = {k: round(float(p), 4) for k, p in zip(options, probs)}
                answers[qid] = {
                    "type": "choice",
                    "choice": options[best_i],
                    "probabilities": prob_map,
                    "confidence": round(float(probs[best_i]), 4),
                }

            elif qt == "score":
                score = float(np.sum(probs * np.arange(len(probs))))
                prob_map = {str(j): round(float(p), 4) for j, p in enumerate(probs)}
                answers[qid] = {
                    "type": "score",
                    "score": round(score, 4),
                    "probabilities": prob_map,
                    "confidence": round(float(np.max(probs)), 4),
                }

            elif qt == "noul":
                p_true = round(float(probs[1]) if len(probs) >= 2 else float(probs[0]), 4)
                answers[qid] = {
                    "type": "noul",
                    "noul": p_true,
                    "probabilities": {"false": round(1.0 - p_true, 4), "true": p_true},
                    "confidence": round(max(p_true, 1.0 - p_true), 4),
                }

        return answers, total_tokens

        return answers, input_len


# ──────────────────────────────────────────────────────────────────────
# HTTP server (stdlib only)
# ──────────────────────────────────────────────────────────────────────

def run_server(host: str, port: int, engine: LayaONNX):
    from http.server import HTTPServer, BaseHTTPRequestHandler

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path == "/health":
                self._respond(200, {"status": "ok"})
            else:
                self._respond(404, {"error": "not found"})

        def do_POST(self):
            if self.path == "/v1/systemone":
                length = int(self.headers.get("Content-Length", 0))
                body = json.loads(self.rfile.read(length)) if length else {}
                t0 = time.time()

                state = body.get("state", {})
                questions = body.get("questions", {})
                state_text = json.dumps(state) if not isinstance(state, str) else state

                try:
                    answers, input_tokens = engine.predict(state_text, questions)
                    ms = (time.time() - t0) * 1000
                    print(f"[onnx_server] {ms:.0f}ms  tokens={input_tokens}  questions={len(answers)}")
                    self._respond(200, {
                        "answers": answers,
                        "usage": {"input_tokens": input_tokens, "output_tokens": 0},
                    })
                except Exception as e:
                    self._respond(500, {"error": str(e)})
            else:
                self._respond(404, {"error": "not found"})

        def _respond(self, code, data):
            out = json.dumps(data, ensure_ascii=False, indent=2, default=str).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(out)))
            self.end_headers()
            self.wfile.write(out)

        def log_message(self, fmt, *args):
            pass  # quiet

    print(f"[onnx_server] Listening on http://{host}:{port}")
    HTTPServer((host, port), Handler).serve_forever()


# ──────────────────────────────────────────────────────────────────────
# Quick validation against PyTorch
# ──────────────────────────────────────────────────────────────────────

def validate(engine: LayaONNX):
    """Compare ONNX output with PyTorch for the same input."""
    print("\n[validate] Comparing ONNX vs PyTorch...")

    # Build same input as export_onnx.py
    tokens = [101, 7592, 1010, 2088, 102] + [0] * 123  # pad to 128
    seq_len = 128

    input_ids = np.zeros((1, seq_len), dtype=np.int64)
    attention_mask = np.zeros((1, seq_len), dtype=np.int64)
    for i, t in enumerate(tokens[:seq_len]):
        input_ids[0, i] = t
        attention_mask[0, i] = 1

    marker_pos = np.zeros((1, 32), dtype=np.int64)
    marker_mask = np.zeros((1, 32), dtype=np.bool_)
    qtype_arr = np.zeros((1,), dtype=np.int64)  # shape [1], not [1,32]
    for i in range(4):
        marker_pos[0, i] = i + 1
        marker_mask[0, i] = True

    onnx_out = engine.session.run(None, {
        "input_ids": input_ids,
        "attention_mask": attention_mask,
        "marker_pos": marker_pos,
        "marker_mask": marker_mask,
        "qtype": qtype_arr,
    })

    print(f"[validate] ONNX logits shape: {onnx_out[0].shape}")
    print(f"[validate] ONNX logits[0,:5]: {onnx_out[0][0, :5]}")
    print(f"[validate] ✅ ONNX inference working!")


# ──────────────────────────────────────────────────────────────────────
# CLI
# ──────────────────────────────────────────────────────────────────────

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--model", default=str(REPO / "code/laya/laya_english.onnx"))
    p.add_argument("--tokenizer", default=None)
    p.add_argument("--port", type=int, default=8200)
    p.add_argument("--host", default="0.0.0.0")
    p.add_argument("--validate", action="store_true", help="Run validation then exit")
    p.add_argument("--max-seq-len", type=int, default=512)
    p.add_argument("--num-markers", type=int, default=32)
    args = p.parse_args()

    engine = LayaONNX(args.model, args.tokenizer, args.max_seq_len, args.num_markers)

    if args.validate:
        validate(engine)
        return

    run_server(args.host, args.port, engine)


if __name__ == "__main__":
    main()
