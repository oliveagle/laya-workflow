# laya 插件包 — @VERSION@

@N_PLUGINS@ 个 Rhai 插件 + @N_SPECS@ 个 laya-mem spec。给 `laya-linux-cpu-x86_64-@VERSION@` 配套用，
但其实**不装也能跑** —— 工作流引擎和真模型决策都不依赖插件。

```bash
tar xzf laya-plugins-0.8.0.tar.gz
cd laya-plugins-0.8.0
./install.sh            # -> ~/.laya-workflow/plugins + ~/.laya-workflow/laya-mem/specs
```

## 里面有什么

**插件（@N_PLUGINS@ 个，Rhai 脚本 + 可选 page/）**

| 类别 | 插件 |
|---|---|
| 搜索 / 聚合 | `bing` `hackernews` `v2ex` `goofish` `taobao` `hf-trending` `arxiv` `alphaxiv` |
| 开发文档 | `github` `crates` `docsrs` `mdn` `pypi` `wikipedia` |
| 工作流能力 | `bdd`（Given/When/Then 断言）`browser_base`（浏览器基座）`cua`（AX 帧定位点击）`jev-planner`（决策规划）`textdigest`（文本摘要） |

全部 @N_PLUGINS@ 个：

@PLUGIN_LIST@

每个插件一个目录：`plugin.json`（元数据）+ `main.rhai`（逻辑），部分带 `page/`（浏览器页面脚本）。

**laya-mem specs（@N_SPECS@ 个 JSON）**

@SPEC_LIST@
—— 记忆闸门的决策依据，laya-mem 靠它们工作。

## 装完

```bash
laya-workflow plugin list        # 应该看到 @N_PLUGINS@ 个
laya-workflow laya-mem info      # specs 目录和 sqlite 位置
```

## 另一种装法

其中一部分本来就编译进 `laya-workflow` 二进制里（内置副本），
所以不装这个包也能装上：

```bash
laya-workflow install            # 装插件 + laya-mem specs
laya-workflow install --force    # 覆盖
```

区别是：这个 zip 给你一份**可以离线审计、可以手改**的副本，
`laya-workflow install` 每次都从二进制里重新铺。

## 注意

大部分插件要浏览器/CDP 或网络才能真正跑起来（`browser`、`chrome` 能力）。
这个包只保证插件文件到位；**chrome 和 duckdb 不在 laya 离线包里**，
需要的话单独装。

## 卸载

```bash
./install.sh --uninstall
```
