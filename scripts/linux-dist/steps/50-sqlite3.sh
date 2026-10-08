#!/bin/bash
# 静态编译一个 sqlite3 CLI（db 能力要 shell out 到它，系统不一定有）。
# 放容器里编，保证是 glibc 静态二进制，任何 dist 都能跑。
source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

step "50/80 编译 sqlite3（$CONTAINER_IMAGE 内静态编译）"

OUT=$WORK/sqlite3
if [ -f "$OUT" ]; then
  ok "已有 $OUT  $(human "$(filesize "$OUT")")"
else
  mkdir -p "$WORK/sqlite-src"
  docker_run "$CONTAINER_IMAGE" bash -s -- "$SQLITE_URL" "$SQLITE_ZIP_BYTES" <<'INNER'
set -euo pipefail
URL=$1; WANT=$2
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq --no-install-recommends gcc libc6-dev ca-certificates unzip >/dev/null 2>&1
mkdir -p /work/sqlite-src && cd /work/sqlite-src
[ -f sqlite-amalgamation.zip ] || curl -fsSL -o sqlite-amalgamation.zip "$URL"
got=$(stat -c%s sqlite-amalgamation.zip)
[ "$got" = "$WANT" ] || { echo "zip 大小不对: $got != $WANT" >&2; exit 1; }
[ -d sqlite-amalgamation-3530400 ] || unzip -q -o sqlite-amalgamation.zip
cd sqlite-amalgamation-3530400
gcc -O2 -static -o /work/sqlite3 shell.c sqlite3.c -lpthread -ldl -lm
/work/sqlite3 --version
INNER
  [ -f "$OUT" ] || die "容器里没产出 sqlite3"
  ok "sqlite3 $(human "$(filesize "$OUT")")  $(docker_run "$CONTAINER_IMAGE" /work/sqlite3 --version)"
fi
