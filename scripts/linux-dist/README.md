# laya Linux 离线包 —— 打包 / 验证流水线

在 macOS 上交叉编译出 **Linux x86_64 / CPU** 的 laya 离线运行包，解压即用。
产物 4 个文件：

| 文件 | 大小 | 说明 |
|---|---|---|
| `laya-linux-cpu-x86_64-<ver>.zip` | ~859 MB | 主包：二进制 + libtorch + 模型 + spec |
| `laya-linux-cpu-x86_64-<ver>.tar.gz` | ~860 MB | 同上，tar 格式 |
| `laya-plugins-<ver>.zip` | ~239 KB | 插件包（可审计副本 + laya-mem specs） |
| `laya-plugins-<ver>.tar.gz` | ~192 KB | 同上，tar 格式 |

解压后 **1.2 GB**，其中模型 807 MB + libtorch 417 MB 占 99%。
`duckdb` 和 `chrome` 按需求**没有**打进去。

## 快速开始

```bash
./build.sh          # 全流程：交叉编译 → 下载 → 编译 sqlite3 → 收插件 → 组装 → 打包
./verify.sh         # 起干净容器，从实际 zip 解压跑端到端验证
```

产物落在 `$LAYA_DIST_OUT`（默认 `~/ole/dist/laya`），同时生成
`SHA256SUMS` 和 `MANIFEST.md`（体积构成 + 校验和 + 各组件来源）。

## 步骤

`build.sh` 按依赖顺序跑这些步骤，也可以单独跑：

| 步骤 | 做什么 | 缓存 |
|---|---|---|
| `10-toolchain` | 装/校验交叉 gcc、补 rust-std for linux target、写 C++ wrapper | 幂等 |
| `30-libtorch` | 下载 PyTorch CPU 运行库，只留 `lib/` | 缓存在 `$WORK/libtorch` |
| `20-binaries` | 取官方 release 的 `laya-workflow` + 交叉编译 `laya-tch` | cargo 自己管 |
| `40-model` | 下载 HF 权重 + tokenizer（807 MB） | 缓存在 `$WORK/model` |
| `50-sqlite3` | 容器内静态编译 sqlite3 CLI | 缓存在 `$WORK/sqlite3` |
| `60-plugins` | 收插件（容器里跑 `laya-workflow install` + 仓库 `plugins/`） | 每次重收 |
| `70-stage` | 组装 staging 目录，生成 VERSION / README（体积按实测填） | 重建 |
| `80-archive` | zip + tar.gz + SHA256SUMS + MANIFEST.md | 重建 |

```bash
./build.sh --list              # 看步骤
./build.sh libtorch model      # 只跑这两步
./build.sh --from stage        # 从组装开始
./build.sh --only archive      # 只重新打包（改了 README 模板时用）
./build.sh clean               # 清 staging + 产物，保留下载缓存
./build.sh clean --all         # 连 ~1 GB 下载缓存一起清
```

所有步骤幂等：文件在且大小对就跳过。改了代码重新 `./build.sh` 即可，
只有 20/60/70/80 会真的重跑。

## 环境要求

| 东西 | 用来做什么 | 怎么装 |
|---|---|---|
| macOS | 交叉编译 | —— |
| `messense/macos-cross-toolchains/x86_64-unknown-linux-gnu-gcc` | C/C++ 交叉编译器（sysroot glibc 2.17） | `brew install messense/macos-cross-toolchains/x86_64-unknown-linux-gnu-gcc` |
| rustup stable toolchain | rustc / cargo | `rustup toolchain install stable` |
| docker | 编 sqlite3、收插件、跑验证 | Docker Desktop |

**必须用 rustup 的 rustc，不能用 brew 的** —— brew 的 rustc 编译产物和
rustup 的 std 不兼容，会报 `E0514 found crate compiled by an incompatible version`。

`x86_64-unknown-linux-gnu` 的 rust-std 官方镜像基本没有，脚本会从
`static.rust-lang.org` 手动补进 toolchain（幂等，只做一次）。

## 为什么还得 build —— GitHub release 到底给了什么

有官方 release，优先用。`v0.9.0` 的 release 里有：

```
laya-workflow-x86_64-unknown-linux-gnu.tar.gz   4.6 MB   ← 直接可用
laya-workflow-aarch64-apple-darwin.tar.gz                 ← macOS 用
laya-workflow-dev-*.tar.gz                                ← 测试 harness，非运行时
```

`20-binaries.sh` 现在**优先下载官方的 `laya-workflow`**（真 Ubuntu 上 `cargo build --locked`
编的，比 macOS 交叉编译可信，且大小写进 `common.sh` 校验），拿不到才退回自己交叉编译。
`LAYA_USE_RELEASE_BIN=0` 可强制退回。

但**离线包真正需要的东西 release 里一半都没有**：

| 需要 | release 有吗 | 为什么 |
|---|---|---|
| `laya-workflow` | ✅ | — |
| `laya-tch`（模型推理 engine） | ❌ | `release.yml:36` 写死 `-p laya-workflow`；`laya-tch` 是独立 crate，那个工作流根本没编它 |
| libtorch 417 MB | ❌ | PyTorch 的运行时，不是本仓库产物 |
| 模型权重 807 MB | ❌ | 在 HF 上 |
| sqlite3 CLI | ❌ | 脚本现编 |
| 98 个 `specs/*.json` | ❌（且**必须自带**） | 见下 |

最后一条是要点：`src/spec.rs:191` 的 `builtin_spec_dir()` 是
`concat!(env!("CARGO_MANIFEST_DIR"), "/dsl")` —— **编译期写死的构建机绝对路径**。
用官方二进制时 `list` 打出来是：

```
builtin  /home/runner/work/laya-workflow/laya-workflow/dsl  (missing)
user     /root/.laya-workflow/dsl
98 spec(s)
```

`builtin` 那层在目标机上根本不存在，98 个 spec 全靠包内自带的 `specs/` 顶上来。
所以这个包不是「下载即用」，libtorch / 权重 / specs 三样都得自己备齐。

顺带一提：v0.10.0 加了 `laya-workflow update` 子命令（`src/update.rs`），
装好的 laya-workflow 能自己从 GitHub Releases 自我更新。它替代的是「下载二进制」这一步，
**不替代**这个离线包 —— 它不带 libtorch、权重和 specs，所以 `20-binaries` 依然要编
`laya-tch`。

**根治办法**：把 `laya-tch` 也加进 `release.yml`（去掉 `-p laya-workflow` 的限制，
或者加一条 `-p laya-tch` 的构建）。那样 `20-binaries` 整步都能省掉，
连 brew 交叉工具链都不再是硬依赖。截至 v0.10.1 仍未做 —— `release.yml` 从 v0.7.0 起没改过，
release 里还是那 4 个 tarball。

## 交叉编译踩过的坑（脚本里都处理了，别删）

1. **rust-std 缺失** —— 镜像源没有 `x86_64-unknown-linux-gnu` 的 std，
   `10-toolchain.sh` 从 `static.rust-lang.org` 补。
2. **不能用 brew 的 rustc** —— 见上。
3. **`-stdlib=libc++`** —— `esaxx-rs` 的 build.rs 用 `target_os == "macos"` 判断，
   而 build.rs 是给 host 编译的，于是在 macOS 上交叉编 Linux 时它会给 Linux 的 g++
   塞一个 macOS 专用的 `-stdlib=libc++`，直接编译失败。
   `10-toolchain.sh` 生成一个 wrapper 把这个参数剔掉。
4. **`GLIBC_2.28` 未定义引用** —— brew 的 sysroot 是 glibc 2.17，链接期解析不了
   libtorch 需要的 `fcntl64@GLIBC_2.28`。这些符号在运行机（glibc≥2.28）上都有，
   是 shared lib 的未定义引用，链接期加 `-Wl,--allow-shlib-undefined` 放宽即可。
5. **libtorch 的 ABI** —— 下载 URL 文件名里没有 `cxx11-abi`，但 linux 轮子实际就是
   cxx11 ABI。`20-binaries.sh` 显式设 `LIBTORCH_CXX11_ABI=1`，
   `30-libtorch.sh` 会用 `__cxx11` 符号自检。

## 打包踩过的坑（`80-archive.sh`，别删那些断言）

1. **bsdtar 会往 tar 里塞 AppleDouble `._*` 成员** —— macOS 的 bsdtar 读文件上的
   xattr 时会**悄悄**加一个 `._xxx` 成员来携带它，而且 **bsdtar 自己 list 时会把这些成员
   藏起来**，所以本机 `tar tzf` 看一切正常。换到 Linux 用 GNU tar 解出来，`._*` 就是
   实打实的文件 —— `specs/` 凭空多一倍（98 → 196），`laya-workflow` 会把它们当 spec 读，
   而且**版本号会对不上**。zip 格式不受影响（`zip -X` 天然不带）。
2. **`xattr -cr` 清不掉 `com.apple.provenance`** —— 新版 macOS 不让普通用户删这个
   xattr（命令返回 0 但什么都没删）。所以真正的开关是打包时的 flag：
   `COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs`。少了 `--no-xattrs` 的话
   GNU tar 解包时还会刷一百多条 `Ignoring unknown extended header keyword`。
3. **别用 `gzip -dc | grep -c` 数 tar 成员** —— 那是对二进制流数行，会得到几百万这种
   毫无意义的数。`80-archive.sh` 的 `tar_list()` 改用 `tar -tzf`——`tar` 直接列全部成员
   （含 `._*` AppleDouble），既不会像 bsdtar 那样藏 `._*`，数得也准，无须任何解释器。

`80-archive.sh` 有三条**构建期**断言，就是为了不让上面这些坑漏到 Linux 用户手上：

- zip / tar 的成员数必须 == staging 里「文件 + 目录」的总数（多一个都不行）
- tar 里出现任何 basename 以 `._` 开头的成员 → 直接 `die`
- 每次都报一次「没有 AppleDouble 成员」，让人知道这层保险还在

## 脚本里踩过的坑（locale）

`build.sh` 每次跑之前都会 `lint_locale_vars` 扫一遍所有脚本，找这种写法：

```sh
info "容器保留: $NAME（docker rm -f $NAME 清掉）"   # ← 全角括号紧跟 $NAME
```

在某些 locale 下 bash 会把高位字节吃进变量名，`$NAME（` 被当成一个叫 `NAME（` 的变量，
**静默展开成空串** —— 脚本不报错、退出码 0，但输出里那个名字就没了。所以约定：

- 变量引用一律写 `${NAME}`，后面用空格或标点隔开
- 守卫的匹配规则是 `LC_ALL=C grep -aE '\$[A-Za-z_][A-Za-z0-9_]*[^ -~]'`
  （按字节匹配，能抓到 UTF-8 的高位字节）

踩过 6 处，都修了。改脚本时新增带中文的输出，守卫会替你抓住。

## 版本号不要手工抄

`PKG_VERSION` 直接从 `Cargo.toml` 的 `[package] version` 读：

```bash
PKG_VERSION=${LAYA_PKG_VERSION:-$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO_ROOT/Cargo.toml" | head -1)}
```

以前这里是手抄的常量，后果是升级时漏改一处就打出一个「包名写着 0.10.1、里面装着 0.9.0
编出来的二进制」的包 —— 而版本号还被顺手写进包里的 `VERSION` 文件，用户根本看不出来。
`cargo release` 之类改了 Cargo.toml 但没改脚本的情况同理。需要临时覆盖用
`LAYA_PKG_VERSION`。

## 模型必须 pin 到 commit（`MODEL_REV`）

不能写 `main`。上游 2026-10-03 动过一次 `main`，`tokenizer.json` 从 3582228 变成 3583228
字节 —— `40-model.sh` 的大小校验直接把它拦下来了：

```
✗ tokenizer.json 大小不对：期望 3582228，实际 3583228
```

这正是那个校验存在的意义：静默混入一份不同的权重，比构建失败糟糕得多。
所以 `MODEL_REV` 写死成 `7b928d828b7b0e022f929d9bd2e44165aa270148`（当时的 `main`），
URL 用 `/resolve/$MODEL_REV`。要升级模型：

```bash
curl -s https://huggingface.co/api/models/convaiinnovations/laya | grep -m1 '"sha"'
# 把 sha 填进 common.sh 的 MODEL_REV，并把 MODEL_SAFETENSORS_BYTES /
# MODEL_TOKENIZER_BYTES 更新为新的大小
```

## 插件从哪来（重要）

插件有三个来源，`60-plugins.sh` 按顺序叠加，内置的优先：

1. **二进制内置** 17 个 —— 在容器里跑 linux 版 `laya-workflow install` 收出来
2. **仓库 `plugins/`** 5 个 —— 其中 4 个和内置重复，`jev-planner` 是仓库独有的
3. **`$LAYA_EXTRA_PLUGIN_DIR`**（默认 `~/.laya-workflow/plugins`）—— 本机 LAYA_HOME 里的额外插件

第 3 项**不是仓库内容**。本机上它贡献了 `taobao`（仓库里根本没有这个插件），
所以当前产物是 19 个；换台机器重建，插件集合可能不同。
只要仓库可复现的那份：

```bash
LAYA_EXTRA_PLUGIN_DIR=/nonexistent ./build.sh --only plugins stage archive
```

脚本会把每个来源新增了哪些插件打印出来。

## 验证

`verify.sh` 起一个干净的 `debian:bookworm-slim`（`--platform linux/amd64`），
从**实际产出的 zip** 走完整链路，13 步（tar.gz 单独再验一遍）：

```
 0 环境（glibc / libstdc++）
 1 从 zip 解压 + 关键文件齐全性
 1b tar.gz 用 GNU tar 再解一遍：._* 必须为 0、文件数/spec 数必须和 zip 一致
 2 所有 sh 脚本过 dash -n（install.sh 是 #!/bin/sh，dash 没有 bash 的语法）
 3 不安装就地跑（"解压即用"的前提）
 4 install.sh + @PKG_ROOT@ 替换检查
 5 spec 数一致（磁盘上的 json 数 == list 报的数）
 6 laya-engine start（加载 800MB 权重）
 7 /health 直连
 8 真模型决策 demo（POST /v1/systemone）
 9 apps 内置参考用例
10 插件包装 + plugin list + laya-mem info
11 停引擎 + 扫 /proc 确认没残留进程
12 卸载（先 stop 再 uninstall，顺序反了会误报）
```

失败会保留容器：

```bash
./verify.sh --keep        # 验证完不销毁
./verify.sh --shell       # 验证完直接进容器
docker exec -it laya-verify-<pid> bash
```

## 改包内容

| 想改 | 改哪 |
|---|---|
| 安装逻辑、wrapper、engine 管理 | `payload/main/{install.sh,env.sh,bin/*}` |
| 主包说明书 | `payload/main/README.md.in`（`@N_XXX@` 占位符在打包时按实测填） |
| 插件包安装逻辑 / 说明书 | `payload/plugins/{install.sh,README.md}` |
| 版本号、URL、校验和、路径 | `common.sh` |
| 产物目录 / 工作目录 | `LAYA_DIST_OUT` / `LAYA_WORK` 环境变量 |

改完只要 `./build.sh --only stage archive && ./verify.sh`。
`payload/` 下的文件是**逐字**进包的（`install.sh` 装的时候会把 wrapper 里的
`@PKG_ROOT@` 替换成安装后的绝对路径），所以在 macOS 上可以直接 review。

## 已知限制

- **只有 x86_64**。aarch64（Apple silicon Linux、ARM 云主机）没有包。
- **libtorch 决定 glibc 下限 2.28**（Ubuntu 20.04 / Debian 10 / RHEL 8+）。
  laya 自己的 rust 代码只要 glibc 2.17，是 libtorch 拉高的。
- **不含 chrome / duckdb**。插件里 `browser`、`chrome` 能力要浏览器才能真跑起来。
- **rust 二进制不保证 bit-reproducible**，同样代码重编 sha256 可能变。
  上游依赖（libtorch `.so`）用 sha256 强校验，见 `common.sh`。
