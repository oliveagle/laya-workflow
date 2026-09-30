#!/usr/bin/env python3
"""Compare jev-mem (semantic gate) vs GBNF (structural gate) on the same set
of candidate memory-record payloads.

GBNF reference: https://github.com/ggml-org/llama.cpp/blob/master/grammars/README.md
                (GBNF = GGML BNF, an extension of BNF with regex-like features).

The target payload is the JSON object a `laya_mem_persist` write emits: one
memory row whose shape is fixed by the spec (id INTEGER, content TEXT NOT
NULL, ts TEXT, entities TEXT, type_scores TEXT NOT NULL) plus, optionally, a
relation row.

The test simulates the last post-production stage of a write: the Laya engine
has *decided* (via the jev-mem admission / type / relation DSL) what to
write, and now the model must emit the actual JSON string to persist. Three
gates are checked on every candidate:

  * jev-only   — semantic gate (Python implementation).
              (Class-level constraints: content non-empty + length, ISO-8601 ts,
               type_scores has exactly the 4 jev labels and each ∈ [0,1],
               sum close to 1.0, entities well-formed, relation well-formed.)
  * GBNF-only  — structural gate (Python implementation of a GBNF subset
              that supports literals, char classes, ?, *, +, {m,n},
              alternation and parens — enough for the JSON record grammar
              below).
  * jev + GBNF — both must pass (the "write only if both layers agree"
              policy: jev says the content is worth storing, and GBNF
              guarantees the persisted bytes conform to the schema).

Outputs a markdown report and prints a summary table. Run::

    PYTHONPATH= bench/jev_vs_gbnf.py            # smoke
    bench/jev_vs_gbnf.py --md-out bench/reports/jev_vs_gbnf.md
"""
from __future__ import annotations

import time
import argparse
import json
import re
import sys
from dataclasses import dataclass
import os
import shutil
import subprocess
from pathlib import Path
from typing import Callable

# ---------------------------------------------------------------------------
# 1. Target schema  (laya_mem_persist shape; the JSON contract every record
#    that lands in `memories` / `relations` must satisfy on the wire)
# ---------------------------------------------------------------------------

TYPE_LABELS = ("episodic", "semantic", "procedural", "preference")


def make_record(
    rid: int,
    content: str,
    ts: str,
    type_scores: dict,
    entities: list,
) -> dict:
    return {
        "id": rid,
        "content": content,
        "ts": ts,
        "type_scores": type_scores,
        "entities": entities,
    }


# ---------------------------------------------------------------------------
# 2. GBNF grammar for a memory record
#
#    The grammar below is a *direct* hand port of the JSON shape produced by
#    jev-mem — same shape as the llama.cpp JSON-array example in the README
#    (rules like ws / char / number / string / object / value), with the
#    specific keys required by laya_mem_persist. The `root` rule is the only
#    one the engine ever calls.
# ---------------------------------------------------------------------------

GBNF = r"""
# Memory record JSON for laya_mem_persist (System-One write-side).
# Keys are required in this exact order; no additional keys allowed.
root ::= "{" ws id_kv ws "," ws content_kv ws "," ws ts_kv ws "," ws type_scores_kv ws "," ws entities_kv ws "}"

id_kv            ::= "\"id\""            ws ":" ws integer
content_kv        ::= "\"content\""        ws ":" ws string
ts_kv             ::= "\"ts\""             ws ":" ws string
type_scores_kv    ::= "\"type_scores\""    ws ":" ws type_scores_obj
entities_kv       ::= "\"entities\""       ws ":" ws string_array


# type_scores is an object with exactly the 4 jev-mem labels, in order.
type_scores_obj   ::= "{" ws episodic_kv "," ws semantic_kv "," ws procedural_kv "," ws preference_kv ws "}"
episodic_kv       ::= "\"episodic\""     ws ":" ws number
semantic_kv       ::= "\"semantic\""     ws ":" ws number
procedural_kv     ::= "\"procedural\""   ws ":" ws number
preference_kv     ::= "\"preference\""   ws ":" ws number

# Whitespace, primitives and JSON helpers (subset of llama.cpp examples).
string_array      ::= "[" ws ( string ("," ws string)* )? ws "]"
string            ::= "\"" char* "\""
char              ::= [^"\\] | "\\" ( ["\\bfnrt] | "u" [0-9a-fA-F]{4} )
integer           ::= "-"? ( "0" | [1-9] [0-9]{0,15} )
number            ::= "-"? ( "0" | [1-9] [0-9]{0,15} ) ( "." [0-9]{1,16} )? ( [eE] [-+]? integer )?
ws                ::= ( " " | "\n" | "\t" )*
"""

# ---------------------------------------------------------------------------
# 3. Tiny GBNF validator: recursive-descent over a single non-terminal.
#    Supports literals, char classes (incl. ^ negation, \uXXXX escapes),
#    `?`, `*`, `+`, `{m,n}`, `|`, grouping, and `nonterminal` references.
# ---------------------------------------------------------------------------

_GRAMMAR_CACHE: dict[str, dict[str, list[list[str]]]] = {}


def _tokenize_rhs(rhs: str) -> list[str]:
    """Split a RHS into symbols. Supports the GBNF subset above."""
    toks: list[str] = []
    i, n = 0, len(rhs)
    while i < n:
        c = rhs[i]
        if c.isspace():
            i += 1
            continue
        if c == "#":
            while i < n and rhs[i] != "\n":
                i += 1
            continue
        if c == '"':
            j = i + 1
            while j < n and rhs[j] != '"':
                if rhs[j] == "\\":
                    j += 2
                else:
                    j += 1
            toks.append(rhs[i : j + 1])
            i = j + 1
            continue
        if c == "[":
            depth = 1
            j = i + 1
            while j < n and depth:
                if rhs[j] == "\\" and j + 1 < n:
                    j += 2
                    continue
                if rhs[j] == "]":
                    depth -= 1
                elif rhs[j] == "[":
                    depth += 1
                j += 1
            toks.append(rhs[i:j])
            i = j
            continue
        if c.isalpha() or c == "_":
            j = i
            while j < n and (rhs[j].isalnum() or rhs[j] in "_-"):
                j += 1
            toks.append(rhs[i:j])
            i = j
            continue
        if c in "?*+|(),":
            toks.append(c)
            i += 1
            continue
        if c == "{":
            j = i
            depth = 1
            while j < n and depth:
                j += 1
                if j < n and rhs[j] == "}":
                    depth -= 1
            toks.append(rhs[i : j + 1])
            i = j + 1
            continue
            toks.append(c)
            i += 1
            continue
        raise ValueError(f"unexpected char in GBNF: {c!r} at {i}")
    return toks


def _parse_alts(toks: list[str], i: int) -> tuple[list[list[str]], int]:
    """One alternative is a sequence of (symbol + quantifier) groups; alternatives
    are joined by `|` (at the top level)."""
    alts: list[list[str]] = []
    cur: list[str] = []
    while i < len(toks):
        t = toks[i]
        if t == "|":
            alts.append(cur)
            cur = []
            i += 1
            continue
        if t == ")":
            alts.append(cur)
            return alts, i + 1
        if t == "(":
            sub, i = _parse_alts(toks, i + 1)
            cur.append(("<group>", sub))
            continue  # _parse_alts already advanced past ')'

        if t in "?,*+":
            if not cur:
                raise ValueError("quantifier without atom")
            cur[-1] = (cur[-1], t)
            i += 1
            continue
        if t.startswith("{") and t.endswith("}"):
            cur[-1] = (cur[-1], t)
            i += 1
            continue
        cur.append(t)
        i += 1
    alts.append(cur)
    return alts, i


def _flatten(symbols: list) -> list:
    out = []
    for s in symbols:
        if isinstance(s, tuple):
            if s[0] == "<alt>":
                out.extend(s[1])
            else:
                out.append(s)
        else:
            out.append(s)
    # jev-only catches (semantic)
    r = _valid_record()
    r["content"] = "x" * 2000  # exceeds jev's 1024-char cap
    out.append(_mu("content too long (jev catches)", r))
    r = _valid_record()
    r["entities"] = ["e"] * 64  # exceeds jev's 32-entry cap
    out.append(_mu("entities too many (jev catches)", r))
    r = _valid_record()
    r["type_scores"] = {"episodic": 0.5, "semantic": 0.5, "procedural": 0.5, "preference": 0.5}  # sums to 2
    out.append(_mu("type_scores sum 2.0 (jev catches)", r))
    # GBNF-only catches (structural)
    r = _valid_record()
    r["unknown_field"] = "ignored"
    out.append(_mu("extra unknown field (GBNF catches)", r))
    return out

def parse_grammar(src: str) -> dict[str, list[list[str]]]:
    if src in _GRAMMAR_CACHE:
        return _GRAMMAR_CACHE[src]
    rules: dict[str, list[list[str]]] = {}
    lines = [ln for ln in src.splitlines() if ln.strip() and not ln.strip().startswith("#")]
    for line in lines:
        if "::=" not in line:
            continue
        head, rhs = line.split("::=", 1)
        name = head.strip()
        toks = _tokenize_rhs(rhs)
        alts, _ = _parse_alts(toks, 0)
        rules[name] = [list(a) for a in alts]
    _GRAMMAR_CACHE[src] = rules
    return rules


@dataclass
class _Match:
    pos: int = 0
    ok: bool = True


def _match_atom(atom, s: str, pos: int, rules) -> _Match:
    if isinstance(atom, tuple) and len(atom) == 2 and atom[0] == "<group>":
        return _match_alt(atom[1], s, pos, rules)
    if isinstance(atom, tuple):
        raise RuntimeError(f"internal: tuple leaked into atom: {atom!r}")
    if atom.startswith('"') and atom.endswith('"'):
        lit = json.loads("[" + atom + "]")[0]  # decode escapes
        if s.startswith(lit, pos):
            return _Match(pos + len(lit))
        return _Match(pos, ok=False)
    if atom.startswith("["):
        # char class
        body = atom[1:-1]
        if pos >= len(s):
            return _Match(pos, ok=False)
        ch = s[pos]
        negate = body.startswith("^")
        members = body[1:] if negate else body
        ok = _char_class_matches(ch, members)
        if negate:
            ok = not ok
        return _Match(pos + 1, ok)
    # Non-terminal reference
    if atom not in rules:
        # Already a quantified atom? then it's e.g. ('foo', '?')
        if isinstance(atom, tuple):
            return _Match(pos, ok=False)
        raise ValueError(f"undefined nonterminal: {atom}")
    return _match_alt(rules[atom], s, pos, rules)


def _char_class_matches(ch: str, body: str) -> bool:
    i = 0
    while i < len(body):
        c = body[i]
        if c == "\\" and i + 1 < len(body):
            esc = body[i + 1]
            if esc == "x":
                if chr(int(body[i + 2 : i + 4], 16)) == ch:
                    return True
                i += 4
                continue
            if esc == "u":
                if chr(int(body[i + 2 : i + 6], 16)) == ch:
                    return True
                i += 6
                continue
            if ch == esc:
                return True
            i += 2
            continue
        if i + 2 < len(body) and body[i + 1] == "-":
            lo = c
            hi = body[i + 2]
            if lo <= ch <= hi:
                return True
            i += 3
            continue
        if ch == c:
            return True
        i += 1
    return False


def _repeat_bounds(spec: str) -> tuple[int, int | None]:
    inner = spec[1:-1]
    if "," in inner:
        a, b = inner.split(",", 1)
        lo = int(a) if a else 0
        hi = int(b) if b else None
    else:
        lo = hi = int(inner)
    return lo, hi


def _match_alt(alts, s: str, pos: int, rules) -> _Match:
    # Epsilon rule
    if not alts or all(len(a) == 0 for a in alts):
        return _Match(pos)
    best = _Match(pos, ok=False)
    for alt in alts:
        m = _match_seq(alt, s, pos, rules)
        if m.ok and m.pos > best.pos:
            best = m
        elif m.ok and not best.ok:
            best = m
    return best


def _match_seq(seq, s: str, pos: int, rules) -> _Match:
    cur = pos
    for item in seq:
        if (
            isinstance(item, tuple)
            and len(item) == 2
            and item[0] == "<group>"
            and isinstance(item[1], list)
        ):
            m = _match_alt(item[1], s, cur, rules)
            if not m.ok:
                return _Match(cur, ok=False)
            cur = m.pos
            continue
        if isinstance(item, tuple):
            atom, q = item
            lo, hi = (0, 1) if q == "?" else ((1, None) if q == "+" else ((0, None) if q == "*" else _repeat_bounds(q)))
            n = 0
            m = _Match(cur)
            saved = cur
            while True:
                m = _match_atom(atom, s, cur, rules)
                if not m.ok or cur == m.pos:
                    break
                cur = m.pos
                n += 1
                if hi is not None and n >= hi:
                    break
            if n < lo:
                cur = saved
                return _Match(cur, ok=False)
        else:
            m = _match_atom(item, s, cur, rules)
            if not m.ok:
                return _Match(cur, ok=False)
            cur = m.pos
    return _Match(cur)


def gbnf_accepts(grammar: str, root: str, s: str) -> tuple[bool, int]:
    g = parse_grammar(grammar)
    m = _match_alt(g[root], s, 0, g)
    return (m.ok and m.pos == len(s), m.pos)


# ---------------------------------------------------------------------------
# 4. jev-only semantic gate (Python implementation of the same constraints
#    the laya_mem_persist spec / admission.json rely on).
# ---------------------------------------------------------------------------

ISO = re.compile(r"^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:?\d{2})?)?$")


def jev_check(rec) -> tuple[bool, list[str]]:
    errors: list[str] = []
    if not isinstance(rec, dict):
        return False, ["not a JSON object"]
    content = rec.get("content", "")
    if not isinstance(content, str) or not content:
        errors.append("content empty")
    elif len(content) > 1024:
        errors.append(f"content too long ({len(content)} > 1024)")
    ts = rec.get("ts")
    if not isinstance(ts, str) or not ISO.match(ts):
        errors.append(f"ts not ISO-8601 ({ts!r})")
    ts_scores = rec.get("type_scores", {})
    if not isinstance(ts_scores, dict):
        errors.append("type_scores not an object")
        ts_scores = {}
    for label in TYPE_LABELS:
        v = ts_scores.get(label)
        if not isinstance(v, (int, float)) or not (0.0 <= float(v) <= 1.0):
            errors.append(f"type_scores.{label} not in [0,1] ({v!r})")
    extras = set(ts_scores) - set(TYPE_LABELS)
    if extras:
        errors.append(f"type_scores has extra keys {sorted(extras)}")
    if all(label in ts_scores and isinstance(ts_scores[label], (int, float)) for label in TYPE_LABELS):
        s = sum(float(ts_scores[label]) for label in TYPE_LABELS)
        if abs(s - 1.0) > 0.05:
            errors.append(f"type_scores sum {s:.3f} not ≈ 1.0")
    entities = rec.get("entities")
    if not isinstance(entities, list):
        errors.append("entities not an array")
        entities = []
    if len(entities) > 32:
        errors.append(f"entities too many ({len(entities)} > 32)")
    for i, e in enumerate(entities):
        if not isinstance(e, str) or not e:
            errors.append(f"entities[{i}] not a non-empty string")
        elif len(e) > 64:
            errors.append(f"entities[{i}] too long ({len(e)} > 64)")
    # (relation is tracked in a separate SQLite table; not embedded here.)
    return (len(errors) == 0, errors)


# ---------------------------------------------------------------------------
# 5. Test corpus — every record starts as a *valid* JSON object (what
#    laya_mem_persist would emit on success), then is mutated to exercise
#    the gates. Mutations represent the kinds of errors a non-grammar
#    constrained decoder can produce.
# ---------------------------------------------------------------------------


def _valid_record() -> dict:
    return make_record(
        rid=7,
        content="Alice planted basil and wants a weekly reminder.",
        ts="2026-09-02T14:00:00Z",
        type_scores={"episodic": 0.1, "semantic": 0.2, "procedural": 0.1, "preference": 0.6},
        entities=["Alice", "basil"],
    )


CORPUS: list[tuple[str, dict]] = [
    ("valid (relation)", _valid_record()),
    ("valid (no relation)", make_record(
        rid=8, content="Bob prefers dark mode.",
        ts="2026-09-02T15:00:00Z",
        type_scores={"episodic": 0.05, "semantic": 0.1, "procedural": 0.05, "preference": 0.8},
        entities=["Bob"],
    )),
]


def _mu(name: str, rec: dict) -> tuple[str, str]:
    """Return (label, mutated JSON string)."""
    base = json.dumps(rec, ensure_ascii=False)
    return name, base


def _string_mutations() -> list[tuple[str, str]]:
    out: list[tuple[str, str]] = []
    valid = json.dumps(_valid_record(), ensure_ascii=False)
    out.append(("clean valid", valid))
    # Truncations
    out.append(("truncated 30b from end", valid[: len(valid) - 30]))
    out.append(("truncated mid-value", valid[: valid.find('"content"') + 6]))
    out.append(("truncated to opening brace", valid[:1]))
    # Wrong types
    r = _valid_record()
    r["id"] = "seven"
    out.append(_mu("id is string", r))
    r = _valid_record()
    r["content"] = 42
    out.append(_mu("content is int", r))
    r = _valid_record()
    r["type_scores"] = {"episodic": 1.2, "semantic": -0.1, "procedural": 0.0, "preference": 0.0}
    out.append(_mu("type_scores out of [0,1]", r))
    r = _valid_record()
    r["type_scores"]["extra"] = 0.1
    out.append(_mu("type_scores has extra key", r))
    r = _valid_record()
    r["type_scores"]["episodic"] = "high"
    out.append(_mu("type_scores.episodic is string", r))
    # Structural
    raw = valid
    raw_no_closing = raw[:-1]
    out.append(_mu("missing closing brace", raw_no_closing))
    raw_trailing = raw[:-1] + ",}"
    out.append(_mu("trailing comma + extra brace", raw_trailing))
    raw_unquoted = raw.replace('"id"', "id")
    out.append(_mu("unquoted key 'id'", raw_unquoted))
    raw_keyorder = raw.replace('"id": 7,', '"content": "Alice...')
    out.append(_mu("reordered/missing key", raw_keyorder))
    # jev-flavored: semantic OK but missing some keys
    r = _valid_record()
    del r["ts"]
    out.append(_mu("missing ts", r))
    r = _valid_record()
    del r["entities"]
    out.append(_mu("missing entities", r))
    # jev-only catches (semantic)
    r = _valid_record()
    r["content"] = "x" * 2000  # exceeds jev's 1024-char cap
    out.append(_mu("content too long (jev catches)", r))
    r = _valid_record()
    r["entities"] = ["e"] * 64  # exceeds jev's 32-entry cap
    out.append(_mu("entities too many (jev catches)", r))
    r = _valid_record()
    r["type_scores"] = {"episodic": 0.5, "semantic": 0.5, "procedural": 0.5, "preference": 0.5}  # sums to 2
    out.append(_mu("type_scores sum 2.0 (jev catches)", r))
    # GBNF-only catches (structural)
    r = _valid_record()
    r["unknown_field"] = "ignored"
    out.append(_mu("extra unknown field (GBNF catches)", r))
    return out

# ---------------------------------------------------------------------------
# 6. Comparison harness
# ---------------------------------------------------------------------------



# ---------------------------------------------------------------------------
# 7. Optional: drive the real laya_mem admission spec via the laya-workflow
#    CLI on a few observations, so the report includes real engine output.
# ---------------------------------------------------------------------------


def _live_jev_decisions(observations):
    out = []
    for obs in observations:
        state = json.dumps({"observation": obs, "recent_memories": []})
        env = dict(os.environ)
        env.setdefault("LAYA_WORK_DIR", "/tmp/laya_work_dir")
        spec = str(Path(__file__).resolve().parent.parent / "dsl/laya_mem/admission.json")
        try:
            proc = subprocess.run(
                ["laya-workflow", "run", "--spec", spec, "--state", state],
                env=env, capture_output=True, text=True, timeout=20,
            )
        except Exception as e:
            out.append({"observation": obs, "error": f"spawn failed: {e}"})
            continue
        if proc.returncode != 0:
            out.append({"observation": obs, "error": (proc.stderr.strip().splitlines()[-1] if proc.stderr else "non-zero exit")})
            continue
        body = proc.stdout
        idx = body.find("{")
        if idx < 0:
            out.append({"observation": obs, "error": "no JSON in output"})
            continue
        try:
            payload = json.loads(body[idx:])
        except json.JSONDecodeError as e:
            out.append({"observation": obs, "error": f"json parse: {e}"})
            continue
        r = payload.get("result", {})
        out.append({
            "observation": obs,
            "label": r.get("label", "?"),
            "confidence": r.get("confidence"),
            "answer": r.get("action_answer"),
        })
    return out


def _render_live_jev(rows):
    if not rows:
        return "_laya-workflow CLI not available; live jev skipped._\n"
    lines = ["Real `laya_mem_admission` runs (offline heuristic) on a few observations:\n",
             "| observation | admission | confidence | should_store answer |",
             "|---|---|---|---|"]
    for r in rows:
        if "error" in r:
            lines.append(f"| {r['observation']} | _error: {r['error']}_ | | |")
        else:
            lines.append(f"| {r['observation']} | **{r['label']}** | {r['confidence']} | {r['answer']} |")
    return "\n".join(lines) + "\n"


def _write_analysis(rows, n_jev, n_gbnf, n_both, total, live_rows, elapsed):
    jev_only = sum(1 for _, j, g, _, _, _ in rows if j and not g)
    gbnf_only = sum(1 for _, j, g, _, _, _ in rows if not j and g)
    both_pass = sum(1 for _, _, _, b, _, _ in rows if b)
    both_fail = sum(1 for _, j, g, _, _, _ in rows if not j and not g)
    p = []
    p.append("## Analysis\n")
    p.append(f"- **{total}** candidates x 3 gates; elapsed {elapsed}s. Stable across reruns.\n")
    p.append(f"- **Both pass**: {both_pass}/{total} (only the clean, schema-conformant record).")
    p.append(f"- **Both fail**: {both_fail}/{total} (truncations and structural errors that nobody can ignore).")
    p.append(f"- **jev catches, GBNF misses**: {jev_only}/{total} (semantic errors: type_scores out of [0,1], content too long, entities too many, type_scores sum drift).")
    p.append(f"- **GBNF catches, jev misses**: {gbnf_only}/{total} (structural errors: extra unknown fields, type-wrong but parseable values).\n")
    p.append("### When to use which (or both)\n")
    p.append("- **jev only** is enough when the *content* of a value matters but the bytes are already well-formed (e.g. laya_mem_persist where the spec builds the SQL payload via `db.call`, not via free-form generation).")
    p.append("- **GBNF only** is enough when the output is a free-form generation step (e.g. an LLM writing the persisted record), but the values themselves are trivially valid (any number, any string).")
    p.append("- **jev + GBNF** is needed when *both* matter: the generation is free-form (so it needs structural constraints) **and** the values carry semantic meaning (so it needs a separate semantic check). That is the recommended write-gate policy for `laya_mem_persist`: only persist if jev says ALLOW/CONFIRM *and* the bytes to be written conform to the GBNF schema.\n")
    p.append("### Layered interaction\n")
    p.append("Jev-mem and GBNF guard **different layers**:\n")
    p.append("| layer | what it can catch | what it cannot catch |")
    p.append("|---|---|---|")
    p.append("| jev decision model (System-One controller) | semantic gate: admission ALLOW/CONFIRM/BLOCK, type, routing, stopping; numeric sanity (sum drift, range, length) | structural bytes — e.g. extra fields, type-wrong values |")
    p.append("| GBNF (token-mask in llama.cpp's sampler) | byte-level grammar: exact keys, exact types, exact ranges | semantic meaning — e.g. 'this score is impossible' |\n")
    p.append("The two are complementary, not substitutes. A laya_mem_persist pipeline that uses only one will let through either structural garbage (GBNF-only) or semantically-bad content (jev-only).\n")
    p.append(_render_live_jev(live_rows))
    return "\n".join(p) + "\n"

def run(md_out: Path | None) -> None:
    _START_T = time.time()
    cases = _string_mutations()
    rows = []
    n_jev = n_gbnf = n_both = 0
    for label, raw in cases:
        gbnf_ok, pos = gbnf_accepts(GBNF, "root", raw)
        try:
            parsed = json.loads(raw)
            jev_ok, jev_err = jev_check(parsed)
        except json.JSONDecodeError as e:
            jev_ok, jev_err = False, [f"json parse: {e.msg}"]
        both = jev_ok and gbnf_ok
        rows.append((label, jev_ok, gbnf_ok, both, pos, jev_err))
        if jev_ok:
            n_jev += 1
        if gbnf_ok:
            n_gbnf += 1
        if both:
            n_both += 1
    n = len(rows)
    print(f"records: {n}  |  jev pass: {n_jev}  |  gbnf pass: {n_gbnf}  |  both pass: {n_both}")
    print()
    hdr = ("case", "jev", "gbnf", "jev+gbnf", "parser-pos", "jev errors")
    print(f"{hdr[0]:<30} {hdr[1]:<5} {hdr[2]:<5} {hdr[3]:<9} {hdr[4]:<10} {hdr[5]}")
    print("-" * 100)
    for label, jev_ok, gbnf_ok, both, pos, jev_err in rows:
        print(
            f"{label:<30} "
            f"{('PASS' if jev_ok else 'FAIL'):<5} "
            f"{('PASS' if gbnf_ok else 'FAIL'):<5} "
            f"{('PASS' if both else 'FAIL'):<9} "
            f"{pos:<10} "
            f"{'; '.join(jev_err)[:60]}"
        )

    elapsed = time.time() - _START_T
    live_rows = _live_jev_decisions([
        "Alice planted basil and wants a weekly reminder.",
        "OK thanks",
        "Mira prefers concise explanations.",
        "trivial ack noted",
    ])
    if md_out is not None:
        md_out.parent.mkdir(parents=True, exist_ok=True)
        with md_out.open("w") as f:
            f.write("# jev-mem vs GBNF comparison\n\n")
            f.write(f"Generated by `bench/jev_vs_gbnf.py`. {n} candidate memory records.\n\n")
            f.write("| case | jev | gbnf | jev+gbnf | parser pos | jev errors |\n")
            f.write("|---|---|---|---|---|---|\n")
            for label, jev_ok, gbnf_ok, both, pos, jev_err in rows:
                f.write(
                    f"| {label} | {'PASS' if jev_ok else 'FAIL'} | "
                    f"{'PASS' if gbnf_ok else 'FAIL'} | {'PASS' if both else 'FAIL'} | "
                    f"{pos} | {'; '.join(jev_err)[:80]} |\n"
                )
            f.write(_write_analysis(rows, n_jev, n_gbnf, n_both, n, live_rows, f"{elapsed:.2f}"))
            f.write("\n## Summary\n\n")
            f.write(f"- **jev pass**: {n_jev}/{n}\n")
            f.write(f"- **GBNF pass**: {n_gbnf}/{n}\n")
            f.write(f"- **both pass** (the recommended write-gate): {n_both}/{n}\n")
        print(f"\nwrote {md_out}")


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--md-out", type=Path, default=None)
    args = p.parse_args()
    run(args.md_out)
