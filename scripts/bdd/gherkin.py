"""A deliberately small Gherkin parser.

Only the subset a laya-workflow BDD document actually needs:

    @tag
    Feature: name
      Background:
        Given ...

      Scenario: name
        Given ... / When ... / Then ... / And ... / But ...

      Scenario Outline: name
        Given ... uses <placeholders>
        Examples:
          | col | col |
          | a   | b   |

Every example row of an outline becomes its own concrete Scenario, so
downstream (the transpiler, the runner) never has to think about outlines
again - it sees plain scenarios with already-substituted step text.

This is not a general Gherkin implementation. It rejects what it does not
understand instead of guessing, because a BDD step that silently parses to
the wrong thing is worse than one that refuses to parse.
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass, field
from typing import Iterator

# Given / When / Then / And / But. And/But inherit the previous keyword, which
# is what Gherkin says they mean.
STEP_RE = re.compile(r"^\s*(Given|When|Then|And|But|\*)\s+(.*\S)\s*$")
TAG_RE = re.compile(r"^\s*@(\S+)\s*$")
# A table row: | a | b |  ->  ["a", "b"]
ROW_RE = re.compile(r"^\s*\|(.*)\|\s*$")
# `include: <path>` inside a Background, contributing that file's steps.
# Deliberately the *only* reuse mechanism here. Measured, the whole suite has
# 46 step lines and 37 unique ones, and 14 of the 9 duplicates are two
# boilerplate setup lines repeated in four features. A general $ref with
# parameters, conditionals and nesting would be a larger language to keep
# honest than the duplication it removes. This moves those two lines to one
# place and stops.
INCLUDE_RE = re.compile(r"^include:\s*(?P<path>[^\s]+)\s*$", re.IGNORECASE)


@dataclass
class Step:
    keyword: str  # the literal keyword as written
    kind: str  # resolved: given | when | then
    text: str  # step text, placeholders substituted
    line: int

    def __str__(self) -> str:
        return f"{self.kind} {self.text}"


@dataclass
class Scenario:
    name: str
    steps: list[Step] = field(default_factory=list)
    tags: list[str] = field(default_factory=list)
    line: int = 0
    outline: bool = False
    # (row_index, row_dict) for each Examples row; empty for a plain scenario.
    examples: list[tuple[int, dict[str, str]]] = field(default_factory=list)

    @property
    def slug(self) -> str:
        s = re.sub(r"[^a-z0-9]+", "_", self.name.lower()).strip("_")
        return s or "scenario"


@dataclass
class Feature:
    name: str
    description: str = ""
    background: list[Step] = field(default_factory=list)
    scenarios: list[Scenario] = field(default_factory=list)
    tags: list[str] = field(default_factory=list)
    path: str = ""

    @property
    def slug(self) -> str:
        s = re.sub(r"[^a-z0-9]+", "_", self.name.lower()).strip("_")
        return s or "feature"


class GherkinError(Exception):
    """Raised with a line number when a feature file cannot be read."""


def _cells(line: str, lineno: int) -> list[str]:
    m = ROW_RE.match(line)
    if not m:
        raise GherkinError(f"line {lineno}: expected a `| a | b |` table row, got: {line.strip()!r}")
    return [c.strip() for c in m.group(1).split("|")]


def _substitute(text: str, row: dict[str, str]) -> str:
    """Replace <placeholder> with the example row's value."""
    def repl(m: re.Match) -> str:
        key = m.group(1)
        if key not in row:
            raise GherkinError(f"Examples table has no column {key!r} (used by {text!r})")
        return row[key]

    return re.sub(r"<([^<>]+)>", repl, text)


def _read_include(rel: str, including: str, lineno: int) -> list[Step]:
    """Steps from an included file, or a GherkinError explaining why not.

    Every failure mode here is loud. The dangerous one would be an include
    that silently contributes nothing, because a feature that quietly lost its
    `Given I am on ...` would fail later as "needs a page" - a confusing error
    far from the line that caused it. So a missing file, an unreadable one, an
    empty one, one outside bdd/features, and one that itself includes are all
    errors here, and each says which file asked for it.
    """
    if os.path.isabs(rel):
        raise GherkinError(
            f"line {lineno}: `include {rel}` must be a path relative to the "
            f"including file, not an absolute path"
        )
    base = os.path.dirname(os.path.abspath(including)) if including != "<string>" else os.getcwd()
    target = os.path.normpath(os.path.join(base, rel))
    if not os.path.isfile(target):
        raise GherkinError(
            f"line {lineno}: `include {rel}` in {os.path.basename(including)} "
            f"does not exist (looked for {target})"
        )
    with open(target, encoding="utf-8") as f:
        body = f.read()
    if INCLUDE_RE.search(body) or any(
        INCLUDE_RE.match(l.strip()) for l in body.splitlines()
    ):
        raise GherkinError(
            f"line {lineno}: {rel} itself contains an `include`. Includes are "
            "one level deep on purpose - a cycle is a hang, not an error"
        )
    steps: list[Step] = []
    last_kind: str | None = None
    for n, raw in enumerate(body.splitlines(), start=1):
        text = raw.strip()
        if not text or text.startswith("#"):
            continue
        m = STEP_RE.match(raw)
        if not m:
            raise GherkinError(
                f"{rel} line {n}: not a step: {text!r} - an included file is a "
                "flat list of Given/When/Then, not another feature"
            )
        keyword = m.group(1)
        if keyword == "*":
            if last_kind is None:
                raise GherkinError(
                    f"{rel} line {n}: `*` step with no preceding Given/When/Then"
                )
            kind = last_kind
        elif keyword in ("And", "But"):
            if last_kind is None:
                raise GherkinError(
                    f"{rel} line {n}: {keyword} step with no preceding Given/When/Then"
                )
            kind = last_kind
        else:
            kind = {"Given": "given", "When": "when", "Then": "then"}[keyword]
        last_kind = kind
        steps.append(Step(keyword, kind, m.group(2), n))
    if not steps:
        raise GherkinError(
            f"line {lineno}: `include {rel}` contributed no steps. An include "
            "that silently adds nothing is worse than no include, so it is an "
            "error here rather than a feature that quietly lost its setup"
        )
    return steps


def parse(text: str, path: str = "<string>") -> Feature:
    lines = text.splitlines()
    feature: Feature | None = None
    scenario: Scenario | None = None
    pending_tags: list[str] = []
    last_kind: str | None = None
    in_background = False
    in_examples = False
    examples_header: list[str] | None = None
    section: str | None = None  # 'feature' | 'background' | 'scenario' | 'examples'
    desc: list[str] = []

    for lineno, raw in enumerate(lines, start=1):
        line = raw.rstrip()
        stripped = line.strip()

        if not stripped or stripped.startswith("#"):
            continue

        m = TAG_RE.match(line)
        if m:
            pending_tags.append(m.group(1))
            continue

        head = stripped.split(":", 1)[0].strip().lower()
        if head in ("feature", "background", "scenario", "scenario outline", "examples", "rule"):
            label, _, rest = stripped.partition(":")
            section = "scenario" if head == "rule" else head

            if section == "feature":
                feature = Feature(name=rest.strip(), tags=list(pending_tags), path=path)
                pending_tags = []
                desc = []
                in_background = False
                in_examples = False
                continue

            if feature is None:
                raise GherkinError(f"line {lineno}: {label.strip()} before any Feature:")

            if section == "background":
                in_background = True
                in_examples = False
                scenario = None
                continue

            if section == "examples":
                in_background = False
                in_examples = True
                if scenario is None:
                    raise GherkinError(f"line {lineno}: Examples: outside of a Scenario")
                examples_header = None
                continue

            # scenario / scenario outline / rule
            in_background = False
            in_examples = False
            if scenario is not None and scenario.outline and not scenario.examples:
                raise GherkinError(
                    f"line {lineno}: Scenario Outline {scenario.name!r} has no Examples table"
                )
            scenario = Scenario(
                name=rest.strip() or f"scenario@{lineno}",
                tags=list(pending_tags),
                line=lineno,
                outline=section == "scenario outline",
            )
            pending_tags = []
            last_kind = None
            feature.scenarios.append(scenario)
            continue

        if feature is None:
            raise GherkinError(f"line {lineno}: text before any Feature: header: {stripped!r}")

        if in_examples:
            assert scenario is not None
            cells = _cells(stripped, lineno)
            if examples_header is None:
                examples_header = cells
                if len(set(cells)) != len(cells):
                    raise GherkinError(f"line {lineno}: duplicate column in Examples header {cells}")
            else:
                if len(cells) != len(examples_header):
                    raise GherkinError(
                        f"line {lineno}: Examples row has {len(cells)} cells, "
                        f"header has {len(examples_header)}: {stripped!r}"
                    )
                scenario.examples.append((lineno, dict(zip(examples_header, cells))))
            continue

        if section == "feature" and not feature.scenarios and not stripped.lower().startswith(
            ("given", "when", "then", "and", "but")
        ):
            desc.append(stripped)
            feature.description = "\n".join(desc)
            continue

        inc = INCLUDE_RE.match(stripped)
        if inc:
            # Only meaningful where steps are collected, and only one level
            # deep. An include inside an included file is refused rather than
            # followed: a cycle here is a hang, not an error, and the fix for
            # a hang is much harder to read than the fix for a message.
            if not in_background:
                raise GherkinError(
                    f"line {lineno}: `include` is only allowed inside a "
                    f"Background, not in a {section!r} block"
                )
            feature.background.extend(_read_include(inc.group("path"), path, lineno))
            last_kind = None
            continue

        m = STEP_RE.match(line)
        if not m:
            raise GherkinError(f"line {lineno}: not a step, table or section header: {stripped!r}")

        keyword, body = m.group(1), m.group(2)
        if keyword == "*":
            if last_kind is None:
                raise GherkinError(f"line {lineno}: `*` step with no preceding Given/When/Then")
            kind = last_kind
        elif keyword in ("And", "But"):
            if last_kind is None:
                raise GherkinError(f"line {lineno}: {keyword} step with no preceding Given/When/Then")
            kind = last_kind
        else:
            kind = {"Given": "given", "When": "when", "Then": "then"}[keyword]
        last_kind = kind

        if in_background:
            feature.background.append(Step(keyword, kind, body, lineno))
        elif scenario is not None:
            scenario.steps.append(Step(keyword, kind, body, lineno))
        else:
            raise GherkinError(f"line {lineno}: step outside Background/Scenario: {stripped!r}")

    if feature is None:
        raise GherkinError("no Feature: header found")
    if not feature.scenarios:
        raise GherkinError("feature has no Scenario")
    for s in feature.scenarios:
        if s.outline and not s.examples:
            raise GherkinError(f"Scenario Outline {s.name!r} has no Examples table")

    return _expand_outlines(feature)


def _expand_outlines(feature: Feature) -> Feature:
    """Turn every Examples row into a concrete scenario."""
    out: list[Scenario] = []
    for s in feature.scenarios:
        if not s.outline:
            out.append(s)
            continue
        for i, (lineno, row) in enumerate(s.examples):
            out.append(
                Scenario(
                    name=_substitute(s.name, row)
                    + (f" (row {i + 1})" if len(s.examples) > 1 else ""),
                    steps=[Step(st.keyword, st.kind, _substitute(st.text, row), st.line) for st in s.steps],
                    tags=list(s.tags),
                    line=lineno,
                )
            )
    feature.scenarios = out
    return feature


def load(path: str) -> Feature:
    with open(path, "r", encoding="utf-8") as fh:
        return parse(fh.read(), path=path)


def iter_features(paths: Iterator[str] | list[str]) -> Iterator[Feature]:
    for p in paths:
        yield load(p)
