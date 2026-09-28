# `notify` — desktop notifications (macOS Notification Center + log fallback)

`kind: "notify"` (alias `notify_local`) turns a workflow step into a **real
desktop notification**. On macOS it posts a Notification Center banner through
`osascript`'s `display notification`; everywhere it can also append a line to a
log file. There is no extra crate — the banner is just an `osascript` invocation
run through the `exec` capability, so it obeys the same policy gates.

## Channels

The `channel` field (capability-level, overridable per call with
`with.channel`) decides where the notification goes:

| channel | effect |
|---------|--------|
| `log` (default, aliases `""` / `file`) | append `[<epoch>\t]<event>\t<message>` to `path` (path-allow-listed) |
| `macos` (aliases `os` / `osx` / `notification`) | Notification Center banner via `osascript` |
| `both` | banner **and** the log line |
| `auto` | `macos` on macOS, `log` elsewhere |

`auto` is the portable choice: on a developer Mac you get a banner, on a Linux
CI box the same spec degrades to a log file with no change.

## Banner fields

| field | meaning |
|-------|---------|
| `title` | banner title (default `laya-workflow`) |
| `subtitle` | banner subtitle (optional) |
| `sound` | sound name, e.g. `Glass`; empty ⇒ silent |
| `timeout_ms` | bound on the `osascript` call |

The message itself comes from the call: `with.message` (with `with.event` used
as the log tag, default `notify`). `title` / `subtitle` / `sound` may also be
overridden per call through `with`.

## Policy

Posting a banner **spawns `osascript`**, so the spec needs
`policy.allow_exec: true`. The `log` channel writes a file, so that file (or its
parent) must sit under `policy.allow_paths`. `auto`/`macos`/`both` therefore need
`allow_exec`; a `log`-only spec needs just `allow_paths` (and no `allow_exec`).
On a non-macOS host, the `macos` channel fails closed with a clear message — use
`auto` (or `log`) if the same spec must run on Linux/CI.

## Spec

`dsl/capabilities/notify_macos.json`:

```jsonc
{
  "policy": { "allow_exec": true, "allow_paths": ["${env.LAYA_WORK_DIR}"] },
  "capabilities": {
    "banner": {
      "kind": "notify",
      "channel": "auto",
      "title": "laya-workflow",
      "subtitle": "${state.topic}",
      "sound": "Glass",
      "path": "${env.LAYA_WORK_DIR}/notify.log",
      "timestamp": true
    }
  }
  // node action: {"kind":"call","capability":"banner",
  //               "with":{"event":"notify","message":"${state.text}"}}
}
```

```sh
export LAYA_WORK_DIR=/tmp/laya-notify-demo && mkdir -p "$LAYA_WORK_DIR"
laya-workflow run --spec dsl/capabilities/notify_macos.json \
  --state '{"text":"build finished","topic":"ci"}'
```

The call returns the delivery record:

```jsonc
{ "capability": "notify", "channel": "macos",
  "event": "notify", "message": "build finished",
  "macos": { "title": "laya-workflow", "subtitle": "ci", "sound": "Glass",
             "delivered": true, "exit_code": 0, "script": "display notification …" } }
```

`macos` / `path` / `bytes` / `bell` appear only for the channel(s) that ran.

## CLI

The same machinery is exposed directly, so you can fire a notification from a
shell without writing a spec:

```bash
laya-workflow notify --message "build finished" --title Laya --subtitle ci --sound Glass
laya-workflow notify --message "logged" --channel log --path /tmp/laya-notify/log.txt
laya-workflow notify --message "both"   --channel both --path /tmp/laya-notify/log.txt
```

`--channel` defaults to `auto`.
