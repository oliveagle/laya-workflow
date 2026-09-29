#!/usr/bin/env python3
"""Every bundled plugin must appear in the registry that documents it.

"X is a standard plugin" is a claim, and this repo keeps a registry for it: the
`## Bundled plugins` table in both `docs/plugins.md` (the reference) and
`src/skill/sections/plugins.md` (the section the skill loads). Measured, that
table was missing three of the four directories under `plugins/`:

    plugins/bdd            absent from both
    plugins/jev-planner    absent from both
    plugins/browser_base   absent from the skill table only

So the registry did not register everything it ships, and a plugin could be
written, compiled, gated and run while being invisible to the one document
whose job is to say what exists. That is the same failure as a step table that
maps a step to the wrong op: the system works, and the thing that describes it
has quietly stopped being true.

Scope is deliberately `plugins/` only. Those four are the tool plugins - the
surface where "a standard plugin" is the actual question - and they are all
stable. The `websites/` tree grows faster, several directories are named
differently from the table (`websites/developer.mozilla.org` is listed as
`websites/mdn`, `websites/huggingface.co` as `websites/hf-trending`), and some
are mid-flight, so demanding all of them would make this a nuisance rather than
a gate. The other direction is checked too: a row pointing at a directory that
was renamed or deleted sends a reader to nothing.

Chrome-free, network-free, no build step - so it is cheap enough to run in
both `scripts/verify.sh` and CI, which is the point: `verify.sh` is not invoked
by CI, so a check wired only into it would be a check that runs on one
developer's machine and nowhere else.
"""

from __future__ import annotations

import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                    ".."))
PLUGIN_DIR = os.path.join(ROOT, "plugins")

# Both documents that keep the table. They are written by hand for different
# audiences and they drift from each other, which is exactly why the check
# covers both rather than whichever happens to be longer.
REGISTRIES = [
    os.path.join(ROOT, "docs", "plugins.md"),
    os.path.join(ROOT, "src", "skill", "sections", "plugins.md"),
]

ROW_RE = re.compile(r"^\| `(?P<name>plugins/[A-Za-z0-9_.-]+)`", re.M)


def _has_manifest(directory: str) -> bool:
    """A plugin directory, in either layout the repo uses.

    `plugins/<name>/plugin.json` is how the tool plugins are laid out, and
    `websites/<domain>/plugin/plugin.json` is how the site plugins are. Only
    the first is looked for here, and that is a hole rather than a detail: a
    plugin laid out the other way ships, compiles, and is *invisible to this
    check*, so it would never be asked to be in the registry. Found by building
    the negative test the wrong way round and watching it pass.
    """
    return (os.path.isfile(os.path.join(directory, "plugin.json"))
            or os.path.isfile(os.path.join(directory, "plugin", "plugin.json")))


def shipped_plugins() -> list[str]:
    if not os.path.isdir(PLUGIN_DIR):
        return []
    return sorted(
        f"plugins/{d}" for d in os.listdir(PLUGIN_DIR)
        if os.path.isdir(os.path.join(PLUGIN_DIR, d))
        and _has_manifest(os.path.join(PLUGIN_DIR, d))
    )


def main() -> int:
    shipped = shipped_plugins()
    if not shipped:
        print("plugin registry: no plugins/ with a plugin.json - skipped")
        return 0

    problems: list[str] = []
    for path in REGISTRIES:
        rel = os.path.relpath(path, ROOT)
        if not os.path.isfile(path):
            problems.append(f"{rel} is missing, so nothing registers the plugins")
            continue
        with open(path, encoding="utf-8") as f:
            listed = set(ROW_RE.findall(f.read()))
        if not listed:
            problems.append(f"{rel} has no `## Bundled plugins` table any more - "
                            "restore it, or delete this check rather than "
                            "letting it rot")
            continue
        for name in shipped:
            if name not in listed:
                problems.append(
                    f"{rel} does not list {name}, which ships. A plugin that is "
                    "not in the registry is a plugin nobody can find")
        for name in sorted(listed):
            if name not in shipped:
                problems.append(
                    f"{rel} lists {name}, which does not ship (no plugin.json). "
                    "Either the directory was renamed or the row is stale")

    if problems:
        print(f"plugin registry: {len(problems)} problem(s):", file=sys.stderr)
        for p in problems:
            print(f"  {p}", file=sys.stderr)
        return 1
    print(f"plugin registry: {len(shipped)} bundled plugin(s) listed in "
          f"{len(REGISTRIES)} registries, no stale rows")
    return 0


if __name__ == "__main__":
    sys.exit(main())
