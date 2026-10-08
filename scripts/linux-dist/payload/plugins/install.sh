#!/bin/sh
# laya 插件包安装（配合 laya-linux-cpu-x86_64 使用，也可以单独用）
#
#   ./install.sh                装到 ~/.laya-workflow
#   ./install.sh --state DIR    装到别的状态目录
#   ./install.sh --force        覆盖已存在的同名插件
#   ./install.sh --uninstall    删掉本包装的插件和 laya-mem specs
set -e

PKG_ROOT=$(cd "$(dirname "$0")" && pwd)
STATE="${LAYA_HOME:-$HOME/.laya-workflow}"
FORCE=0
UNINSTALL=0

while [ $# -gt 0 ]; do
  case "$1" in
    --state) STATE="$2"; shift 2 ;;
    --state=*) STATE="${1#*=}"; shift ;;
    --force) FORCE=1; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help) sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "未知参数: $1" >&2; exit 2 ;;
  esac
done

PLUGINS="$STATE/plugins"
SPECS="$STATE/laya-mem/specs"

if [ "$UNINSTALL" -eq 1 ]; then
  n=0
  for d in "$PKG_ROOT"/payload/plugins/*/; do
    name=$(basename "$d")
    if [ -d "$PLUGINS/$name" ]; then rm -rf "$PLUGINS/$name"; n=$((n+1)); fi
  done
  for f in "$PKG_ROOT"/payload/laya-mem/specs/*.json; do
    name=$(basename "$f")
    [ -f "$SPECS/$name" ] && rm -f "$SPECS/$name"
  done
  echo "删掉了 $n 个插件（laya-mem specs 也清了）"
  exit 0
fi

mkdir -p "$PLUGINS" "$SPECS"

n=0; skip=0
for d in "$PKG_ROOT"/payload/plugins/*/; do
  name=$(basename "$d")
  if [ -d "$PLUGINS/$name" ] && [ "$FORCE" -eq 0 ]; then
    skip=$((skip+1)); continue
  fi
  rm -rf "$PLUGINS/$name"
  cp -R "$d" "$PLUGINS/$name"
  n=$((n+1))
done
echo "插件: 装了 $n 个，跳过 $skip 个（已存在；要覆盖加 --force） -> $PLUGINS"

m=0
for f in "$PKG_ROOT"/payload/laya-mem/specs/*.json; do
  name=$(basename "$f")
  if [ -f "$SPECS/$name" ] && [ "$FORCE" -eq 0 ]; then continue; fi
  cp -f "$f" "$SPECS/$name"
  m=$((m+1))
done
echo "laya-mem specs: $m 个 -> $SPECS"

echo
echo "看一眼："
echo "  LAYA_HOME=$STATE laya-workflow plugin list"
echo "  LAYA_HOME=$STATE laya-workflow laya-mem info"
