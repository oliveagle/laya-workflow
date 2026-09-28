# notify — desktop notifications (macOS banner + log fallback)

`kind: "notify"` (alias `notify_local`) posts a **real desktop notification**.
On macOS it fires a Notification Center banner via `osascript`'s
`display notification`; it can also append a line to a log file. No extra crate:
the banner is an `osascript` call run through the `exec` capability, so it obeys
the same policy gates.

## Channels — where the notification goes

Set `channel` on the capability, or override it per call with `with.channel`:

| channel | effect | needs |
|---------|--------|-------|
| `log` (default; `""`/`file`) | append `[<epoch>\t]<event>\t<message>` to `path` | `allow_paths` |
| `macos` (`os`/`osx`/`notification`) | Notification Center banner | `allow_exec` + macOS host |
| `both` | banner **and** log line | `allow_exec` + `allow_paths` |
| `auto` | `macos` on macOS, else `log` | portable; `allow_exec` on macOS |

`auto` is the portable default: a banner on a dev Mac, a log line on Linux/CI.
On a non-macOS host the `macos` channel fails closed with a clear message.

## Fields

* `title` (default `laya-workflow`), `subtitle`, `sound` (e.g. `Glass`),
  `timeout_ms` — banner shape; each is overridable per call via `with`.
* `path` — the log file for the `log` channel (path-allow-listed).
* `timestamp` (default `true`) — prefix the log line with the epoch seconds.
* `bell` — also write `\x07` to stderr.

The message comes from the call: `with.message`; `with.event` is the log tag
(default `notify`).

## Policy

The banner **spawns `osascript`** ⇒ `policy.allow_exec: true`. The `log` channel
writes a file ⇒ the file (or its parent) under `policy.allow_paths`. Prefer
`auto` when one spec must run on both macOS and CI.

## Example

```jsonc
{
  "policy": { "allow_exec": true, "allow_paths": ["${env.LAYA_WORK_DIR}"] },
  "capabilities": {
    "banner": { "kind": "notify", "channel": "auto", "title": "laya-workflow",
                "subtitle": "${state.topic}", "sound": "Glass",
                "path": "${env.LAYA_WORK_DIR}/notify.log", "timestamp": true }
  }
  // {"kind":"call","capability":"banner",
  //  "with":{"event":"notify","message":"${state.text}"}}
}
```

Runnable spec: `dsl/capabilities/notify_macos.json`. Full reference:
`docs/notify.md`.

## CLI — no spec needed

```bash
laya-workflow notify --message "build finished" --title Laya --subtitle ci --sound Glass
laya-workflow notify --message "logged" --channel log  --path /tmp/laya-notify/log.txt
laya-workflow notify --message "both"   --channel both --path /tmp/laya-notify/log.txt
```

`--channel` defaults to `auto`.

Next: `skill --section safety` (the policy gates), `skill --section dsl`
(the kind catalogue).
