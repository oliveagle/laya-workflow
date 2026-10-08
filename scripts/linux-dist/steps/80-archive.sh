#!/bin/bash
# 打成 4 个产物 + 写 SHA256SUMS / MANIFEST。
#   $DIST_OUT/laya-linux-cpu-x86_64-0.8.0.{zip,tar.gz}
#   $DIST_OUT/laya-plugins-0.8.0.{zip,tar.gz}
source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

step "80/80 打包"

for d in "$STAGE_MAIN" "$STAGE_PLUGINS"; do
  [ -d "$d" ] || die "没有 staging: ${d}（先跑 ./build.sh stage）"
done
mkdir -p "$DIST_OUT"
# 旧的清掉
find "$DIST_OUT" -maxdepth 1 -type f \( -name 'laya-*.zip' -o -name 'laya-*.tar.gz' \
  -o -name 'SHA256SUMS' -o -name 'MANIFEST.md' \) -delete

# macOS 的 bsdtar 会把文件上的 xattr 写进归档，两种表现形式都会坑到 Linux：
#   1) com.apple.provenance 这类 xattr 变成 pax extended header，GNU tar 解的时候刷
#      一百多条 "Ignoring unknown extended header keyword"；
#   2) AppleDouble 的 ._* 成员 —— bsdtar 自己 list 时会把它们藏起来，本机看着一切正常，
#      换到 Linux 用 GNU tar 解出来 ._* 就变成实打实的文件，spec 目录凭空多一倍
#      （98 -> 196），laya-workflow 会把它们当 spec 读。
#
# xattr -cr 是第一道保险，但对 com.apple.provenance 无效（新版 macOS 不让普通用户清），
# 所以真正的开关是打包时那三个 flag：--no-mac-metadata --no-xattrs + COPYFILE_DISABLE=1。
strip_macos_xattrs() {
  if command -v xattr >/dev/null 2>&1; then
    # 清不掉也正常，只是提前少几个；真兜底是 tar 的 --no-xattrs
    xattr -cr "$1" 2>/dev/null || true
  fi
}

# tar 成员数和 AppleDouble 检查用 python3 做：
#   1) gzip -dc | grep -c 对二进制流数行是瞎数（会得到几百万），没有意义；
#   2) bsdtar 自己 list 会把 AppleDouble 成员藏起来，正好漏掉要抓的东西。
# python 的 tarfile 两个坑都不踩。
tar_list() { # 归档路径 -> 每行一个成员名（包含 ._*）
  python3 -c 'import sys,tarfile
for m in tarfile.open(sys.argv[1]).getmembers():
    print(m.name)' "$1"
}

# 归档里出现 ._* 就是出事了，直接失败，别等人到 Linux 上才发现
assert_no_appledouble() { # 归档路径
  local n
  n=$(tar_list "$1" | grep -ac '/\._' || true)
  if [ "${n:-0}" -ne 0 ]; then
    die "归档 ${1} 里有 ${n} 个 AppleDouble ._* 成员 —— GNU tar 在 Linux 上会把它们解成实文件"
  fi
  ok "$(basename "$1")  没有 AppleDouble 成员"
}

# 期望的成员数（文件 + 目录，含包根那一层）
expect_members() { find "$WORK/stage/$1" | wc -l | tr -d ' '; }

pack_one() { # 目录名
  local name=$1 want
  want=$(expect_members "$name")
  strip_macos_xattrs "$WORK/stage/$name"

  info "zip   $name.zip"
  # -X 不写扩展字段；zip 的 -x 之后所有参数都算排除项，所以排除项必须放最后
  ( cd "$WORK/stage" && zip -r -q -y -X "$DIST_OUT/$name.zip" "$name" "${ZIP_EXCL[@]}" )

  info "tar   $name.tar.gz"
  # --no-mac-metadata / --no-xattrs 两个都必须给：只给前者 GNU tar 还是会刷 extended header 警告
  ( cd "$WORK/stage" && COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs \
      -czf "$DIST_OUT/$name.tar.gz" "$name" )

  # 归档成员数必须和 staging 里的文件+目录数一致，多一个都不行
  local nz nt
  nz=$(zipinfo -1 "$DIST_OUT/$name.zip" 2>/dev/null | wc -l | tr -d ' ')
  nt=$(tar_list "$DIST_OUT/$name.tar.gz" | grep -ac '^' || true)
  [ "${nz:-0}" = "$want" ] || die "zip 成员数 ${nz} != 期望 ${want}（${name}）"
  [ "${nt:-0}" = "$want" ] || die "tar 成员数 ${nt} != 期望 ${want}（${name}）"
  assert_no_appledouble "$DIST_OUT/$name.tar.gz"
  ok "${name}: zip ${nz} / tar ${nt} 个成员（期望 ${want}）"
}

pack_one "$PKG_NAME-$PKG_VERSION"
pack_one "$PLUGIN_PKG_NAME-$PKG_VERSION"

( cd "$DIST_OUT" && shasum -a 256 laya-*.zip laya-*.tar.gz > SHA256SUMS )

# MANIFEST.md：体积构成 + 校验和，让人一眼知道为什么这么大
MAIN_TOTAL=$(dirsize "$STAGE_MAIN")
N_PLUGIN_BUILTIN=$(ls -1 "$WORK/plugin-harvest/home/plugins" 2>/dev/null | wc -l | tr -d ' ')
# shellcheck disable=SC1091
[ -f "$WORK/.plugin-counts" ] && . "$WORK/.plugin-counts"
{
  echo "# laya 离线包 $PKG_VERSION — 产物清单"
  echo
  echo "生成于 $(date '+%Y-%m-%d %H:%M:%S %Z')，由 \`scripts/linux-dist/build.sh\` 产出。"
  echo
  echo "## 产物"
  echo
  echo "| 文件 | 大小 |"
  echo "|---|---|"
  for f in "$DIST_OUT"/laya-*.zip "$DIST_OUT"/laya-*.tar.gz; do
    printf '| `%s` | %s |\n' "$(basename "$f")" "$(human "$(filesize "$f")")"
  done
  echo
  echo "## 主包解压后体积构成（$MAIN_TOTAL B = $(human "$MAIN_TOTAL")）"
  echo
  echo "| 组成 | 大小 | 占比 | 能不能省 |"
  echo "|---|---|---|---|"
  row() { printf '| %s | %s | %s | %s |\n' "$1" "$(human "$2")" \
    "$(awk -v a="$2" -v b="$MAIN_TOTAL" 'BEGIN{printf "%.1f%%", a*100/b}')" "$3"; }
  # 注意：反引号在双引号里是命令替换，所以标签一律用单引号
  N_JSON=$(find "$STAGE_MAIN/specs" -name '*.json' | wc -l | tr -d ' ')
  row '模型 `models/laya/`'          "$(dirsize "$STAGE_MAIN/models/laya")"      '不能，这是决策模型本体'
  row 'libtorch `lib/libtorch/lib/`' "$(dirsize "$STAGE_MAIN/lib/libtorch/lib")" '不能，CPU 推理运行时'
  row '二进制 `libexec/`'             "$(dirsize "$STAGE_MAIN/libexec")"         '不能'
  row '`specs/`（'"$N_JSON"' 个 spec）' "$(dirsize "$STAGE_MAIN/specs")"           '不能，没编译进二进制'
  row '`bin/sqlite3`'                "$(filesize "$STAGE_MAIN/bin/sqlite3")"     '能，系统已有 sqlite3 就不用带'
  # bin/ 里除了 sqlite3 就只剩 wrapper 和脚本，别和上面那行重复计数
  row 'wrapper + 脚本'                "$(( $(dirsize "$STAGE_MAIN/bin") - $(filesize "$STAGE_MAIN/bin/sqlite3") ))" '不能，很小'
  echo
  echo "## 已排除（按需求）"
  echo
  echo "- duckdb —— 不在依赖里，没打进来"
  echo "- chrome / playwright —— 浏览器能力要另装"
  echo
  echo "## SHA256"
  echo
  echo '```'
  cat "$DIST_OUT/SHA256SUMS"
  echo '```'
  echo
  echo "## 组成来源"
  echo
  echo "| 组件 | 来源 |"
  echo "|---|---|"
  echo "| laya-workflow / laya-tch | 本仓库 \`cargo build --target $RUST_TARGET --release\` |"
  echo "| libtorch $LIBTORCH_VERSION+cpu | $LIBTORCH_URL |"
  echo "| 模型 | $MODEL_BASE |"
  echo "| sqlite3 3.53.4 | ${SQLITE_URL}（容器内静态编译） |"
  echo "| 插件 | 二进制内置 $N_PLUGIN_BUILTIN 个 + 仓库 \`plugins/\` |"
} > "$DIST_OUT/MANIFEST.md"

step "完成"
for f in "$DIST_OUT"/laya-*.zip "$DIST_OUT"/laya-*.tar.gz "$DIST_OUT"/SHA256SUMS "$DIST_OUT"/MANIFEST.md; do
  printf '    %-42s %10s\n' "$(basename "$f")" "$(human "$(filesize "$f")")"
done
echo
info "校验和: $DIST_OUT/SHA256SUMS"
