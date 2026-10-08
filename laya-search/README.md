# laya-search

用**本地 laya-mlx** 复现 [superagents-lab/jev-search](https://github.com/superagents-lab/jev-search) 的 API：自然语言搜索，决策模型负责选源、理解查询和相关性排序，搜索经 Search1API 执行，结果带可见的相关性分数。不生成答案，只给链接和摘要。

```
"Rust async runtimes on Hacker News this month"
→ intent: sources=hackernews(96%),reddit(90%),github(65%)  window=any
→ lane: 每个引擎的排序结果流式返回
```

## 与 jev-search 的对应关系

| jev-search | laya-search |
| --- | --- |
| TypeSafe Jev / Clef / GPT-6 Luna（云端决策） | **本地 `laya-mlx --serve`**（`/v1/systemone`，同一 decision 方言） |
| Cloudflare Workers + TanStack Start + React | 纯 Node 22 ESM，`node src/server.js`，无构建步骤 |
| Search1API | Search1API（`SEARCH1API_API_KEY`） |
| KV 缓存 | 进程内 TTL 缓存（同 10min–6h 分档） |
| 速率限制 / Origin 强校验 | 本地部署省略限流；Origin 存在时强校验，curl 等无 Origin 客户端放行 |

### 面向本地模型的适配（实测）

源判断的 criteria 用短句（"Yes, this source fits…"）而非 Jev 版长文案：长文本会把请求挤出模型 512-token 窗口，概率被压平。实测 "…on Hacker News this month"：hackernews 53%→**96%**、reddit→90%、github→65%。时间窗判断是该 checkpoint 的能力边界（"this month" 识别不出，默认 any；可用请求参数 `w` 显式指定）。

## API

### `GET /api/models`

```json
{"models":["laya-mlx"]}
```

laya-mlx 不可达时返回 503 `{"error":"Models are unavailable"}`。

### `POST /api/ask`

请求体（与 jev-search 相同）：

```json
{ "q": "Rust async runtimes on Hacker News this month", "w": "7d", "s": ["google","hackernews"], "m": "laya-mlx" }
```

- `q`：必填，1–300 字符；为空或超长 → 400
- `w`：可选时间窗 `any|24h|7d|30d`（缺省由模型判断）
- `s`：可选源列表（≤12，超出 → 400；去重、忽略非法项；缺省由模型判断）
- `m`：可选模型 id，必须是 `laya-mlx`，否则 400

响应为 NDJSON 流（`application/x-ndjson`），每行一个事件，按发生顺序：

```jsonl
{"type":"intent","request":"…","query":"…","entityQuery":"…","candidates":[…],"window":"7d","sources":["hackernews","google"],"inferred":{"window":{"choice":"7d","confidence":0.9},"sources":{"google":0.9,"hackernews":0.95,…},"query":{"index":1,"confidence":0.9},"entity":{"index":2,"confidence":0.8}},"intentMs":120,"judge":"laya-mlx"}
{"type":"found","source":"hackernews","engine":"google","items":[…],"searchMs":300}
{"type":"lane","source":"hackernews","engine":"google","items":[…],"stale":0,"searchMs":300,"scoreMs":150}
{"type":"done","totalMs":900,"tokens":4200}
```

失败时流内发送 `{"type":"error","message":"…"}`。单引擎 15s 超时、整请求 30s 截止；一个引擎失败不影响其它引擎（lane 带 `error` 字段）。同一 URL 跨引擎合并（engine agreement 计入排序）。

## 运行

```bash
# 1. 决策模型（本仓库）
cd ../laya-mlx && cargo build --release
./target/release/laya-mlx --serve --port 8400

# 2. 搜索服务
cd ../laya-search
SEARCH1API_API_KEY=… npm start          # http://127.0.0.1:3030
```

环境变量：`PORT`（3030）、`LAYA_JUDGE_URL`（http://127.0.0.1:8400）、`LAYA_MODEL_ID`（laya-mlx）、`SEARCH1API_API_KEY`（必需）、`SEARCH1API_BASE_URL`。

浏览器打开 `http://127.0.0.1:3030` 有一个最小演示页。

## 测试

```bash
npm test        # judge 与 Search1API 全部 mock，无需 key
```

覆盖：候选词剥离、intent→lanes→done 事件序、URL 去重合并、相关性打分、引擎失败隔离、请求校验（q/w/s/m）、NDJSON 契约、缺 key 500。

## 目录

```
src/judge.js        laya-mlx 判定客户端：inferIntent + rerank（替换 typesafe.ts）
src/pipeline.js     并发搜索 + 排序流（intent/found/lane/done）
src/sources.js      12 个源、引擎映射、时间窗
src/candidates.js   查询候选构造
src/freshness.js    日期解析与新鲜度
src/search1api.js   Search1API 客户端
src/rank.js merge.js cache.js
src/api.js server.js demo.html
test/
```

## 致谢

管线、候选、排序与缓存逻辑改编自 [superagents-lab/jev-search](https://github.com/superagents-lab/jev-search)（MIT，见 `LICENSE.jev-search`）；决策层替换为本仓库的本地 `laya-mlx`。
