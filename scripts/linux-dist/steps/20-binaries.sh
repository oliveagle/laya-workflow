#!/bin/bash
# 交叉编译两个 ELF：laya-workflow（纯 rust）和 laya-tch（rust + tch，动态链 libtorch）。
# 依赖 30-libtorch.sh 已经把 libtorch 放到 $WORK/libtorch（LIBTORCH 指向它）。
source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

step "20/80 交叉编译 linux/amd64 二进制"

[ -d "$WORK/libtorch/lib" ] || die "没有 $WORK/libtorch/lib，先跑 ./build.sh libtorch"

# 用 rustup 的官方 cargo/rustc，不要 brew 的
RUST_BIN_DIR="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin"
export PATH="$RUST_BIN_DIR:$PATH"

# 两种环境变量的大小写规则不一样，别弄反：
#   cargo  要 CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER（全大写，- 变 _）
#   cc-rs  要 CXX_x86_64_unknown_linux_gnu        （全小写，- 变 _）
TRIPLE_LC=$(printf '%s' "$RUST_TARGET" | tr -- '-' '_')
TRIPLE_UC=$(printf '%s' "$TRIPLE_LC" | tr '[:lower:]' '[:upper:]')
export "CARGO_TARGET_${TRIPLE_UC}_LINKER=$CROSS_PREFIX-gcc"
export "CXX_${TRIPLE_LC}=${LAYA_CXX_WRAP:-$WORK/toolchain/cxx-wrap.sh}"
export "CC_${TRIPLE_LC}=$CROSS_PREFIX-gcc"
info "CARGO_TARGET_${TRIPLE_UC}_LINKER=$CROSS_PREFIX-gcc"
info "CXX_${TRIPLE_LC}=${LAYA_CXX_WRAP:-$WORK/toolchain/cxx-wrap.sh}"
export LIBTORCH="$WORK/libtorch"
export LIBTORCH_CXX11_ABI=1        # libtorch 2.x 的 linux 轮子就是 cxx11 ABI

# brew 的交叉 sysroot 是 glibc 2.17，解析不了 libtorch 需要的 GLIBC_2.27/2.28 符号。
# 这些符号在运行机（glibc>=2.28）上都有，是 shared lib 的未定义引用，链接期放宽即可。
export RUSTFLAGS="-C link-arg=-Wl,--allow-shlib-undefined"

cd "$REPO_ROOT"

# laya-workflow：官方 release 里有（在真 Ubuntu 上用 --locked 编的，比 macOS 交叉编译可信），
# 就用官方的；只有拿不到（离线 / tag 上还没发 / 用户指定）才退回自己交叉编译。
#
# 但 laya-tch release 里没有 —— release.yml 第 36 行写死了 `-p laya-workflow`，
# laya-tch 是独立 crate，那个工作流根本没编它。所以这个还是必须自己编。
RELEASE_BIN=$WORK/release-bin
mkdir -p "$RELEASE_BIN"
RELEASE_TARBALL=laya-workflow-$RUST_TARGET.tar.gz
USE_RELEASE=0

if [ "${LAYA_USE_RELEASE_BIN:-1}" = "1" ]; then
  URL="$GITHUB_RELEASE_BASE/$RELEASE_TARBALL"
  info "取官方 release: $URL"
  if curl -fL --retry 3 --retry-delay 3 --progress-bar -o "$RELEASE_BIN/$RELEASE_TARBALL" "$URL" 2>/dev/null; then
    got=$(filesize "$RELEASE_BIN/$RELEASE_TARBALL")
    if [ "$got" = "$RELEASE_LAYA_WORKFLOW_BYTES" ]; then
      tar xzf "$RELEASE_BIN/$RELEASE_TARBALL" -C "$RELEASE_BIN"
      [ -f "$RELEASE_BIN/laya-workflow" ] || die "release tarball 里没有 laya-workflow"
      chmod 755 "$RELEASE_BIN/laya-workflow"
      file "$RELEASE_BIN/laya-workflow" | grep -qE 'ELF 64-bit.*x86-64' \
        || die "release 里的 laya-workflow 不是 x86-64 ELF"
      USE_RELEASE=1
      ok "用官方 v$PKG_VERSION 的 laya-workflow（$(human "$got")，免去交叉编译）"
    else
      warn "release 包大小对不上（期望 ${RELEASE_LAYA_WORKFLOW_BYTES}，实际 ${got}）—— 回退交叉编译"
    fi
  else
    warn "拿不到官方 release（离线？）—— 回退交叉编译"
  fi
else
  info "LAYA_USE_RELEASE_BIN=0，强制自己交叉编译 laya-workflow"
fi

if [ "$USE_RELEASE" = "1" ]; then
  info "编译 laya-tch ...（release 里没有，只能自己编）"
  cargo build --release --locked -p laya-tch --bin laya-tch --target "$RUST_TARGET"
else
  info "编译 laya-workflow ..."
  cargo build --release --locked --bin laya-workflow --target "$RUST_TARGET"
  info "编译 laya-tch ..."
  cargo build --release --locked -p laya-tch --bin laya-tch --target "$RUST_TARGET"
fi

# 官方那个（如果用了）盖在 cargo 产物上面，下面的循环统一从 $RUST_TARGET_DIR 取
[ "$USE_RELEASE" = "1" ] && cp -f "$RELEASE_BIN/laya-workflow" "$RUST_TARGET_DIR/laya-workflow"

for b in laya-workflow laya-tch; do
  f="$RUST_TARGET_DIR/$b"
  [ -f "$f" ] || die "没编出 $f"
  file "$f" > /tmp/.laya-file.$$ ; grep -qE 'ELF 64-bit.*x86-64' /tmp/.laya-file.$$ \
    || die "$b 不是 x86-64 ELF: $(cat /tmp/.laya-file.$$)"
  find /tmp -maxdepth 1 -name '.laya-file.*' -delete
  ok "$b  $(filesize "$f") B  ($(human "$(filesize "$f")"))  $(shasum -a 256 "$f" | cut -c1-16)…"
done

info "laya-tch 依赖："
"$CROSS_PREFIX-objdump" -p "$RUST_TARGET_DIR/laya-tch" | awk '/NEEDED/{printf "    %s\n", $2}'
