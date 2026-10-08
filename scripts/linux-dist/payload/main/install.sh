#!/bin/sh
# laya 离线包安装脚本（Linux x86_64 / CPU）
#
#   ./install.sh                    装到 ~/.local/bin（不需要 root）
#   ./install.sh --prefix /usr/local  装到 /usr/local/bin（需要 root）
#   ./install.sh --prefix /usr/local --with-plugins   顺便装内置插件
#   ./install.sh --force            覆盖已安装的 dsl / 插件
#   ./install.sh --uninstall        卸载（只删本包放进去的东西）
set -e

PKG_ROOT=$(cd "$(dirname "$0")" && pwd)
PREFIX="$HOME/.local/bin"
STATE="${LAYA_HOME:-$HOME/.laya-workflow}"
WITH_PLUGINS=0
FORCE=0
UNINSTALL=0

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    --prefix=*) PREFIX="${1#*=}"; shift ;;
    --state) STATE="$2"; shift 2 ;;
    --state=*) STATE="${1#*=}"; shift ;;
    --with-plugins) WITH_PLUGINS=1; shift ;;
    --force) FORCE=1; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "未知参数: $1" >&2; exit 2 ;;
  esac
done

say() { printf '%s\n' "$*"; }

# ---------- 卸载 ----------
if [ "$UNINSTALL" -eq 1 ]; then
  for f in laya-workflow laya-tch laya-engine; do
    if [ -e "$PREFIX/$f" ] || [ -L "$PREFIX/$f" ]; then
      rm -f "$PREFIX/$f" 2>/dev/null || sudo rm -f "$PREFIX/$f"
      say "removed $PREFIX/$f"
    fi
  done
  if [ -f "$PREFIX/sqlite3.laya" ]; then rm -f "$PREFIX/sqlite3.laya"; say "removed $PREFIX/sqlite3.laya"; fi
  say ""
  say "保留未动：${STATE}（状态目录：dsl / plugins / laya-mem / chrome / run）"
  say "要一起删：rm -rf $STATE"
  exit 0
fi

# ---------- 前置检查 ----------
say "==> 检查运行环境"
ARCH=$(uname -m)
case "$ARCH" in
  x86_64|amd64) ;;
  *) say "!! 这个包是 x86_64 的，你这里是 ${ARCH}。aarch64 请用 apple silicon(macOS) 或自行编译。"; exit 1 ;;
esac
say "    架构      $ARCH  OK"

# glibc >= 2.28（libtorch 的要求）
GLIBC_OK=$(ldd --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+$')
if [ -n "$GLIBC_OK" ]; then
  MAJ=${GLIBC_OK%.*}; MIN=${GLIBC_OK#*.}
  if [ "$MAJ" -lt 2 ] || { [ "$MAJ" -eq 2 ] && [ "$MIN" -lt 28 ]; }; then
    say "!! glibc $GLIBC_OK 太老（需要 >= 2.28，即 Ubuntu 20.04 / Debian 10 / RHEL 8 及以上）"; exit 1
  fi
  say "    glibc     $GLIBC_OK  OK"
else
  say "    glibc     ?（检测不到，继续）"
fi

# libstdc++6（libtorch_cpu.so 依赖它）
NEED_LIBSQL=0
if [ -e /lib64/libstdc++.so.6 ] || [ -e /usr/lib64/libstdc++.so.6 ] \
   || ls /usr/lib/*/libstdc++.so.6 >/dev/null 2>&1 || ls /lib/*/libstdc++.so.6 >/dev/null 2>&1; then
  say "    libstdc++ OK"
else
  NEED_LIBSQL=1
  say "    libstdc++ 缺失 —— laya-tch 需要 libstdc++6"
fi
if [ "$NEED_LIBSQL" -eq 1 ]; then
  say ""
  say "    安装它（挑一个）："
  say "      apt-get install -y libstdc++6            # Debian / Ubuntu"
  say "      yum install -y libstdc++                 # RHEL / CentOS / Rocky"
  say "      apk add libstdc++                        # Alpine"
  say ""
  printf "    现在装不了就回车继续（只影响 laya-tch，laya-workflow 不依赖）: "
  read -r _ || true
fi

# ---------- 状态目录 ----------
say ""
say "==> 准备状态目录 $STATE"
for sub in dsl plugins websites laya-mem chrome run; do
  mkdir -p "$STATE/$sub"
done

# ---------- spec（工作流 spec）----------
# 编译进二进制的 builtin 路径是打包机上的绝对路径，在目标机上不存在，
# 所以 spec 必须落到状态目录里，layered lookup 才认。
#
# 源在包内 $PKG_ROOT/specs（不是 specs/ 以外的名字），装到状态目录的 dsl/ 下。
# 状态目录这一层固定叫 dsl —— 那是引擎认的 user 层位置，别改。
if [ "$FORCE" -eq 1 ] || [ ! -d "$STATE/dsl" ] || [ -z "$(ls -A "$STATE/dsl" 2>/dev/null)" ]; then
  cp -R "$PKG_ROOT/specs/." "$STATE/dsl/"
  say "    dsl       $(find "$STATE/dsl" -name '*.json' | wc -l | tr -d ' ') 个 spec  -> $STATE/dsl"
else
  say "    dsl       已有，保留（要覆盖请加 --force）"
fi

# ---------- 插件 ----------
if [ "$WITH_PLUGINS" -eq 1 ]; then
  say "    插件      从二进制内置副本安装 ..."
  if [ "$FORCE" -eq 1 ]; then
    "$PKG_ROOT/bin/laya-workflow" install --force >/dev/null
  else
    "$PKG_ROOT/bin/laya-workflow" install >/dev/null
  fi
  say "    插件      $(ls "$STATE/plugins" 2>/dev/null | wc -l | tr -d ' ') 个 -> $STATE/plugins"
  say "    laya-mem  specs -> $STATE/laya-mem/specs"
elif [ -d "$PKG_ROOT/plugins" ]; then
  mkdir -p "$STATE/plugins"
  cp -R "$PKG_ROOT/plugins/." "$STATE/plugins/"
  say "    插件      $(ls "$STATE/plugins" | wc -l | tr -d ' ') 个 -> $STATE/plugins（包内附带的）"
else
  say "    插件      跳过（这个包不带插件；用插件包，或重跑加 --with-plugins）"
fi

# ---------- 命令行 ----------
say ""
say "==> 安装命令到 $PREFIX"
mkdir -p "$PREFIX" 2>/dev/null || true
if [ ! -w "$PREFIX" ]; then
  if [ "$(id -u)" -eq 0 ]; then
    say "    $PREFIX 不可写，但当前是 root —— 继续"
  else
    say "    $PREFIX 不可写，改用 $HOME/.local/bin"
    PREFIX="$HOME/.local/bin"
    mkdir -p "$PREFIX"
  fi
fi

INSTALL_WRAPPER() {
  src="$PKG_ROOT/bin/$1"; dst="$PREFIX/$1"
  tmp="$dst.laya-tmp.$$"
  sed "s|@PKG_ROOT@|$PKG_ROOT|g" "$src" > "$tmp"
  chmod 755 "$tmp"
  mv -f "$tmp" "$dst"
  say "    $dst"
}

INSTALL_WRAPPER laya-workflow
INSTALL_WRAPPER laya-tch
INSTALL_WRAPPER laya-engine

# sqlite3：db 能力要 shell out 到它。系统已经有就别覆盖（避免影响用户其它脚本）
if command -v sqlite3 >/dev/null 2>&1; then
  cp -f "$PKG_ROOT/bin/sqlite3" "$PREFIX/sqlite3.laya"
  chmod 755 "$PREFIX/sqlite3.laya" || true
  say "    $PREFIX/sqlite3.laya   （系统已有 sqlite3，没有覆盖）"
else
  cp -f "$PKG_ROOT/bin/sqlite3" "$PREFIX/sqlite3" && chmod 755 "$PREFIX/sqlite3"
  say "    $PREFIX/sqlite3  （静态编译 3.53.4，系统原本没有）"
fi

# ---------- PATH ----------
case ":$PATH:" in
  *":$PREFIX:"*) say ""; say "    $PREFIX 已经在 PATH 里了" ;;
  *)
    say ""
    say "!! $PREFIX 不在 PATH 里，加一行到 ~/.bashrc（或者自己 export）："
    say "     export PATH=\"$PREFIX:\$PATH\""
    if [ -f "$HOME/.bashrc" ] && ! grep -q "$PREFIX" "$HOME/.bashrc" 2>/dev/null; then
      printf '\nexport PATH="%s:$PATH"\n' "$PREFIX" >> "$HOME/.bashrc"
      say "     已经帮你写进 ~/.bashrc（新开 shell 生效，或 source ~/.bashrc）"
    fi
    ;;
esac

# ---------- 冒烟 ----------
say ""
say "==> 自检"
if LAYA_HOME="$STATE" "$PREFIX/laya-workflow" list >/tmp/.laya-list.$$ 2>&1; then
  say "    laya-workflow list  OK（$(grep -oE '[0-9]+ spec\(s\)' /tmp/.laya-list.$$ | head -1)）"
else
  tail -n 5 /tmp/.laya-list.$$ | while IFS= read -r l; do say "    $l"; done
  say "    !! laya-workflow list 失败，试着手动跑：$PREFIX/laya-workflow list"
fi
rm -f /tmp/.laya-list.$$

cat <<'TIP'

装完了。用法：

  # 1) 起推理引擎（CPU，第一次加载 800MB 权重，约 10~40 秒）
  laya-engine start

  # 2) 跑一个真模型决策
  export LAYA_BASE_URL=http://127.0.0.1:8400
  laya-workflow demo

  # 或者一步到位（起引擎 + 跑 demo + 打印决策结果）
  laya-engine smoke

  # 3) 停引擎
  laya-engine stop

TIP
