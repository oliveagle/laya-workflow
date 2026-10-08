#!/bin/bash
# linux-dist 公共配置与工具函数。所有 steps/*.sh 都 source 这个文件。
# 改版本号 / URL / 校验和，只改这里。

set -euo pipefail

# ---------------------------------------------------------------- 路径
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/../.." && pwd)
# 中间产物（可缓存，几 GB）。想换：export LAYA_WORK=/your/path
WORK=${LAYA_WORK:-/tmp/laya-linux-dist}
# 最终 4 个产物放这里
DIST_OUT=${LAYA_DIST_OUT:-$HOME/ole/dist/laya}
# 额外插件来源（本机 LAYA_HOME 之类）。见 README「插件从哪来」
EXTRA_PLUGIN_DIR=${LAYA_EXTRA_PLUGIN_DIR:-$HOME/.laya-workflow/plugins}

# ---------------------------------------------------------------- 版本 / 外部依赖
PKG_VERSION=0.9.0                    # 跟 Cargo.toml workspace.version 一致
PKG_NAME=laya-linux-cpu-x86_64
PLUGIN_PKG_NAME=laya-plugins

RUST_TARGET=x86_64-unknown-linux-gnu # 只支持这个（README 里有说明）
CROSS_PREFIX=x86_64-unknown-linux-gnu  # brew 的交叉工具链前缀

LIBTORCH_VERSION=2.13.0
# 注意：文件名里没有 cxx11-abi，但 linux 轮子实际就是 cxx11 ABI
# （libc10.so 里有 _ZNSt7__cxx1112... 符号；30-libtorch.sh 会验）
LIBTORCH_URL=https://download.pytorch.org/libtorch/cpu/libtorch-shared-with-deps-2.13.0%2Bcpu.zip
LIBTORCH_ZIP_BYTES=126385248
# 上游 .so 的 sha256：下载解压后逐个核对，防止换镜像/换版本悄悄混进来
LIBTORCH_SO_SHA256="55de3057c8866e30d3fe56e4c4554860d5bf85c37b8ae6e2c81eea5c00d0ec3c  libtorch_cpu.so
52d951ec184bf4ead929cfa5a24daa14cc617bf9e2180b6e551434b6f11d0b3f  libc10.so
28b3d3926e0674eda7dcdf26c27dfd05cb455d681d11cb84fdbf7e0df72f3f7b  libtorch.so"

MODEL_REPO=convaiinnovations/laya
MODEL_REV=main
MODEL_SAFETENSORS_BYTES=842609210   # root 变体（typed-decisions，英文）
MODEL_BASE=https://huggingface.co/$MODEL_REPO/resolve/$MODEL_REV

SQLITE_YEAR=2026
SQLITE_NUMBER=3530400                # 3.53.4
SQLITE_URL=https://www.sqlite.org/$SQLITE_YEAR/sqlite-amalgamation-$SQLITE_NUMBER.zip
SQLITE_ZIP_BYTES=2946650

# 验证 / 编 sqlite3 / 收插件用的容器
CONTAINER_IMAGE=${LAYA_CONTAINER_IMAGE:-debian:bookworm-slim}
DOCKER_PLATFORM=linux/amd64

# ---------------------------------------------------------------- 派生路径
STAGE_MAIN=$WORK/stage/$PKG_NAME-$PKG_VERSION
STAGE_PLUGINS=$WORK/stage/$PLUGIN_PKG_NAME-$PKG_VERSION
DL=$WORK/download
TOOLS=$WORK/toolchain
RUST_TARGET_DIR=$REPO_ROOT/target/$RUST_TARGET/release

# ---------------------------------------------------------------- 工具函数
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  C_R=$'\033[31m'; C_G=$'\033[32m'; C_Y=$'\033[33m'; C_B=$'\033[36m'; C_0=$'\033[0m'
else
  C_R=; C_G=; C_Y=; C_B=; C_0=
fi

step()  { printf '\n%s==> %s%s\n' "$C_B" "$*" "$C_0"; }
info()  { printf '    %s\n' "$*"; }
ok()    { printf '    %s✓ %s%s\n' "$C_G" "$*" "$C_0"; }
warn()  { printf '    %s! %s%s\n' "$C_Y" "$*" "$C_0"; }
die()   { printf '\n%s✗ %s%s\n' "$C_R" "$*" "$C_0" >&2; exit 1; }

# 人类可读体积
human() {
  local b=${1:-0}
  if [ "$b" -ge 1073741824 ]; then awk -v b="$b" 'BEGIN{printf "%.2f GB", b/1073741824}'
  elif [ "$b" -ge 1048576 ]; then awk -v b="$b" 'BEGIN{printf "%.1f MB", b/1048576}'
  elif [ "$b" -ge 1024 ]; then awk -v b="$b" 'BEGIN{printf "%.1f KB", b/1024}'
  else printf '%s B' "$b"; fi
}

# 文件大小（跨平台，stat 参数不一样）
filesize() {
  if stat -f%z "$1" >/dev/null 2>&1; then stat -f%z "$1"; else stat -c%s "$1"; fi
}

# 目录总体积（字节）
dirsize() { du -sk "$1" 2>/dev/null | awk '{print $1*1024}'; }

# 校验 sha256（对着 "hash  name" 的清单）
verify_shas() {
  local dir=$1 list=$2 line name got
  echo "$list" | while read -r want name; do
    [ -n "${name:-}" ] || continue
    [ -f "$dir/$name" ] || die "缺文件: $dir/$name"
    got=$(shasum -a 256 "$dir/$name" | awk '{print $1}')
    [ "$got" = "$want" ] || die "sha256 不匹配: $name
  期望 $want
  实际 $got"
    ok "sha256 $name"
  done
}

need_cmd() { command -v "$1" >/dev/null 2>&1 || die "缺命令: $1（$(brew info "$1" 2>/dev/null | head -1 || echo '装一下')）"; }

# 注意：不要写 `producer | grep -q x` —— pipefail 下 grep -q 命中就退出，
# producer 收到 SIGPIPE 返回 141，整个管道被判成失败，条件就反了。
# 要用 has() / count_grep()。
has() { grep -qE "$1" || return 1; }          # 从 stdin 读，调用方自己重定向
count_grep() { grep -cE "$1" || true; }       # grep -c 读完整个流，不 SIGPIPE

# docker 跑一次性命令（自动挂载 work 目录、强制 amd64）
docker_run() {
  local image=$1; shift
  docker run --rm --platform "$DOCKER_PLATFORM" \
    -v "$WORK:/work" -v "$DIST_OUT:/dist" \
    -v "$SCRIPT_DIR:/scripts:ro" \
    "$image" "$@"
}

# 归档时排除 macOS 垃圾
ZIP_EXCL=(-x '*.DS_Store' -x '__MACOSX/*' -x '*/.DS_Store')
