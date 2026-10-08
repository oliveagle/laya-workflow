#!/bin/bash
# 组装 staging 目录（还没压缩）。两个包：
#   $STAGE_MAIN     laya-linux-cpu-x86_64-0.8.0
#   $STAGE_PLUGINS  laya-plugins-0.8.0
source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

step "70/80 组装 staging"

# ---- 前置检查 ----
for p in "$WORK/libtorch/lib" "$WORK/model/model.safetensors" "$WORK/sqlite3" \
         "$RUST_TARGET_DIR/laya-workflow" "$RUST_TARGET_DIR/laya-tch" "$REPO_ROOT/dsl"; do
  [ -e "$p" ] || die "缺 $p —— 按顺序跑：./build.sh toolchain libtorch model sqlite binaries"
done
[ -f "$WORK/.plugin-counts" ] || die "没跑过 ./build.sh plugins"

# ---- 清干净重建 ----
for d in "$STAGE_MAIN" "$STAGE_PLUGINS"; do
  [ -d "$d" ] && find "$d" -mindepth 1 -delete
  mkdir -p "$d"
done
find "$d" -name '.DS_Store' -delete 2>/dev/null || true

# ================= 主包 =================
info "主包 $PKG_NAME-$PKG_VERSION"
mkdir -p "$STAGE_MAIN"/{bin,libexec,lib/libtorch,models/laya/tokenizer,specs}

# 静态脚本（install.sh / env.sh / 3 个 wrapper）—— 原样拷，install.sh 装的时候会把
# wrapper 里的 @PKG_ROOT@ 替换成安装后的绝对路径
cp "$SCRIPT_DIR/payload/main/install.sh" "$STAGE_MAIN/install.sh"
cp "$SCRIPT_DIR/payload/main/env.sh"     "$STAGE_MAIN/env.sh"
for w in laya-workflow laya-tch laya-engine; do
  cp "$SCRIPT_DIR/payload/main/bin/$w" "$STAGE_MAIN/bin/$w"
done
chmod 755 "$STAGE_MAIN/install.sh" "$STAGE_MAIN/bin/"*

# 二进制
cp "$RUST_TARGET_DIR/laya-workflow" "$STAGE_MAIN/libexec/laya-workflow"
cp "$RUST_TARGET_DIR/laya-tch"       "$STAGE_MAIN/libexec/laya-tch"
cp "$WORK/sqlite3"                   "$STAGE_MAIN/bin/sqlite3"
chmod 755 "$STAGE_MAIN/libexec/"* "$STAGE_MAIN/bin/sqlite3"

# libtorch：只要 lib/，include/ 和 share/ 用不到
cp -R "$WORK/libtorch/lib" "$STAGE_MAIN/lib/libtorch/"

# 模型
cp "$WORK/model/model.safetensors"        "$STAGE_MAIN/models/laya/model.safetensors"
cp -R "$WORK/model/tokenizer/."            "$STAGE_MAIN/models/laya/tokenizer/"

# specs（工作流 spec）
#   注意 1：spec 没有编译进二进制 —— 二进制里的 builtin_spec_dir() 是**打包机**上的绝对路径。
#           所以目标机上必须靠这 98 个 json 才能让 `laya-workflow list` 有东西。
#   注意 2：目录名故意叫 specs 而不是 dsl。引擎会从当前目录往上找
#           `.laya-workflow/dsl` 或 `dsl`（见 src/spec.rs 的 REPO_SPEC_REL_PATHS），
#           找到就当 `repo` 层。包根如果叫 dsl，用户在包目录里跑任何命令都会
#           多加载一层，同一批 spec 变成 196 个，而且会**盖住**用户装在
#           ~/.laya-workflow/dsl 里自己改过的版本。叫 specs 就绕开了。
cp -R "$REPO_ROOT/dsl/." "$STAGE_MAIN/specs/"

find "$STAGE_MAIN" -name '.DS_Store' -delete 2>/dev/null || true

# VERSION
cat > "$STAGE_MAIN/VERSION" <<EOF
laya-workflow $PKG_VERSION
libtorch $LIBTORCH_VERSION+cpu (linux x86_64, from $(basename "$LIBTORCH_URL"))
model $MODEL_REPO@$MODEL_REV :: model.safetensors + tokenizer (root variant, typed-decisions)
sqlite3 3.53.4 (static)
EOF

# README：模板 + 实测体积
N_SPEC=$(find "$STAGE_MAIN/specs" -name '*.json' | wc -l | tr -d ' ')
N_FILES=$(find "$STAGE_MAIN" -type f | wc -l | tr -d ' ')
sed \
  -e "s|@VERSION@|$PKG_VERSION|g" \
  -e "s|@N_SPEC@|$N_SPEC|g" \
  -e "s|@N_SPEC_SUM@|$((N_SPEC * 2))|g" \
  -e "s|@N_FILES@|$N_FILES|g" \
  -e "s|@N_DSL_BYTES@|$(human "$(dirsize "$STAGE_MAIN/specs")")|g" \
  -e "s|@N_TCH_BYTES@|$(human "$(filesize "$STAGE_MAIN/libexec/laya-tch")")|g" \
  -e "s|@N_WF_BYTES@|$(human "$(filesize "$STAGE_MAIN/libexec/laya-workflow")")|g" \
  -e "s|@N_SQLITE_BYTES@|$(human "$(filesize "$STAGE_MAIN/bin/sqlite3")")|g" \
  -e "s|@N_LIBTORCH_BYTES@|$(human "$(dirsize "$STAGE_MAIN/lib/libtorch/lib")")|g" \
  -e "s|@N_MODEL_BYTES@|$(human "$(dirsize "$STAGE_MAIN/models/laya")")|g" \
  -e "s|@N_TOTAL_BYTES@|$(human "$(dirsize "$STAGE_MAIN")")|g" \
  -e "s|@TOTAL_HUMAN@|$(du -sh "$STAGE_MAIN" | cut -f1)|g" \
  "$SCRIPT_DIR/payload/main/README.md.in" > "$STAGE_MAIN/README.md"

ok "主包 $(human "$(dirsize "$STAGE_MAIN")")  $N_FILES 个文件  $N_SPEC 个 spec"

# ================= 插件包 =================
info "插件包 $PLUGIN_PKG_NAME-$PKG_VERSION"
# shellcheck disable=SC1090
. "$WORK/.plugin-counts"   # plugins=N specs=M
mkdir -p "$STAGE_PLUGINS/payload/laya-mem"
cp -R "$WORK/plugin-merged"                        "$STAGE_PLUGINS/payload/plugins"
cp -R "$WORK/plugin-harvest/home/laya-mem/specs"   "$STAGE_PLUGINS/payload/laya-mem/specs"
cp "$SCRIPT_DIR/payload/plugins/install.sh" "$STAGE_PLUGINS/install.sh"
chmod 755 "$STAGE_PLUGINS/install.sh"
# 名单也按实际内容生成，插件增减不用手改模板
PLUGIN_LIST=$(ls -1 "$WORK/plugin-merged" | sort | awk '{printf "`%s` ", $0}')
SPEC_LIST=$(ls -1 "$WORK/plugin-harvest/home/laya-mem/specs" | sed 's/\.json$//' | sort | awk '{printf "`%s` ", $0}')
sed -e "s|@VERSION@|$PKG_VERSION|g" -e "s|@N_PLUGINS@|$plugins|g" -e "s|@N_SPECS@|$specs|g" \
    -e "s|@PLUGIN_LIST@|$PLUGIN_LIST|" -e "s|@SPEC_LIST@|$SPEC_LIST|" \
  "$SCRIPT_DIR/payload/plugins/README.md" > "$STAGE_PLUGINS/README.md"
find "$STAGE_PLUGINS" -name '.DS_Store' -delete 2>/dev/null || true

ok "插件包 $(human "$(dirsize "$STAGE_PLUGINS")")  $plugins 个插件 + $specs 个 specs"
