#!/bin/bash
# 准备 macOS -> linux/amd64 的交叉编译环境。
#   - brew 的 x86_64-unknown-linux-gnu-gcc/g++（gcc 11.2，sysroot glibc 2.17）
#   - rustc/cargo 用 rustup 装的官方 toolchain（brew 的 rustc 和官方 std 不兼容，见下）
#   - x86_64-unknown-linux-gnu 的 rust-std：镜像源常常没有，直接从 static.rust-lang.org 补
source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

step "10/80 检查交叉工具链"

[ "$(uname -s)" = "Darwin" ] || die "这个脚本假定在 macOS 上交叉编译；其它平台请自己准备 $RUST_TARGET 工具链"

need_cmd cargo
need_cmd curl
need_cmd shasum

# --- brew 交叉 gcc/g++ ---
for t in "$CROSS_PREFIX-gcc" "$CROSS_PREFIX-g++"; do
  if ! command -v "$t" >/dev/null 2>&1; then
    warn "$t 没装"
    info "装它：brew install messense/macos-cross-toolchains/$t"
    exit 1
  fi
done
ok "$("$CROSS_PREFIX-g++" --version | head -1)"

# --- 官方 toolchain（不要用 brew 的 rustc：它编出来的 std 跟 rustup 的对不上，
#     会报 E0514 "found crate compiled by an incompatible version"）---
RUST_BIN_DIR="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin"
[ -x "$RUST_BIN_DIR/cargo" ] || die "找不到 $RUST_BIN_DIR/cargo，先跑 rustup toolchain install stable"
export PATH="$RUST_BIN_DIR:$PATH"
RUSTC_VERSION=$(rustc --version | awk '{print $2}')
ok "$(rustc --version)   (用 $RUST_BIN_DIR)"

# --- rust-std for the linux target ---
STD_DIR="$(rustc --print sysroot)/lib/rustlib/$RUST_TARGET"
if [ -d "$STD_DIR/lib" ] && [ -n "$(ls -A "$STD_DIR/lib" 2>/dev/null)" ]; then
  ok "rust-std for $RUST_TARGET 已就位"
else
  warn "缺 $RUST_TARGET 的 rust-std（镜像源基本都没有这个 target）"
  info "从 static.rust-lang.org 手动补 $RUST_VERSION"
  mkdir -p "$TOOLS"
  TARBALL="$TOOLS/rust-std-$RUST_VERSION-$RUST_TARGET.tar.xz"
  if [ ! -f "$TARBALL" ]; then
    curl -fL --retry 5 --retry-delay 3 -o "$TARBALL" \
      "https://static.rust-lang.org/dist/rust-std-$RUST_VERSION-$RUST_TARGET.tar.xz" \
      || die "下载 rust-std 失败"
  fi
  EXTRACT="$TOOLS/rust-std-extract"
  mkdir -p "$EXTRACT"
  tar xf "$TARBALL" -C "$EXTRACT"
  SRC=$(find "$EXTRACT" -maxdepth 1 -type d -name 'rust-std-*' | head -1)
  [ -n "$SRC" ] || die "解压后找不到 rust-std-* 目录"
  mkdir -p "$STD_DIR"
  cp -R "$SRC/lib/rustlib/$RUST_TARGET/." "$STD_DIR/"
  ok "rust-std for $RUST_TARGET 装好了 -> $STD_DIR"
fi

# --- 写 C++ wrapper ---
# esaxx-rs 的 build.rs 用 `target_os == "macos"` 判断，而 build.rs 是给 host 编译的，
# 于是在 macOS 上交叉编 Linux 时它会给 Linux 的 g++ 塞一个 -stdlib=libc++，直接编译失败。
# 这里把该参数剔掉，其他原样转发。
CXX_WRAP="$TOOLS/cxx-wrap.sh"
mkdir -p "$TOOLS"
cat > "$CXX_WRAP" <<WRAP
#!/bin/bash
# 由 scripts/linux-dist/steps/10-toolchain.sh 生成
# 过滤掉 -stdlib=libc++（那是给 macOS 的 clang 用的）
args=()
for a in "\$@"; do
  [[ "\$a" == "-stdlib=libc++" ]] && continue
  args+=("\$a")
done
exec $(command -v "$CROSS_PREFIX-g++") "\${args[@]}"
WRAP
chmod +x "$CXX_WRAP"
ok "CXX wrapper -> $CXX_WRAP"

export LAYA_CXX_WRAP="$CXX_WRAP"
printf 'export LAYA_CXX_WRAP=%s\n' "$CXX_WRAP" > "$WORK/.cxx-wrap-path"
