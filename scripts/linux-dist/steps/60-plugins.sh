#!/bin/bash
# 收集插件包内容。插件有三个来源，脚本会按顺序叠加并打印每个来源贡献了什么：
#
#   1. 二进制内置     17 个  —— 在容器里跑 linux 版 laya-workflow install 收出来
#   2. 仓库 plugins/   5 个   —— 其中 4 个和内置重复，jev-planner 是仓库独有的
#   3. --extra 目录    n 个   —— 本机 LAYA_HOME 里的额外插件（默认 ~/.laya-workflow/plugins）
#
# 注意：来源 3 不是仓库内容，重建时结果可能不同。想只要仓库可复现的，就
#   LAYA_EXTRA_PLUGIN_DIR=/nonexistent ./build.sh plugins
source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

step "60/80 收集插件 + laya-mem specs"

BIN="$RUST_TARGET_DIR/laya-workflow"
[ -f "$BIN" ] || die "没有 ${BIN}，先跑 ./build.sh binaries"

HARVEST=$WORK/plugin-harvest
mkdir -p "$HARVEST/bin" "$HARVEST/home"
cp "$BIN" "$HARVEST/bin/laya-workflow"

# --- 1. 二进制内置 ---
info "在容器里跑 laya-workflow install ..."
docker_run "$CONTAINER_IMAGE" bash -c '
  set -e
  chmod +x /work/plugin-harvest/bin/laya-workflow
  LAYA_HOME=/work/plugin-harvest/home /work/plugin-harvest/bin/laya-workflow install 2>&1 | tail -3
'
N_BUILTIN=$(ls -1 "$HARVEST/home/plugins" 2>/dev/null | wc -l | tr -d ' ')
ok "二进制内置 $N_BUILTIN 个"
[ "$N_BUILTIN" -gt 0 ] || die "没收到插件"

# --- 2/3. 叠加其它来源 ---
MERGED=$WORK/plugin-merged
if [ -d "$MERGED" ]; then find "$MERGED" -mindepth 1 -delete; else mkdir -p "$MERGED"; fi
cp -R "$HARVEST/home/plugins/." "$MERGED/"

overlay() { # 目录 标签
  local src=$1 label=$2 added=0 name
  [ -d "$src" ] || { warn "$label 不存在，跳过: $src"; return 0; }
  for d in "$src"/*/; do
    [ -d "$d" ] || continue
    name=$(basename "$d")
    if [ -d "$MERGED/$name" ]; then continue; fi   # 内置的优先，不覆盖
    cp -R "$d" "$MERGED/$name"; added=$((added+1))
    info "  + $name   ($label)"
  done
  [ "$added" -gt 0 ] && ok "$label 新增 $added 个" || info "  ($label 没有新插件)"
  return 0
}
overlay "$REPO_ROOT/plugins" "仓库 plugins/"
if [ "$EXTRA_PLUGIN_DIR" = "/nonexistent" ] || [ -z "$EXTRA_PLUGIN_DIR" ]; then
  info "额外目录已禁用（只要仓库可复现的那份插件）"
elif [ -d "$EXTRA_PLUGIN_DIR" ]; then
  overlay "$EXTRA_PLUGIN_DIR" "额外目录 $EXTRA_PLUGIN_DIR"
  warn "来源 3 不是仓库内容：换台机器重建插件集合可能不同（要可复现：LAYA_EXTRA_PLUGIN_DIR=/nonexistent）"
else
  info "额外目录不存在，跳过: $EXTRA_PLUGIN_DIR"
fi

N_PLUGINS=$(ls -1 "$MERGED" | wc -l | tr -d ' ')
N_SPECS=$(ls -1 "$HARVEST/home/laya-mem/specs" 2>/dev/null | wc -l | tr -d ' ')
ok "插件合计 $N_PLUGINS 个，laya-mem specs $N_SPECS 个"
printf 'plugins=%s specs=%s\n' "$N_PLUGINS" "$N_SPECS" > "$WORK/.plugin-counts"
