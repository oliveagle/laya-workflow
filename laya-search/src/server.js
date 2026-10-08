// Plain-Node HTTP server exposing the jev-search API with laya-mlx as the
// decision model. No build step, no bundler: node src/server.js.
import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { askHandler, modelsHandler, sameOrigin, selectModel, validateAskRequest } from './api.js';
import { judgeConfigFromEnv } from './judge.js';
import { memoryCache } from './cache.js';

const demo = readFileSync(fileURLToPath(new URL('./demo.html', import.meta.url)));

function json(response, status, body) {
  response.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
  response.end(JSON.stringify(body));
}

export function depsFromEnv(env = process.env) {
  return {
    search1api: {
      apiKey: env.SEARCH1API_API_KEY ?? '',
      baseUrl: env.SEARCH1API_BASE_URL,
    },
    judge: judgeConfigFromEnv(env),
    cache: memoryCache(),
  };
}

export function createApp(deps) {
  return async function handle(request, response) {
    const url = new URL(request.url ?? '/', `http://${request.headers.host ?? 'localhost'}`);

    if (request.method === 'GET' && (url.pathname === '/' || url.pathname === '/index.html')) {
      response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' });
      return response.end(demo);
    }

    if (request.method === 'GET' && url.pathname === '/api/models') {
      const { status, body } = await modelsHandler(deps);
      return json(response, status, body);
    }

    if (request.method === 'POST' && url.pathname === '/api/ask') {
      if (!sameOrigin(url, request.headers.origin, request.headers['sec-fetch-site'])) {
        return json(response, 403, { error: 'forbidden' });
      }
      let data;
      try {
        const raw = await new Promise((resolve, reject) => {
          const chunks = [];
          request.on('data', (c) => chunks.push(c));
          request.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
          request.on('error', reject);
        });
        data = validateAskRequest(JSON.parse(raw || '{}'));
      } catch (error) {
        return json(response, 400, { error: error instanceof Error ? error.message : 'Bad request' });
      }
      try {
        selectModel(deps.judge, data.m);
      } catch (error) {
        return json(response, 400, { error: error instanceof Error ? error.message : 'Invalid model' });
      }
      if (!deps.search1api.apiKey) {
        return json(response, 500, { error: 'Missing configuration: set SEARCH1API_API_KEY (https://search1api.com)' });
      }

      // 30s overall deadline, like the original; client disconnect aborts lanes.
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(new Error('Search timed out')), 30_000);
      request.on('close', () => controller.abort());
      response.writeHead(200, {
        'Content-Type': 'application/x-ndjson; charset=utf-8',
        'Cache-Control': 'no-store',
        'X-Accel-Buffering': 'no',
      });
      try {
        const events = await askHandler(deps, data, controller.signal);
        for await (const event of events) {
          if (controller.signal.aborted) break;
          response.write(`${JSON.stringify(event)}\n`);
        }
      } catch (error) {
        if (!controller.signal.aborted) {
          response.write(`${JSON.stringify({ type: 'error', message: error instanceof Error ? error.message : 'Search failed' })}\n`);
        }
      } finally {
        clearTimeout(timeout);
        response.end();
      }
      return;
    }

    json(response, 404, { error: 'not found' });
  };
}

export function startServer(deps, { host = '127.0.0.1', port = 3030 } = {}) {
  const server = createServer(createApp(deps));
  return new Promise((resolve) => server.listen(port, host, () => resolve(server)));
}

const isMain = process.argv[1] && import.meta.url === `file://${process.argv[1]}`;
if (isMain) {
  const env = process.env;
  const port = Number(env.PORT ?? 3030);
  const host = env.HOST ?? '127.0.0.1';
  const deps = depsFromEnv(env);
  startServer(deps, { host, port }).then(() => {
    console.log(`[laya-search] listening on http://${host}:${port}`);
    console.log(`[laya-search] judge  : ${deps.judge.baseUrl} (${deps.judge.modelId})`);
    console.log(`[laya-search] search : ${deps.search1api.apiKey ? 'Search1API key set' : 'NO SEARCH1API_API_KEY — /api/ask returns 500'}`);
  });
}
