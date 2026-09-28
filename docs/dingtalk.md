# DingTalk (`钉钉`) — 能读消息吗？调研结论

**结论：公开 API 基本读不了会话历史。** 与飞书（`lark-cli` 可以个人身份读自己的会话历史 + 全局搜索）相比，钉钉没有对等能力。下面把"能拿到什么"和"做不到什么"分开写清楚，并附官方文档出处。

## 一、能拿到消息内容的三条正规途径

| 途径 | 拿到什么 | 限制 |
|------|----------|------|
| **机器人接收消息**（Stream / HTTP 回调，topic 固定 `/v1.0/im/bot/messages/get`） | 用户 **@机器人** 时那条消息（实时推送：`conversationId`/`senderNick`/`text`/`msgtype`…） | 只限 **@机器人** 的消息；**没有历史** |
| **消息菜单 JSAPI「获取消息内容」**（`dingtalk-jsapi`） | 用户在**某条消息**上点你的消息菜单 → 回调给你**那一条**消息（`conversation.openConversationId`、`msgList[].senderName/createAt/msgtype/text`，图片/文件可再走钉盘下载） | **单条**、**用户触发**、依赖客户端版本 |
| **机器人会话内文件/图片** | @机器人 时**附带**发送的文件 | 仅当次，不覆盖群里其他人的历史文件 |

官方文档：
- 机器人接收消息 <https://open.dingtalk.com/document/orgapp/robot-receive-message>
- 获取消息内容（消息菜单）<https://open.dingtalk.com/document/development/message-menu-api>

## 二、明确做不到的

* **拉取群 / 单聊的历史消息列表**：开放平台**没有**这个 API（只能实时收 @ 机器人的消息，或按需取单条）。
* **会话存档类 API**：钉钉公开文档里**没有**（企业微信有「会话内容存档」，钉钉没有等价的公开接口）。钉钉开发者社区问答也明确："目前没有获取聊天记录的接口，也没有聊天记录存档的接口。"
* **数据类 API**（`获取企业聊天数据` / `获取企业各部门聊天数据` / `获取企业群聊统计数据`）只返回**汇总计数**（消息数、人数、群数、文件数），**不含消息内容**；且属于数据资产类，**2023-09-01 起关闭了开发者后台的新申请入口**，统一迁移到「钉钉数据资产平台」。<https://open.dingtalk.com/document/orgapp/dingtalk-chat-information-in-key-accounts>
* **官方 MCP**：钉钉 OpenAPI MCP（`open-dingtalk/dingtalk-mcp`，<https://github.com/open-dingtalk/dingtalk-mcp>）覆盖通讯录/部门、AI 表格、日程、待办、**机器人发消息/DING/工作通知**、TB 项目管理、日志、企业荣誉、签到等——**不含"读消息"**。<https://open.dingtalk.com/document/ai-dev/dingtalk-server-api-mcp-overview>

## 三、本地客户端（本机实测）

* 本机已装 `DingTalk.app`；账号数据在
  `~/Library/Containers/5ZSL2CJU2T.com.dingtalk.mac/Data/Library/Application Support/DingTalkMac/<hash>_v3/`
* 消息库是 `DBFiles/dingtalk.db`（~9 MB，配套 `dingtalk.db_fts` 全文索引），还有 `members.db`、`calendar_v2.db` 等。
* **但这些库是加密的**：文件头不是 `SQLite format 3`，`sqlite3` 直接报 `file is not a database`（SQLCipher）。要读需要一个非公开的密钥——属于非官方途径，脆弱、且涉及合规风险，**不建议**。

## 四、手动导出（官方唯一的"导出"）

手机端钉钉：**长按消息 → 多选 → 导出**（仅限自己可见的会话；安卓一次 ≤50 条、iOS ≤100 条；导出为 PDF / 图片）。桌面端无批量导出。

## 五、结论 / 建议

* 只要**实时**接住"发给机器人"的消息 → **可以**：企业内部应用 + 群机器人 + Stream/HTTP 回调。
* 只要**某一条**消息内容（用户点菜单触发）→ **可以**：消息菜单 JSAPI。
* 要**批量 / 历史**读消息 → **公开 API 做不到**。只能走企业级合规 / 专属钉钉（专属化、私有化部署，线下商务对接，无公开 API），或客户端本地解密（不建议）。

> 对比：飞书侧已有可运行方案（`docs/feishu.md` + `dsl/capabilities/feishu_chat_history.json`，用 `lark-cli` 以用户身份读会话历史）。钉钉没有对等的公开能力。
