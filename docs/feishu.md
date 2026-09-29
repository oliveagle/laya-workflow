# Feishu / Lark (`飞书`) — reading chats and history with `lark-cli`

**Short answer: yes.** `lark-cli` (installed at `~/.local/bin/lark-cli`) is a
Feishu/Lark CLI that covers the whole IM surface — list chats, read a
conversation's message history, search across chats, read threads, download
attachments, send/reply, reactions, pins, urgent-notify, interactive cards. The
`lark-im` skill (`~/.agents/skills/lark-im/`) documents it. laya-workflow reaches
it through the `exec` (or `shell`) capability, so a workflow can pull Feishu
history into state and run model decisions over it.

There is no Feishu code in this repo — the integration point is the CLI itself,
driven by `exec`. See `dsl/capabilities/feishu_chat_history.json`.

## Identity and auth

`lark-cli auth status` reports the app and per-identity state:

```jsonc
{ "appId": "cli_…", "brand": "feishu", "identity": "user",
  "identities": {
    "user": { "status": "ready", "userName": "Oliver Tang (唐锐华)", "openId": "ou_…",
              "scope": "im:chat:read im:message:readonly im:message.group_msg:get_as_user
                        im:message.p2p_msg:get_as_user search:message …" },
    "bot":  { "status": "ready" } } }
```

Two identities, and the choice matters:

* `--as user` — acts as the logged-in person: sees **their own** chats and DMs,
  including history. This is what you want for "read my conversations".
* `--as bot` — acts as the app bot: only chats the bot is a **member of** (e.g.
  a P2P chat with a GitLab notification bot). Querying user resources as the bot
  returns **empty success, not an error**, so it silently looks like "no data".

Reading history is gated by these scopes, all already granted here:
`im:chat:read`, `im:message:readonly`, `im:message.group_msg:get_as_user`,
`im:message.p2p_msg:get_as_user`, `search:message`.

Re-auth: `lark-cli auth login` (prints a `verification_url` → render it with
`lark-cli auth qrcode`). The user token auto-refreshes on the next user call;
`refreshExpiresAt` is the real deadline (e.g. 2026-09-30), after which you must
log in again. Success is `ok == true` / exit 0 — **not** `code == 0`.

## Reading history

```bash
# group chat or topic chat
lark-cli im +chat-messages-list --chat-id oc_xxx --page-size 50 --order desc

# a direct message (resolves the p2p chat_id from the other user's open_id)
lark-cli im +chat-messages-list --user-id ou_xxx

# bounded window / pagination / compact text
lark-cli im +chat-messages-list --chat-id oc_xxx --start 2026-09-01 --end 2026-09-28
lark-cli im +chat-messages-list --chat-id oc_xxx --page-all --page-limit 5 --format json
lark-cli im +chat-messages-list --chat-id oc_xxx --concise
```

Find the `chat_id` first:

```bash
lark-cli im +chat-list --types=group,p2p            # chats you are in
lark-cli im +chat-search --query "系统研发部"        # search visible chats by name
```

Search across conversations (keyword + filters):

```bash
lark-cli im +messages-search --query "laya" --page-size 20
lark-cli im +messages-search --query "发布" --chat-type group --start 2026-09-01
```

Each message comes back with `message_id` (`om_…`), `msg_type`
(`text`/`post`/`interactive`/`image`/`file`/…), `create_time`, a resolved
`sender.name`, and an app link. `--download-resources` saves attachments into
`./lark-im-resources/`. Thread replies: `lark-cli im +threads-messages-list`.

## Driving it from laya-workflow

`dsl/capabilities/feishu_chat_history.json` runs the CLI through `exec` and
projects the JSON into state:

```jsonc
{ "policy": { "allow_exec": true },
  "capabilities": {
    "history": { "kind": "exec",
      "argv": ["lark-cli","im","+chat-messages-list",
               "--chat-id","${state.chat_id}","--page-size","20",
               "--order","desc","--no-reactions","--format","json"] } },
  // node actions:
  //   {"kind":"call","capability":"history","with":{},
  //    "project":{"messages_raw":"/stdout","exit_code":"/exit_code"}}
  //   … then a choice node reads the history and decides
}
```

```bash
export PATH="$HOME/.local/bin:$PATH"
laya-workflow run --spec dsl/capabilities/feishu_chat_history.json \
  --state '{"chat_id":"oc_xxx"}'
```

## 未读消息分类汇总 — `feishu_unread_digest`

`dsl/capabilities/feishu_unread_digest.json` sweeps every chat, keeps only what
you have **not** read, buckets it by what it asks of you, and prints a ranked
digest. Read-only: it never sends, marks read, or reacts.

```bash
export PATH="$HOME/.local/bin:$PATH"
laya-workflow run --spec dsl/capabilities/feishu_unread_digest.json \
  --state '{"chats_limit":20,"page_size":20}'
```

```
[question] MOM技术支持 / Ke Guo (郭可)
    咨询下，两个 MOM 项目都通过 "项目引用" 引用了同一个 common mom，生成的两个
    jar 里各自内嵌了一份 common 的模型类…想问下有什么解决方法？
    -> https://applink.feishu.cn/client/chat/open?openChatId=oc_…
```

Three nodes: `collect` (shell) → `classify` (Rhai) → `triage` (a choice node
that judges whether anything actually needs a reply).

### Unread has to be derived

There is no Feishu endpoint for "my unread messages" —
`GET /open-apis/im/v1/message_badge` is a **404**, and `+chat-list` returns no
read state. So `websites/feishu.com/plugin/collect.sh` derives it per chat:

1. `+chat-messages-list --order desc` — recent messages, newest first
2. `+messages-read-status` — `is_read` per message id, 50 at a time
3. keep the intersection

That is two CLI spawns per chat (~1.3s each against a cold token), so
`chats_limit` is the coverage/latency knob — a 20-chat sweep measures ~22s.
Chats with nothing unread are omitted, so a quiet account yields
`{"ok":true,"chats":[]}`.

Both knobs are optional, and the defaulting is not free. The engine expands an
unset `${state.page_size}` through `serde_json::Value`, and
`Value::Null.to_string()` is the literal `"null"` — **not** `""`. A plain
`${2:-20}` therefore hands the script the string `null`, `lark-cli` exits 1 with
no stdout, and the classifier fails on a bare `json_parse` EOF that says nothing
about the real cause. `collect.sh` guards both spellings:

```console
$ sh websites/feishu.com/plugin/collect.sh --print-knobs null ""
15/20
```

`--print-knobs` prints the resolved knobs and exits without any network call,
which is how the gate asserts this defaulting against the real script rather
than a copy that could drift.

### Buckets

`mention` → `action` → `question` → `decision` → `info`, and the order *is* the
priority. The rules that are not obvious:

* **`@_all` beats everything.** Being called out in front of a group outranks a
  plain question, so `mention` is checked first.
* **A DM with no keyword word is still `action`.** "看了" has no cue word, but in
  a group silence is normal while in a DM the sender is waiting — filing it
  under `info` is how a DM gets missed.
* **`[Invalid text JSON]` is surfaced, not dropped.** That literal is what the
  CLI emits for a payload it could not decode. It is forced to `question`,
  because the undecodable body may be hiding the point.
* **`post` bodies arrive as HTML.** Real rich text reads `<p>请问…</p>`, so the
  tags are stripped before cue words match, or the digest quotes markup.

Cue words are overridable without editing the plugin:
`--state '{"action_cn":["帮忙","跟进"]}'`.

### Why rules, and where the model still decides

Per-message routing is keyword-based because it has to be instant, deterministic
and reviewable — you can see exactly why a message landed where it did. The
*digest* is still judged by a model: the `triage` node reads the buckets and
answers whether anything needs a reply.

### The gate

`scripts/rhai/check_feishu.py` asserts the routing against
`scripts/rhai/fixtures/feishu_unread.json` (7 messages, one per bucket, shaped
from real `lark-cli` output) and runs inside `scripts/rhai/check.sh`. It has to
*execute* the plugin: this Rhai build ships no `replace`, `replace_all`, `strip`
or array `clone`, and a missing builtin is a runtime `Function not found` that
compiles clean and then fails on the first real message.

## Safety notes

* Any `exec`/`shell` spec that calls `lark-cli` **spawns a process** ⇒ needs
  `policy.allow_exec: true`. `policy.allow_hosts` does **not** apply (that gates
  the `http` capability); the CLI's own network calls are not host-filtered, so
  treat a `lark-cli` exec like any other exec — trusted argv only.
* Reading is read-only. **Sending** (`+messages-send` / `+messages-reply`) is a
  write to other people's chats; the skill marks risky writes `high-risk-write`
  (exit 10 + `--yes`), and you should confirm intent before running them. Don't
  wire a send into an unattended workflow without an explicit gate.
* The CLI is user-authenticated: whoever runs the workflow operates **as that
  user** on real chats. Secrets/tokens are stored in `lark-cli`'s own config, not
  in the spec.
