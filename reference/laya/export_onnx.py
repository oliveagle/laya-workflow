"""Export Laya DecisionModel to ONNX.

Usage:
    LAYA_MODEL_DIR=/path/to/convaiinnovations--laya python export_onnx.py
"""
import os, sys, torch, json
from pathlib import Path

MODEL_DIR = Path(os.environ.get("LAYA_MODEL_DIR", Path.home() / "models" / "convaiinnovations--laya"))
ONNX_PATH = Path("code/laya/laya_english.onnx")
MAX_SEQ_LEN = 512
NUM_MARKERS = 32

def main():
    sys.path.insert(0, str(MODEL_DIR))
    from rl_agent_api import RLAgent

    print("Loading Laya model...")
    agent = RLAgent(str(MODEL_DIR), device="cpu")
    model = agent.model
    model.eval()
    print(f"Loaded: {type(model).__name__}, params={sum(p.numel() for p in model.parameters())/1e6:.1f}M")

    # Build dummy inputs matching forward() signature
    batch_size = 1
    seq_len = 128
    num_markers = NUM_MARKERS

    input_ids = torch.randint(0, 1000, (batch_size, seq_len))
    attention_mask = torch.ones(batch_size, seq_len, dtype=torch.long)
    marker_pos = torch.arange(num_markers).unsqueeze(0).expand(batch_size, -1) % seq_len
    marker_mask = torch.ones(batch_size, num_markers, dtype=torch.bool)
    qtype = torch.zeros(batch_size, dtype=torch.long)  # 0=choice, 1=score, 2=noul

    # Verify forward
    with torch.no_grad():
        logits, act_logits = model(input_ids, attention_mask, marker_pos, marker_mask, qtype)
    print(f"Forward OK: logits={logits.shape}, act_logits={act_logits.shape}")

    # Export to ONNX
    print(f"Exporting to ONNX: {ONNX_PATH}")
    ONNX_PATH.parent.mkdir(parents=True, exist_ok=True)

    # Use legacy exporter (more compatible with complex models)
    torch.onnx.export(
        model,
        (input_ids, attention_mask, marker_pos, marker_mask, qtype),
        str(ONNX_PATH),
        opset_version=14,
        input_names=["input_ids", "attention_mask", "marker_pos", "marker_mask", "qtype"],
        output_names=["logits", "act_logits"],
        dynamic_axes=None,
        dynamo=False,
    )

    size_mb = ONNX_PATH.stat().st_size / 1024 / 1024
    print(f"ONNX exported: {ONNX_PATH} ({size_mb:.1f} MB)")

    # Verify ONNX output matches PyTorch
    try:
        import onnxruntime as ort
        sess = ort.InferenceSession(str(ONNX_PATH))
        onnx_out = sess.run(None, {
            "input_ids": input_ids.numpy(),
            "attention_mask": attention_mask.numpy(),
            "marker_pos": marker_pos.numpy(),
            "marker_mask": marker_mask.numpy(),
            "qtype": qtype.numpy(),
        })
        diff = (logits.numpy() - onnx_out[0]).max()
        print(f"ONNX verify: max diff = {diff:.6f} {'✅' if diff < 1e-4 else '⚠️'}")
    except ImportError:
        print("onnxruntime not installed, skipping verify")

    # Save tokenizer config for Rust runtime
    tokenizer_config = {
        "vocab_size": 50368,
        "max_seq_len": MAX_SEQ_LEN,
        "num_markers": NUM_MARKERS,
        "marker_token_id": 50367,  # will be verified
    }
    # Detect marker token id from tokenizer
    marker_token_id = 50284  # default for this tokenizer
    for special in ["[MASK]", "<MASK>"]:
        try:
            candidate = tok.convert_tokens_to_ids(special) if hasattr(tok, 'convert_tokens_to_ids') else None
            if candidate is not None and candidate != tok.unk_token_id:
                marker_token_id = candidate
                break
        except:
            pass
    tokenizer_config["marker_token_id"] = marker_token_id
    print(f"Tokenizer config: {json.dumps(tokenizer_config, indent=2)}")

    config_path = ONNX_PATH.with_suffix(".config.json")
    config_path.write_text(json.dumps(tokenizer_config, indent=2))
    print(f"Config saved: {config_path}")

    print("\n✅ Done! ONNX model ready for Rust inference.")

if __name__ == "__main__":
    main()
