# install — the state root, bundled plugins, and the laya-mem memory gate

## One root

Everything this tool persists for the user lives under a single directory:

| path | what | override |
|---|---|---|
| `~/.laya-workflow/dsl/` | per-user specs | `LAYA_USER_DSL_DIR` |
| `~/.laya-workflow/plugins/` | installed plugins (`plugin install`) | `LAYA_USER_PLUGIN_DIR` |
| `~/.laya-workflow/websites/` | site-scoped plugins, `websites/<domain>/plugin/` | follows the plugin root |
| `~/.laya-workflow/laya-mem/` | the memory gate: `specs/` + `codex.sqlite` | `LAYA_MEM_SPEC_DIR` / `LAYA_MEM_SQLITE` |
| `~/.laya-workflow/chrome/` | Chrome profile for `chrome_cdp` | `LAYA_BROWSER_PROFILE` |

`$LAYA_HOME` moves the whole root; otherwise it is `$HOME/.laya-workflow`.
Precedence is always *per-purpose var → `$LAYA_HOME` → the default above*, and an
empty var counts as unset.

`$XDG_CONFIG_HOME` deliberately does **not** participate. A dot-directory in
`$HOME` keeps the tool's state in one place that does not move when a user
switches XDG roots, and it sits beside the `~/.laya-workflow/chrome` profile the
browser code already used.

## `laya-workflow install`

```sh
laya-workflow install                # layout + every bundled plugin + the laya-mem specs
laya-workflow install --force        # re-install plugins AND overwrite edited specs
laya-workflow install --dirs-only    # just create the directories
```

Idempotent. Without `--force` an existing plugin is kept and an edited spec is
left alone, so re-running after a `git pull` upgrades what it can without
discarding local work. With `--force` both are replaced by the shipped copies.

It installs from **two** sources, and both matter:

* every plugin discoverable on disk (a checkout's `plugins/` and `websites/`);
* every plugin **compiled into the binary** (written out to
  `~/.laya-workflow/plugins/`, so a machine with only the binary still ends up
  with a usable *and editable* plugin tree).

On-disk copies win where both exist, because a checkout can carry a plugin newer
than the binary reading it. Builtins are normally used straight out of the
embedded copy, so materializing them is only about making them patchable.

After this, nothing needs a checkout: `plugin list` resolves every plugin from
the `user` layer, and `mcp serve` finds its specs.

## laya-mem — the memory gate

`laya-workflow mcp serve` is an MCP stdio server exposing four tools
(`laya_mem_assess` / `laya_mem_retrieve` / `laya_mem_persist` /
`laya_mem_recall`) over the 8 System-One specs in `dsl/laya_mem/`.

```sh
laya-workflow laya-mem info      # resolved spec dir, store, backend, per-spec presence
laya-workflow laya-mem install   # write the embedded specs, keep local edits
laya-workflow laya-mem restore   # overwrite the specs with the shipped copies
```

`info` reports **per-spec** presence on purpose. The failure that matters is a
server that starts cleanly and then answers every tool call with
`spec not found` — which reads like a broken install rather than a missing
directory. The specs are compiled into the binary, so this cannot happen for an
installed copy; it can only happen if someone points `LAYA_MEM_SPEC_DIR`
somewhere wrong.

The specs are written out rather than read from the embedded copy so they stay
inspectable and editable — they are the System-One policy, and tuning it should
mean opening the JSON. `restore` is the way back.

| var | default | effect |
|---|---|---|
| `LAYA_MEM_SQLITE` | `<state>/laya-mem/codex.sqlite` | the store (created on first `persist`) |
| `LAYA_MEM_SPEC_DIR` | `<state>/laya-mem/specs` | the System-One specs |
| `LAYA_BASE_URL` | unset | unset = offline heuristic; set = a real Laya `/v1/systemone` |

The store is only created on the first `persist` — `info` names the directory it
will appear in rather than creating an empty file.

### Registering it with an MCP host

```sh
# Codex — persists into ~/.codex/config.toml
codex mcp add laya_mem -- "$(command -v laya-workflow)" mcp serve

# OpenCode — ~/.config/opencode/opencode.json, as a SIBLING under the "mcp" key.
# NOT `opencode mcp add --global`: older versions write a nested mcp.servers.*
# that the host does not read.
#   "mcp": { "laya_mem": { "type": "local",
#                          "command": ["laya-workflow", "mcp", "serve"],
#                          "enabled": true } }
cd ~ && opencode mcp list     # no --global flag; must run in $HOME or a project
```

MCP servers load at session start, so **restart the host** after registering.
