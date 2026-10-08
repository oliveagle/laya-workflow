#!/bin/bash
# laya Linux 离线包 —— 打包驱动。
#
#   ./build.sh                 跑全部（toolchain → libtorch → model → sqlite → binaries → plugins → stage → archive）
#   ./build.sh libtorch model  只跑指定步骤
#   ./build.sh --from stage    从某步开始往下跑
#   ./build.sh --list          看步骤
#   ./build.sh clean           清掉 staging 和产物（不动下载缓存）
#   ./build.sh clean --all     连下载缓存（libtorch/模型 ~1 GB）一起清
#
# 依赖：macOS + brew 交叉 gcc + rustup stable + docker。详见 README.md。
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

ALL_STEPS=(10-toolchain 20-binaries 30-libtorch 40-model 50-sqlite3 60-plugins 70-stage 80-archive)
# 依赖顺序（--from 按这个顺序走）：工具链 → libtorch → 二进制 → 其余
ORDER=(10-toolchain 30-libtorch 20-binaries 40-model 50-sqlite3 60-plugins 70-stage 80-archive)
ALIASES=(toolchain binaries libtorch model sqlite plugins stage archive)

usage() {
  sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
  echo
  echo "步骤（按依赖顺序）:"
  for s in "${ORDER[@]}"; do echo "  $s"; done
  echo
  echo "环境变量:"
  echo "  LAYA_WORK=$WORK"
  echo "  LAYA_DIST_OUT=$DIST_OUT"
  echo "  LAYA_EXTRA_PLUGIN_DIR=$EXTRA_PLUGIN_DIR   (设 /nonexistent 只要仓库可复现的插件)"
  echo "  LAYA_CONTAINER_IMAGE=$CONTAINER_IMAGE"
}

normalize() { # 别名 -> steps 文件前缀
  case "$1" in
    toolchain) echo 10-toolchain ;; binaries) echo 20-binaries ;; libtorch) echo 30-libtorch ;;
    model)     echo 40-model ;;     sqlite)    echo 50-sqlite3 ;;  plugins) echo 60-plugins ;;
    stage)     echo 70-stage ;;     archive)   echo 80-archive ;;
    *)         echo "$1" ;;
  esac
}

# 扫一遍所有脚本，找 $VAR 后面直接紧跟全角括号的写法 —— 在某些 locale 下 bash 会把高位字节
# 吃进变量名，$VAR 就静默展开成空串，脚本照跑不报错但行为全错。宁可构建期炸掉。
lint_locale_vars() {
  local f hits
  hits=$(LC_ALL=C grep -anE '\$[A-Za-z_][A-Za-z0-9_]*[^ -~]' "$SCRIPT_DIR"/*.sh \
          "$SCRIPT_DIR"/steps/*.sh "$SCRIPT_DIR"/payload/main/install.sh \
          "$SCRIPT_DIR"/payload/plugins/install.sh 2>/dev/null || true)
  # 上面的 grep 自身定义里就有这种写法，排除掉（只留 file:line: 后半段能判定的）
  hits=$(printf '%s\n' "$hits" | grep -v 'lint_locale_vars' | grep -v 'grep -anE' || true)
  if [ -n "$hits" ]; then
    warn "疑似把非 ASCII 字符紧跟在 \$变量 后面（会被 bash 当成变量名的一部分）："
    printf '%s\n' "$hits" | sed 's/^/    /'
    die "修掉上面这些再跑：变量引用要写成 \${VAR}，后面用空格/标点隔开"
  fi
  ok "locale 守卫：没发现会被误吞进变量名的写法"
}

do_clean() {
  step "清理"
  for d in "$WORK/stage" "$WORK/plugin-harvest" "$WORK/plugin-merged" "$WORK/libtorch-extract" "$WORK/sqlite-src"; do
    [ -d "$d" ] && { find "$d" -mindepth 1 -delete 2>/dev/null || true; info "清了 $d"; }
  done
  if [ -d "$DIST_OUT" ]; then
    find "$DIST_OUT" -maxdepth 1 -type f -name 'laya-*' -delete
    find "$DIST_OUT" -maxdepth 1 -type f \( -name 'SHA256SUMS' -o -name 'MANIFEST.md' \) -delete
    info "清了 $DIST_OUT 里的产物"
  fi
  if [ "${1:-}" = "--all" ]; then
    for d in "$WORK/libtorch" "$WORK/model" "$WORK/download" "$WORK/toolchain" "$WORK/sqlite3"; do
      [ -e "$d" ] && { find "$d" -mindepth 0 -delete 2>/dev/null || true; info "清了 ${d}（要重新下载）"; }
    done
  else
    info "下载缓存保留（要一起清：./build.sh clean --all）"
  fi
}

main() {
  local -a want=()
  local from="" do_clean_it=""
  while [ $# -gt 0 ]; do
    case "$1" in
      -h|--help) usage; exit 0 ;;
      --list)    printf '%s\n' "${ORDER[@]}"; exit 0 ;;
      clean)     do_clean_it=1; shift ;;
      --from)    from=$(normalize "$2"); shift 2 ;;
      --only)    want=("$(normalize "$2")"); shift 2 ;;
      -*)        die "未知参数: $1" ;;
      *)         want+=("$(normalize "$1")"); shift ;;
    esac
  done

  [ -n "$do_clean_it" ] && { do_clean "$@"; return 0; }

  if [ -n "$from" ]; then
    local seen=0
    for s in "${ORDER[@]}"; do
      [ "$s" = "$from" ] && seen=1
      [ "$seen" -eq 1 ] && want+=("$s")
    done
    [ "$seen" -eq 1 ] || die "--from $from 不是已知步骤"
  fi
  [ ${#want[@]} -gt 0 ] || want=("${ORDER[@]}")

  lint_locale_vars
  mkdir -p "$WORK" "$DIST_OUT"
  step "laya linux 离线包 $PKG_VERSION —— 打包"
  info "工作目录 $WORK"
  info "产物目录 $DIST_OUT"
  info "要跑 ${#want[@]} 步: ${want[*]}"
  echo

  local t0=$SECONDS
  for s in "${want[@]}"; do
    local f="$SCRIPT_DIR/steps/$s.sh"
    [ -f "$f" ] || die "步骤文件不存在: $f"
    bash "$f"
  done

  echo
  step "全部完成（$((SECONDS - t0))s）"
  info "下一步：./verify.sh"
}
main "$@"
