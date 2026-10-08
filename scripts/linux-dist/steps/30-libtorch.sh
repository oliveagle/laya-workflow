#!/bin/bash
# 下载 PyTorch CPU 运行库（linux x86_64），只保留运行必需的 lib/。
# 解压出来 479 MB，但 include/ + share/ 用不到，剔掉后 417 MB。
source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

step "30/80 下载 libtorch $LIBTORCH_VERSION+cpu"

DEST=$WORK/libtorch
if [ -d "$DEST/lib" ] && [ -f "$DEST/lib/libtorch_cpu.so" ]; then
  ok "已有 $DEST/lib"
else
  mkdir -p "$DL"
  ZIP="$DL/libtorch-$LIBTORCH_VERSION-cpu.zip"
  if [ ! -f "$ZIP" ] || [ "$(filesize "$ZIP")" != "$LIBTORCH_ZIP_BYTES" ]; then
    info "GET $LIBTORCH_URL"
    curl -fL --retry 5 --retry-delay 3 --progress-bar -o "$ZIP" "$LIBTORCH_URL" \
      || die "下载失败"
  fi
  got=$(filesize "$ZIP")
  [ "$got" = "$LIBTORCH_ZIP_BYTES" ] || die "zip 大小不对：期望 ${LIBTORCH_ZIP_BYTES}，实际 $got"
  ok "zip $(human "$got")"

  EX=$WORK/libtorch-extract
  mkdir -p "$EX"
  info "解压 ..."
  unzip -q -o "$ZIP" -d "$EX"
  SRC=$(find "$EX" -maxdepth 1 -type d -name 'libtorch' | head -1)
  [ -n "$SRC" ] || die "解压后没找到 libtorch/ 目录"
  mkdir -p "$DEST"
  cp -R "$SRC/lib" "$DEST/"
  ok "lib/ -> $DEST/lib"
fi

# 上游 .so 逐个核对 sha256：换镜像/换版本时能立刻发现
info "核对 libtorch .so ..."
verify_shas "$DEST/lib" "$LIBTORCH_SO_SHA256"

# ABI 自检：cxx11 的符号里有 NSt7__cxx11
N_CXX11=$(nm -D --defined-only "$DEST/lib/libc10.so" 2>/dev/null | count_grep '__cxx11')
if [ "${N_CXX11:-0}" -gt 0 ]; then
  ok "ABI = cxx11（$N_CXX11 个 __cxx11 符号，与 laya-tch 链接时一致）"
else
  warn "libc10.so 里 0 个 __cxx11 符号 —— 这可能不是 laya-tch 链接过的那份 libtorch"
fi

info "版本标记: $(cat "$DEST/build-version" 2>/dev/null || echo '?')"
info "lib/ 共 $(human "$(dirsize "$DEST/lib")")"
