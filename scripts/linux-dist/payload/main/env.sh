# source 这个文件，就能在当前 shell 里直接用 laya（不装进 PATH 也行）
#   . /解压路径/laya-linux-cpu-x86_64-0.9.0/env.sh
_laya_env_src="${BASH_SOURCE:-$0}"
_laya_pkg=$(cd "$(dirname "$_laya_env_src")" && pwd)

export LAYA_HOME="${LAYA_HOME:-$HOME/.laya-workflow}"
export LAYA_BASE_URL="${LAYA_BASE_URL:-http://127.0.0.1:8400}"
export LAYA_TCH_MODEL_DIR="${LAYA_TCH_MODEL_DIR:-$_laya_pkg/models/laya}"
export PATH="$_laya_pkg/bin:$PATH"

unset _laya_env_src _laya_pkg
echo "laya 环境就绪：$LAYA_BASE_URL  (engine 没起的话先跑 laya-engine start)"
