#!/bin/bash
# 端到端验证：起一个干净的 debian:bookworm-slim（amd64），从**实际产物 zip** 解压，
# 安装 → 起引擎 → 真模型决策 → 插件 → 卸载。任何一步失败退出码非 0。
#
#   ./verify.sh                 用 $DIST_OUT 里的默认文件名
#   ./verify.sh /path/to/x.zip  指定主包
#   ./verify.sh --keep          验证完保留容器（调试用），默认自动销毁
#   ./verify.sh --shell         验证完开个 shell 进容器
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

ZIP=""
PLUGZIP=""
TARGZ=""
KEEP=0
SHELL_AFTER=0
while [ $# -gt 0 ]; do
  case "$1" in
    --keep)  KEEP=1; shift ;;
    --shell) KEEP=1; SHELL_AFTER=1; shift ;;
    -*) die "未知参数: $1" ;;
    *)  ZIP=$1; shift ;;
  esac
done
ZIP=${ZIP:-$DIST_OUT/$PKG_NAME-$PKG_VERSION.zip}
PLUGZIP=${PLUGZIP:-$DIST_OUT/$PLUGIN_PKG_NAME-$PKG_VERSION.zip}
# tar.gz 是可选的：没打就跳过那步，但打了一定要验（AppleDouble 只有它会中招）
[ -f "$DIST_OUT/$PKG_NAME-$PKG_VERSION.tar.gz" ] && TARGZ="$DIST_OUT/$PKG_NAME-$PKG_VERSION.tar.gz"

[ -f "$ZIP" ]     || die "找不到主包: ${ZIP}（先 ./build.sh）"
[ -f "$PLUGZIP" ] || warn "找不到插件包: ${PLUGZIP}（插件那几步会判失败）"
[ -n "$TARGZ" ] && [ -f "$TARGZ" ] || TARGZ=""

# 容器里看到的是挂载后的路径，不是宿主机的绝对路径。
# 产物在 $DIST_OUT 下就映射成 /dist/<basename>；不在就统一软链到 /pkg。
EXTRA_MOUNT=0
container_path() { # 宿主路径 -> 容器路径
  if [ "$EXTRA_MOUNT" -eq 1 ]; then
    case "$1" in
      "$ZIP")     echo "/pkg/main.zip" ;;
      "$PLUGZIP") echo "/pkg/plugins.zip" ;;
      "$TARGZ")   echo "/pkg/main.tar.gz" ;;
      *)          echo "/pkg/$(basename "$1")" ;;
    esac
  else
    echo "/dist/${1#$DIST_OUT/}"
  fi
}
for f in "$ZIP" "$PLUGZIP" ${TARGZ:+"$TARGZ"}; do
  case "$f" in "$DIST_OUT"/*) ;; *) EXTRA_MOUNT=1 ;; esac
done
ZIP_IN=$(container_path "$ZIP")
PLUGZIP_IN=$(container_path "$PLUGZIP")
TARGZ_IN=$([ -n "$TARGZ" ] && container_path "$TARGZ" || echo "")

command -v docker >/dev/null || die "没装 docker"

step "校验产物 sha256"
if [ -f "$DIST_OUT/SHA256SUMS" ]; then
  ( cd "$DIST_OUT" && shasum -a 256 -c SHA256SUMS 2>&1 | sed 's/^/    /' ) \
    && ok "SHA256SUMS 全对" || bad "SHA256 对不上"
else
  warn "没有 SHA256SUMS，跳过"
fi

step "起容器 $CONTAINER_IMAGE ($DOCKER_PLATFORM)"
info "挂载: $DIST_OUT -> /dist"
docker info >/dev/null 2>&1 || die "docker daemon 没起来"

NAME=laya-verify-$$
# 只在被打断时兜底销毁。正常结束时的销毁/保留在脚本末尾显式决定 ——
# 之前挂在 EXIT trap 上，结果打印了「容器保留」容器却还是被删了，行为不可预测。
trap 'docker rm -f "$NAME" >/dev/null 2>&1 || true' INT TERM

EXTRA_ARGS=()
if [ "$EXTRA_MOUNT" -eq 1 ]; then
  # 至少有一个包不在 $DIST_OUT 下：软链到一个统一目录再挂进去，省得挂两处
  STAGE_MNT=/tmp/laya-verify-mount
  find "$STAGE_MNT" -mindepth 1 -delete 2>/dev/null || true
  mkdir -p "$STAGE_MNT"
  ln -sf "$ZIP" "$STAGE_MNT/main.zip"
  [ -f "$PLUGZIP" ] && ln -sf "$PLUGZIP" "$STAGE_MNT/plugins.zip"
  [ -n "$TARGZ" ] && ln -sf "$TARGZ" "$STAGE_MNT/main.tar.gz"
  EXTRA_ARGS=(-v "$STAGE_MNT:/pkg:ro")
  info "额外挂载: $STAGE_MNT -> /pkg"
fi

# bash 3.2（macOS 自带）+ set -u 下展开空数组会报 unbound variable，
# 所以用 ${arr[@]+"${arr[@]}"} 这种写法。
docker run -d --name "$NAME" --platform "$DOCKER_PLATFORM" \
  -v "$DIST_OUT:/dist:ro" \
  -v "$SCRIPT_DIR:/scripts:ro" \
  ${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"} \
  "$CONTAINER_IMAGE" sleep infinity >/dev/null || die "容器起不来"
ok "容器 $NAME"
info "主包   $ZIP_IN"
info "插件包 $PLUGZIP_IN"
[ -n "$TARGZ_IN" ] && info "tar.gz $TARGZ_IN"

# 把 verify-container.sh 拷进去再跑（/scripts 是 ro，直接 bash 也行，但日志留一份在容器里）
docker cp "$SCRIPT_DIR/verify-container.sh" "$NAME:/tmp/verify-container.sh" >/dev/null
docker exec "$NAME" chmod +x /tmp/verify-container.sh

step "跑验证"
set +e
docker exec "$NAME" /tmp/verify-container.sh "$ZIP_IN" "$PLUGZIP_IN" "$TARGZ_IN"
RC=$?
set -e

echo
if [ $RC -eq 0 ] && [ "$KEEP" -eq 0 ]; then
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  step "验证通过 ✓  （容器已销毁）"
elif [ $RC -eq 0 ]; then
  step "验证通过 ✓"
  info "容器保留: ${NAME}（docker rm -f ${NAME} 清掉）"
  [ "$SHELL_AFTER" -eq 1 ] && docker exec -it "$NAME" bash
else
  step "验证失败 ✗"
  KEEP=1   # 失败一律保留，除非调用方要求销毁
  info "容器保留: $NAME"
  info "进去看: docker exec -it $NAME bash"
  info "  里面：/tmp/verify-container.sh 是验证脚本，/work/verify/ 是解压出来的包"
  [ "$SHELL_AFTER" -eq 1 ] && docker exec -it "$NAME" bash
fi
exit $RC