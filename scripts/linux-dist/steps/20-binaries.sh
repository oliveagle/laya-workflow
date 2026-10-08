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
info "编译 laya-workflow ..."
cargo build --release --locked --bin laya-workflow --target "$RUST_TARGET"
info "编译 laya-tch ..."
cargo build --release --locked -p laya-tch --bin laya-tch --target "$RUST_TARGET"

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
