#!/bin/sh
# Collect unread Feishu (飞书) messages across every chat, for the unread digest.
#
#   collect.sh [chats_limit] [page_size]      defaults: 15 chats, 20 messages
#
# Emits one JSON object on stdout: {"ok":true,"chats":[{chat_id,chat_name,
# chat_type,unread:[message...]}]} - chats with nothing unread are omitted, so
# a quiet account prints {"ok":true,"chats":[]}.
#
# There is no Feishu unread API to ask ("GET /open-apis/im/v1/message_badge" is a
# 404). Unread is therefore *derived*, in two calls per chat:
#   1. +chat-messages-list --order desc   recent messages, newest first
#   2. +messages-read-status               is_read per message id, 50 at a time
# and the digest keeps the intersection. Cost is two CLI spawns per chat, so
# chats_limit is the knob that trades coverage for latency (~1.3s per chat
# against a cold token; a full 20-chat sweep measures ~22s).
#
# Failures are *not* swallowed: `ok:false` with the CLI's own error is printed
# and the exit code is non-zero, so the classifier reports "collect failed"
# rather than digesting an empty list as "you are all clear" - the failure mode
# that makes a triage list dangerous to trust.

set -u
# The engine stringifies an unset ${state.x} through serde_json::Value, and
# Value::Null.to_string() is the literal "null" - not "". Passing that straight
# to lark-cli gives `--page-size null`, which exits 1 with no stdout, and the
# classifier then reports a confusing json_parse EOF. `case` on both spellings
# so the spec can leave either knob out entirely.
CHATS_LIMIT="$1"; case "$CHATS_LIMIT" in ""|null) CHATS_LIMIT=15 ;; esac
PAGE="$2";       case "$PAGE"       in ""|null) PAGE=20 ;; esac

# `--print-knobs` exists so scripts/rhai/check_feishu.py can assert this
# defaulting against the real script instead of a copy of it that could drift.
# It prints the resolved knobs and exits without touching the network, which is
# what lets the assertion run in the pre-push gate with no lark-cli token. The
# flag is shifted off first, so `collect.sh --print-knobs null ""` tests the
# very argument positions the spec fills.
if [ "${1:-}" = "--print-knobs" ]; then
  shift
  CHATS_LIMIT="$1"; case "$CHATS_LIMIT" in ""|null) CHATS_LIMIT=15 ;; esac
  PAGE="$2";       case "$PAGE"       in ""|null) PAGE=20 ;; esac
  echo "$CHATS_LIMIT/$PAGE"; exit 0
fi

export PAGE
lark-cli im +chat-list --types=p2p,group --exclude-muted --page-size "$CHATS_LIMIT" --format json 2>/dev/null \
| python3 -c '
import json,os,sys,subprocess
PAGE=int(os.environ.get("PAGE","20"))
def cli(a):
    r=subprocess.run(["lark-cli"]+a,capture_output=True,text=True)
    try: return json.loads(r.stdout)
    except Exception: return {}
d=json.load(sys.stdin)
if not d.get("ok"):
    print(json.dumps({"ok":False,"err":str(d.get("error"))},ensure_ascii=False)); raise SystemExit(1)
out=[]
for c in d.get("data",{}).get("chats",[]):
    cid=c["chat_id"]; mode=c.get("chat_mode","")
    m=cli(["im","+chat-messages-list","--chat-id",cid,"--page-size",str(PAGE),
           "--order","desc","--no-reactions","--format","json"])
    msgs=[x for x in ((m.get("data") or {}).get("messages") or []) if not x.get("deleted")]
    if not msgs: continue
    ids=[x["message_id"] for x in msgs][:50]
    st=cli(["im","+messages-read-status","--as","user","--message-ids",",".join(ids),"--format","json"])
    un={i["message_id"] for i in ((st.get("data") or {}).get("items") or []) if not i.get("is_read")}
    unread=[x for x in msgs if x["message_id"] in un]
    if not unread: continue
    for x in unread:
        x["chat_name"]=c.get("name",""); x["chat_id"]=cid; x["chat_type"]=mode
        x["app_link"]=x.get("message_app_link","")
    out.append({"chat_id":cid,"chat_name":c.get("name",""),"chat_type":mode,"unread":unread})
print(json.dumps({"ok":True,"chats":out},ensure_ascii=False))
'
