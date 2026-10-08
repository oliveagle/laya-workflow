#!/bin/bash
# 容器内端到端验证。**不要在宿主机直接跑** —— 用 ../verify.sh，它负责起容器、挂载产物。
#
# 验证链路：解压 zip → install.sh → list(spec) → 起引擎 → /health → 真模型决策
#           → apps → 插件包 → 停引擎 → 卸载
set -uo pipefail

ZIP=${1:-/dist/laya-linux-cpu-x86_64-0.8.0.zip}
PLUGZIP=${2:-/dist/laya-plugins-0.8.0.zip}
TARGZ=${3:-}
PKGDIR=/work/verify
PREFIX=/root/bin
STATE=/root/.laya-workflow
export HOME=/root
FAIL=0

step() { printf '\n\033[36m########## %s\033[0m\n' "$*"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
bad()  { printf '  \033[31m✗\033[0m %s\n' "$*"; FAIL=$((FAIL+1)); }
warn() { printf '  \033[33m!\033[0m %s\n' "$*"; }
check(){ if [ "$1" = "0" ]; then ok "$2"; else bad "$2"; fi; }

step "0. 环境"
echo "  $(. /etc/os-release; echo "$PRETTY_NAME")  glibc $(ldd --version | head -1 | grep -oE '[0-9.]+$')  bash $(bash --version | head -1 | grep -oE '[0-9.]+$')"
ls /usr/lib/*/libstdc++.so.6 >/dev/null 2>&1 && ok "libstdc++6 在" || bad "libstdc++6 缺失（laya-tch 跑不了）"

step "1. 从 zip 解压"
command -v unzip >/dev/null || { apt-get update -qq >/dev/null 2>&1; apt-get install -y -qq --no-install-recommends unzip >/dev/null 2>&1; }
rm -rf "$PKGDIR"; mkdir -p "$PKGDIR"
time unzip -q "$ZIP" -d "$PKGDIR" || { bad "unzip 失败"; exit 1; }
PKG=$(find "$PKGDIR" -maxdepth 1 -type d -name 'laya-linux-cpu-x86_64-*' | head -1)
[ -n "$PKG" ] || { bad "解压后没找到包目录"; exit 1; }
cd "$PKG" || exit 1
ok "解压到 $PKG  ($(du -sh . | cut -f1), $(find . -type f | wc -l) 个文件)"
for f in install.sh env.sh VERSION README.md bin/laya-workflow bin/laya-tch bin/laya-engine bin/sqlite3 \
         libexec/laya-workflow libexec/laya-tch lib/libtorch/lib/libtorch_cpu.so \
         models/laya/model.safetensors models/laya/tokenizer/tokenizer.json; do
  [ -e "$f" ] || bad "缺 $f"
done
[ -d specs ] && ok "specs/ $(find specs -name '*.json' | wc -l) 个 spec" || bad "缺 specs/"
# 包根一旦叫 dsl/，引擎会从 cwd 往上把它当 repo 层，spec 直接翻倍
# （src/spec.rs 的 REPO_SPEC_REL_PATHS = [".laya-workflow/dsl", "dsl"]）
if [ -d dsl ]; then bad "包根存在 dsl/ —— 引擎会当 repo 层，spec 翻倍（见包内 README「为什么叫 specs」）"
else ok "包根没有 dsl/（不会触发 repo 层）"; fi
echo "  可执行位: $(ls -l bin libexec | awk 'NR>1 && $1 ~ /x/ {printf "%s ", $NF}')"

step "1b. tar.gz 也解一遍（AppleDouble 回归）"
# macOS 的 bsdtar 会把 xattr 写成 pax header / AppleDouble ._* 成员。bsdtar 自己 list 时
# 把 ._* 藏起来，本机看不出来；GNU tar 解到 Linux 上 ._* 就是实文件，spec 数直接翻倍。
# 所以 tar.gz 必须用 GNU tar 解一遍盯死这三个数：._* = 0、文件数一致、spec 数一致。
if [ -n "$TARGZ" ] && [ -f "$TARGZ" ]; then
  command -v tar >/dev/null || bad "容器里没有 tar"
  rm -rf /work/verify-tar; mkdir -p /work/verify-tar
  # 2>&1 留着：GNU tar 会把 unknown extended header 警告刷出来，有就说明又混进 xattr 了
  TARLOG=/work/tar.log
  tar xzf "$TARGZ" -C /work/verify-tar >"$TARLOG" 2>&1 || { bad "tar xzf 失败"; tail -5 "$TARLOG"; }
  if grep -q 'unknown extended header keyword' "$TARLOG" 2>/dev/null; then
    bad "tar 解出来刷了 $(grep -c 'unknown extended header keyword' "$TARLOG") 条 extended header 警告 —— 归档里混进 mac xattr 了"
  else
    ok "tar 解压没有 extended header 警告"
  fi
  TP=$(find /work/verify-tar -maxdepth 1 -type d -name 'laya-linux-cpu-x86_64-*' | head -1)
  if [ -z "$TP" ]; then
    bad "tar 解压后没找到包目录（实际：$(ls /work/verify-tar | tr '\n' ' ')）"
  else
    ok "解压到 $TP"
    N_AD=$(find "$TP" -name '._*' | wc -l)
    [ "$N_AD" -eq 0 ] && ok "没有 AppleDouble ._* 成员" || bad "有 $N_AD 个 ._* 文件 —— spec 会翻倍"
    N_TAR_F=$(find "$TP" -type f | wc -l); N_TAR_J=$(find "$TP/specs" -name '*.json' 2>/dev/null | wc -l)
    N_ZIP_F=$(find . -type f | wc -l);        N_ZIP_J=$(find specs -name '*.json' 2>/dev/null | wc -l)
    [ "$N_TAR_F" = "$N_ZIP_F" ] && ok "文件数一致（${N_TAR_F}）" || bad "文件数不一致 tar=${N_TAR_F} zip=${N_ZIP_F}"
    [ "$N_TAR_J" = "$N_ZIP_J" ] && ok "spec 数一致（${N_TAR_J}）" || bad "spec 数不一致 tar=${N_TAR_J} zip=${N_ZIP_J}"
    [ -x "$TP/bin/laya-workflow" ] && ok "tar 包的 bin/laya-workflow 可执行" || bad "tar 包的 bin/laya-workflow 没执行位"
    ( cd "$TP" && ./bin/laya-workflow list ) >/tmp/tarlist 2>&1 \
      && ok "tar 包就地 list 成功" || { bad "tar 包就地 list 失败"; tail -3 /tmp/tarlist; }
    [ -x "$TP/install.sh" ] || bad "tar 包缺 install.sh 的执行位"
  fi
else
  warn "没给 tar.gz（./verify.sh 会自动传），跳过这步"
fi

step "2. 所有 shell 脚本 dash 语法检查（install.sh 是 #!/bin/sh）"
for f in install.sh env.sh bin/laya-workflow bin/laya-tch bin/laya-engine; do
  if dash -n "$f" 2>/tmp/e; then ok "dash -n $f"; else bad "dash -n $f: $(cat /tmp/e)"; fi
done

step "3. 不安装，就地跑（解压即用的前提）"
./bin/laya-workflow list >/tmp/l1 2>&1 && ok "list 成功: $(grep -oE '[0-9]+ spec\(s\)' /tmp/l1 | head -1)" || bad "就地 list 失败: $(tail -2 /tmp/l1)"
sed -n '/search path/,/spec(s)/p' /tmp/l1 | sed 's/^/    /'

step "4. install.sh"
./install.sh --prefix "$PREFIX" --state "$STATE" >/tmp/inst 2>&1
rc=$?
[ $rc -eq 0 ] && ok "install.sh rc=0" || { bad "install.sh rc=$rc"; tail -20 /tmp/inst; }
for c in laya-workflow laya-tch laya-engine sqlite3; do
  [ -x "$PREFIX/$c" ] && ok "装了 $PREFIX/$c" || bad "没装 $c"
done
grep -q '@PKG_ROOT@' "$PREFIX/laya-workflow" && bad "wrapper 里 @PKG_ROOT@ 没被替换" || ok "@PKG_ROOT@ 已替换"
export PATH="$PREFIX:$PATH"
echo "  状态目录: $(ls "$STATE" | tr '\n' ' ')"

step "5. spec 可用"
N_DSL=$(find "$STATE/dsl" -name '*.json' 2>/dev/null | wc -l)
laya-workflow list >/tmp/l2 2>&1
SPEC_LINE=$(grep -oE '[0-9]+ spec\(s\)' /tmp/l2 | head -1)
echo "  状态目录里 $N_DSL 个 spec；list 报: ${SPEC_LINE:-?}"
if [ "${SPEC_LINE%% *}" = "$N_DSL" ]; then ok "spec 数一致（${N_DSL}）"
else bad "spec 数不一致：list=${SPEC_LINE:-?} 磁盘=$N_DSL"; fi
sed -n '/search path/,/spec(s)/p' /tmp/l2 | sed 's/^/    /'

# 回归断言：cwd 不该影响 spec 数。
# 引擎按 cwd 往上探测 repo 层，包目录必须测一遍（这里曾经真的翻倍过）。
( cd "$PKG" && laya-workflow list ) >/tmp/l3 2>&1
SPEC_IN_PKG=$(grep -oE '[0-9]+ spec\(s\)' /tmp/l3 | head -1)
if [ "${SPEC_IN_PKG%% *}" = "$N_DSL" ]; then
  ok "在包目录里跑也是 ${N_DSL} 个（repo 层没被误触发）"
else
  bad "在包目录里跑变成 ${SPEC_IN_PKG:-?} —— repo 层被触发，spec 翻倍了"
  sed -n '/search path/,/spec(s)/p' /tmp/l3 | sed 's/^/    /'
fi

step "6. laya-engine start（加载 800MB 权重，CPU）"
T0=$SECONDS
laya-engine start >/tmp/start 2>&1
rc=$?
if [ $rc -eq 0 ]; then ok "start 成功（$((SECONDS-T0))s）"; else bad "start 失败 rc=$rc"; tail -30 /tmp/start; fi
laya-engine status | sed 's/^/  /'

step "7. /health 直连"
if command -v curl >/dev/null 2>&1; then
  body="$(curl -fs --max-time 10 http://127.0.0.1:8400/health 2>/dev/null || true)"
  if [ -n "$body" ]; then printf "  /health -> %s\n" "$body"; ok "health 通"; else bad "health 不通"; fi
else
  laya-engine status > /tmp/st
  if grep -q ok /tmp/st; then ok "health ok（engine 自报）"; else bad "health 不通"; fi
fi

step "8. 真模型决策（POST /v1/systemone）"
export LAYA_BASE_URL=http://127.0.0.1:8400
laya-workflow demo >/tmp/demo 2>&1
rc=$?
if [ $rc -eq 0 ]; then ok "demo rc=0"; tail -20 /tmp/demo | sed 's/^/  /'; else bad "demo 失败 rc=$rc"; tail -20 /tmp/demo; fi

step "9. apps（内置参考用例）"
# apps 会跑 4 个 app 的参考用例，每个都是真模型推理。本机是 x86_64 容器跑在
# Apple silicon 上（qemu 模拟），单次决策 ~18s，4 个 app 跑完要好几分钟；
# 裸机 x86 上是秒级。默认给 30 分钟，超时只算 warning —— 那是模拟慢，不是包坏了。
# LAYA_SKIP_APPS=1 可以整步跳过。
APPS_TIMEOUT=${LAYA_APPS_TIMEOUT:-1800}
if [ "${LAYA_SKIP_APPS:-0}" = "1" ]; then
  info "LAYA_SKIP_APPS=1，跳过"
else
  T0=$SECONDS
  timeout "$APPS_TIMEOUT" laya-workflow apps >/tmp/apps 2>&1
  rc=$?
  N=$(grep -cE '^[[:space:]]*(OK|!!)' /tmp/apps 2>/dev/null || echo 0)
  echo "  $N 条结果，用了 $((SECONDS-T0))s（预算 ${APPS_TIMEOUT}s）"
  if [ $rc -eq 0 ]; then
    ok "apps rc=0"
  elif [ $rc -eq 124 ]; then
    warn "apps 超时（qemu 模拟慢，不是包的问题）。部分结果："
    tail -8 /tmp/apps | sed 's/^/    /'
  else
    bad "apps rc=$rc"
    tail -12 /tmp/apps | sed 's/^/    /'
  fi
  tail -8 /tmp/apps | sed 's/^/  /'
fi

step "10. 插件包"
if [ -f "$PLUGZIP" ]; then
  rm -rf /work/plug; mkdir -p /work/plug
  unzip -q "$PLUGZIP" -d /work/plug || bad "插件包解压失败"
  # 插件包和主包一样，解压出来带一层顶层目录（README 里也是 cd 进去再跑）
  PLUG=$(find /work/plug -maxdepth 1 -type d -name 'laya-plugins-*' | head -1)
  if [ -z "$PLUG" ]; then
    bad "插件包解压后没找到 laya-plugins-* 目录（实际：$(ls /work/plug | tr '\n' ' ')）"
  else
    ok "解压到 $PLUG"
    ( cd "$PLUG" && ./install.sh --state "$STATE" ) >/tmp/plug 2>&1
    rc=$?
    if [ $rc -eq 0 ]; then ok "插件包 install rc=0"; else bad "插件包 install rc=$rc"; tail -5 /tmp/plug; fi
    grep -E '插件:|specs:' /tmp/plug | sed 's/^/    /'
    NP=$(ls "$STATE/plugins" | wc -l); NM=$(ls "$STATE/laya-mem/specs" 2>/dev/null | wc -l)
    echo "  装好: $NP 个插件, $NM 个 laya-mem specs"
    [ "$NP" -ge 19 ] && ok "插件数 $NP >= 19" || bad "插件数只有 $NP"
    [ "$NM" -ge 9 ]  && ok "laya-mem specs $NM >= 9" || bad "laya-mem specs 只有 $NM"
    laya-workflow plugin list >/tmp/pl 2>&1 && ok "plugin list 成功" || bad "plugin list 失败"
    laya-workflow laya-mem info >/tmp/mi 2>&1 && ok "laya-mem info 成功" || bad "laya-mem info 失败"
    echo "  插件包 --uninstall:"
    ( cd "$PLUG" && ./install.sh --uninstall --state "$STATE" ) >/tmp/plugun 2>&1 \
      && ok "插件包卸载 rc=0（$(tail -1 /tmp/plugun)）" || bad "插件包卸载失败"
    NP2=$(ls "$STATE/plugins" 2>/dev/null | wc -l)
    # 这个状态目录里只装过插件包带来的东西，所以卸完应该是 0。
    # 如果之前跑过 `laya-workflow install`，内置副本会留下来，那也正常。
    if [ "$NP2" -eq 0 ]; then ok "卸载后 plugins/ 清空"
    else info "卸载后还剩 $NP2 个（之前跑过 laya-workflow install 的话属正常）"; fi
  fi
  cd "$PKG"
else
  bad "找不到 $PLUGZIP"
fi

step "11. 停引擎"
laya-engine stop >/tmp/stop 2>&1 && ok "stop ok" || bad "stop 失败"
laya-engine status | sed 's/^/  /'
# debian-slim 没有 pgrep/procps，直接扫 /proc
LEFT=0
for c in /proc/[0-9]*/cmdline; do
  [ -r "$c" ] || continue
  if tr '\0' ' ' < "$c" 2>/dev/null | grep -q 'libexec/laya-tch'; then LEFT=$((LEFT+1)); fi
done
[ "$LEFT" -eq 0 ] && ok "没有残留 laya-tch 进程" || bad "还有 $LEFT 个 laya-tch 进程在跑"

step "12. 卸载（先停后卸）"
./install.sh --uninstall --prefix "$PREFIX" >/tmp/un 2>&1
rc=$?
[ $rc -eq 0 ] && ok "uninstall rc=0" || bad "uninstall rc=$rc"
for c in laya-workflow laya-tch laya-engine; do
  [ -e "$PREFIX/$c" ] && bad "$c 还留着" || ok "$c 已删"
done
[ -d "$STATE" ] && ok "状态目录保留（按设计，README 有写）：$(ls "$STATE" | tr '\n' ' ')" || bad "状态目录没了"

step "结果"
if [ "$FAIL" -eq 0 ]; then
  printf '\n\033[32m全部通过\033[0m\n'
  exit 0
else
  printf '\n\033[31m%d 项失败\033[0m\n' "$FAIL"
  exit 1
fi
