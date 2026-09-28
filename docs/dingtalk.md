# DingTalk (`钉钉`) — 能读消息吗？

**结论：没有公开 API 能读会话历史，但可以用「客户端 + 辅助功能（Accessibility）」直接读——也就是你说的 computer use。本仓库已内置可运行方案并实测通过。**

* ❌ **服务端 API**：没有拉取历史消息的接口（下面第二节有出处）。
* ✅ **客户端 + Accessibility（computer use）**：DingTalk 是 macOS 原生 App，把消息渲染成了可访问性元素，所以能在不碰数据库、不碰协议的前提下，读出会话列表和打开会话的消息文本。本仓库提供 `scripts/dingtalk-ax` + 可运行 spec `dsl/capabilities/dingtalk_chat_history.json`。

---

## 一、能拿到消息内容的三条**服务端 API** 途径（都不含历史）

| 途径 | 拿到什么 | 限制 |
|------|----------|------|
| **机器人接收消息**（Stream / HTTP 回调，topic 固定 `/v1.0/im/bot/messages/get`） | 用户 **@机器人** 时那条消息（实时推送：`conversationId`/`senderNick`/`text`/`msgtype`…） | 只限 **@机器人** 的消息；**没有历史** |
| **消息菜单 JSAPI「获取消息内容」**（`dingtalk-jsapi`） | 用户在**某条消息**上点你的消息菜单 → 回调给你**那一条**（`openConversationId`、`msgList[].senderName/createAt/msgtype/text`） | **单条**、**用户触发** |
| **机器人会话内文件/图片** | @机器人 时**附带**发送的文件 | 仅当次 |

文档：机器人接收消息 <https://open.dingtalk.com/document/orgapp/robot-receive-message>；获取消息内容 <https://open.dingtalk.com/document/development/message-menu-api>。

## 二、服务端明确做不到的

* **拉取群 / 单聊历史消息列表**：开放平台**没有**这个 API。
* **会话存档类 API**：钉钉公开文档里**没有**（企业微信有「会话内容存档」，钉钉没有等价的公开接口）。钉钉开发者社区问答原话："目前没有获取聊天记录的接口，也没有聊天记录存档的接口。"
* **数据类 API**（`获取企业聊天数据` / `获取企业各部门聊天数据` / `获取企业群组统计数据`）只给**汇总计数**（消息数/人数/群数/文件数），**不含内容**；且属于数据资产类，**2023-09-01 起关闭了开发者后台的新申请入口**，迁到「钉钉数据资产平台」。<https://open.dingtalk.com/document/orgapp/dingtalk-chat-information-in-key-accounts>
* **官方 MCP**（`open-dingtalk/dingtalk-mcp`，<https://github.com/open-dingtalk/dingtalk-mcp>；钉钉 OpenAPI MCP <https://open.dingtalk.com/document/ai-dev/dingtalk-server-api-mcp-overview>）：通讯录/部门、AI 表格、日程、待办、**机器人发消息/DING/工作通知**、日志、荣誉、签到……**不含"读消息"**。

## 三、客户端 + Accessibility（computer use）—— ✅ 实测可行

这是"没有 API 就直接读客户端"的路子。DingTalk macOS 版是 **Qt 原生 App**（不是 WebView 壳），它把界面结构暴露给了 macOS 的 **Accessibility API**，包括：

* **会话列表**：每个会话 = 名称 + 最近一条消息预览 + 时间（AX 元素）。
* **打开会话的消息区**：一个 `AXTable`，每行 `AXRow` = 发送人（`AXButton` title）+ 角色标签（老师/班主任…）+ **正文（`AXTextArea` value）** + 时间；文件消息还能读到文件名与大小（`U1-U3语法复习（默写）.pdf` / `123.3 KB`）。

### 用法

需要给**运行它的进程**（终端 / 宿主 App）授予「辅助功能」权限：系统设置 → 隐私与安全性 → 辅助功能。**不需要**屏幕录制权限（走 AX，不走截图 OCR）。

```bash
cd <repo>
./scripts/dingtalk-ax sessions        # 列出会话：名称 / 最近消息 / 时间
./scripts/dingtalk-ax sessions --json # 同上，JSON
./scripts/dingtalk-ax chat            # 读当前打开会话的消息
./scripts/dingtalk-ax chat --json     # JSON
./scripts/dingtalk-ax click 3         # 打开第 3 个会话（按 AX 几何坐标点击，只切换、不发消息）
./scripts/dingtalk-ax dump 8          # 原始 AX 树（调试用）
```

`scripts/dingtalk-ax` 是个 bash 包装：首次运行用 `xcrun swiftc` 编出 `scripts/dingtalk-ax.swift`（结果缓存在 `$TMPDIR`），之后直接调用。所以机器上要有 Xcode Command Line Tools。

本机实测输出（节选）：

```
# conversations (12)
 0. 9.22 重默季节词语  [09-22]
     刘彩芬: 嗯 好的 回去我说说他
 …
11. 三年级2班  [18:32]
     金钢(老师): 数学: ①完成巩固2.2  友情提醒: ☞下面名单中，没有下划线的学号…

# messages (11)   ← 打开的是「三年级2班」
[今天 14:14] 金钢(老师): 今天值日生是39/40/41/1，请通知家长，5点来接孩子哦！
[今天 15:36] 朱佳怡(老师): 1. 今天默写得C、D的同学准备明天重默。…
[U1-U3语法复习（默写）.pdf] 朱佳怡(老师): [file] 123.3 KB
…
```

`click` 换会话再读也验证过：点第 11 个会话后，`chat` 读到的是该会话的消息正文。

### 从 laya-workflow 驱动

`dsl/capabilities/dingtalk_chat_history.json` 用 `exec` 调 `scripts/dingtalk-ax`，把会话列表/消息正文投进 state，再让模型判断：

```bash
laya-workflow run --spec dsl/capabilities/dingtalk_chat_history.json \
  --state "{\"tool\":\"$PWD/scripts/dingtalk-ax\"}"
```

### 限制与注意

* **只读到 App 已加载/已渲染的部分**：消息列表是虚拟化的，翻更早的历史需要先滚动（`click`/滚动加载后才能读到）。云端本身也只保留 2022-04-08 之后的消息。
* **依赖 Qt 的 AX 实现**：这是非官方、未文档化的接口，钉钉升级客户端可能改结构——比 API 脆弱。
* **只读**：`click` 只切换会话；不要用它去点「发送」。
* 换机器/换终端要重新授权辅助功能。

## 四、本地数据库：加密，不是那条路

消息库在
`~/Library/Containers/5ZSL2CJU2T.com.dingtalk.mac/Data/Library/Application Support/DingTalkMac/<hash>_v3/DBFiles/dingtalk.db`（~9 MB，配 `dingtalk.db_fts` 全文索引）。但它是 **SQLCipher 加密**的：文件头不是 `SQLite format 3`，`sqlite3` 直接报 `file is not a database`。要读得拿到一个非公开密钥——非官方、脆弱、有合规风险，**不建议**。走 AX（第三节）不需要解密。

## 五、手动导出（官方唯一的"导出"）

手机端钉钉：**长按消息 → 多选 → 导出**（仅限自己可见；安卓一次 ≤50 条、iOS ≤100 条；PDF / 图片）。桌面端无批量导出。

## 六、结论

| 需求 | 服务端 API | 客户端 + Accessibility |
|------|-----------|------------------------|
| 实时接住"发给机器人"的消息 | ✅（仅 @机器人） | ✅ |
| 读**某一条**消息（用户触发） | ✅（消息菜单 JSAPI） | ✅ |
| 读某会话的**当前消息** | ❌ | ✅（本仓库 `scripts/dingtalk-ax`） |
| 列出会话 + 最近消息预览 | ❌ | ✅ |
| 批量/全量拉历史 | ❌ | 部分（需滚动加载；受虚拟化/云端年限限制） |

> 对比飞书：飞书有个人身份可用的 `lark-cli`（读自己的会话历史 + 全局搜索），是"正经 API"；钉钉没有，所以这里用 **Accessibility（computer use）** 从客户端直接读，本仓库已内置并实测通过。
