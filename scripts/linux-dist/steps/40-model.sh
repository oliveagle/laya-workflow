#!/bin/bash
# 下载 convaiinnovations/laya 权重（root 变体）+ tokenizer。
# 807 MB，是整个包里最大的一块，也是"必须带上"的部分。
source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

step "40/80 下载模型 $MODEL_REPO@$MODEL_REV"

DEST=$WORK/model
mkdir -p "$DEST/tokenizer"

fetch() { # url dest expect_bytes
  local url=$1 dest=$2 want=$3 got
  if [ -f "$dest" ] && [ "$(filesize "$dest")" = "$want" ]; then
    ok "$(basename "$dest")  $(human "$want")"
    return
  fi
  info "GET $(basename "$dest")  ($(human "$want"))"
  curl -fL --retry 5 --retry-delay 3 --progress-bar -o "$dest" "$url" || die "下载失败: $url"
  got=$(filesize "$dest")
  [ "$got" = "$want" ] || die "$(basename "$dest") 大小不对：期望 ${want}，实际 $got"
  ok "$(basename "$dest")  $(human "$got")"
}

fetch "$MODEL_BASE/model.safetensors"            "$DEST/model.safetensors"            "$MODEL_SAFETENSORS_BYTES"
fetch "$MODEL_BASE/tokenizer/tokenizer.json"      "$DEST/tokenizer/tokenizer.json"     "$MODEL_TOKENIZER_BYTES"
# tokenizer_config.json 是可选的，拿到就带上，没有也不影响
fetch_optional() {
  local url=$1 dest=$2
  if [ -f "$dest" ]; then ok "$(basename "$dest")"; return; fi
  curl -fL --retry 3 --retry-delay 2 -o "$dest" "$url" 2>/dev/null \
    && ok "$(basename "$dest")" || { warn "$(basename "$dest") 拉不到，跳过（可选文件）"; find "$dest" -maxdepth 0 -type f -delete 2>/dev/null || true; }
}
fetch_optional "$MODEL_BASE/tokenizer/tokenizer_config.json" "$DEST/tokenizer/tokenizer_config.json"

info "model/ 共 $(human "$(dirsize "$DEST")")"
