"""Vendored, local-only MLX runtime for Laya typed decision models.

Derived from `mizorewww/laya-mlx` (Apache-2.0); see NOTICE and LICENSE in this
directory for attribution. Trimmed for in-repo, offline use: no Hugging Face
download, no prefix cache, no demo/language extras.

Public API::

    import laya_mlx_native as laya
    agent = laya.load("/path/to/laya-mlx-checkpoint", device="gpu")
    result = agent.system_one(state, questions)
"""

from .model import DecisionModel, EncoderConfig, sanitize_weights
from .runtime import Agent, RLAgent, collate_items, load, resolve_model
from .tokenizer import Tokenizer

__all__ = [
    "Agent",
    "RLAgent",
    "DecisionModel",
    "EncoderConfig",
    "Tokenizer",
    "collate_items",
    "load",
    "resolve_model",
    "sanitize_weights",
]

__version__ = "0.2.0+vendored"
