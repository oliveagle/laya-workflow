// End-to-end over real HTTP: the NDJSON contract of POST /api/ask and
// GET /api/models, with the judge and Search1API mocked at the fetch boundary.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { depsFromEnv, startServer } from '../src/server.js';

function mockProviders({ relevance = 0.9 } = {}) {
  const original = globalThis.fetch;
  globalThis.fetch = async (url, init = {}) => {
    const target = String(url);
    if (target.startsWith('http://laya.test/v1/systemone')) {
      const body = JSON.parse(init.body);
      const answers = {};
      for (const [id, q] of Object.entries(body.questions)) {
        if (id === 'window') answers[id] = { type: 'choice', choice: 'any', probabilities: { any: 0.95 }, confidence: 0.95 };
        else if (id === 'query' || id === 'entity') {
          const keys = Object.keys(q.criteria);
          answers[id] = { type: 'choice', choice: keys[0], probabilities: Object.fromEntries(keys.map((k) => [k, 1 / keys.length])), confidence: 0.5 };
        } else if (id.startsWith('source_')) answers[id] = { type: 'noul', noul: 0.95 };
        else answers[id] = { type: 'noul', noul: relevance };
      }
      return new Response(JSON.stringify({ model: 'laya-mlx', answers, usage: { input_tokens: 7, output_tokens: 0 } }), { status: 200 });
    }
    if (target.startsWith('http://laya.test/health')) {
      return new Response(JSON.stringify({ ok: true, model: 'laya-mlx' }), { status: 200 });
    }
    if (target.startsWith('http://s1a.test/')) {
      return new Response(JSON.stringify({
        results: [
          { title: 'Result &amp; one', link: 'https://one.test/a?utm_source=g', snippet: '3 days ago relevant snippet', published_date: '2026-10-05' },
          { title: 'Result two', link: 'https://two.test/b', snippet: 'also relevant' },
        ],
      }), { status: 200 });
    }
    return original(url, init);
  };
  return () => (globalThis.fetch = original);
}

test('server: /api/models and the /api/ask NDJSON contract', async () => {
  const restore = mockProviders();
  const deps = depsFromEnv({ LAYA_JUDGE_URL: 'http://laya.test', SEARCH1API_API_KEY: 'k', SEARCH1API_BASE_URL: 'http://s1a.test' });
  const server = await startServer(deps, { port: 0 });
  const base = `http://127.0.0.1:${server.address().port}`;
  try {
    const models = await (await fetch(`${base}/api/models`)).json();
    assert.deepEqual(models, { models: ['laya-mlx'] });

    // validation
    const bad = await fetch(`${base}/api/ask`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ q: '' }) });
    assert.equal(bad.status, 400);
    const badModel = await fetch(`${base}/api/ask`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ q: 'hi', m: 'jev-latest' }) });
    assert.equal(badModel.status, 400);

    // NDJSON stream
    const res = await fetch(`${base}/api/ask`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ q: 'rust async runtimes' }) });
    assert.equal(res.status, 200);
    assert.match(res.headers.get('content-type'), /application\/x-ndjson/);
    const text = await res.text();
    const events = text.trim().split('\n').map((line) => JSON.parse(line));
    assert.equal(events[0].type, 'intent');
    assert.equal(events[0].judge, 'laya-mlx');
    assert.ok(events[0].sources.includes('google'));
    assert.ok(events.some((e) => e.type === 'lane' && e.items.length > 0));
    assert.ok(events.at(-1).type === 'done');
    const lane = events.find((e) => e.type === 'lane' && e.items.length > 0);
    assert.equal(lane.items[0].relevance, 0.9);
    assert.ok(lane.items[0].ranked);
  } finally {
    restore();
    server.close();
  }
});

test('server: missing Search1API key is a 500 before any provider call', async () => {
  const restore = mockProviders();
  let judgeCalled = false;
  const original = globalThis.fetch;
  globalThis.fetch = async (url, init) => {
    if (String(url).startsWith('http://laya.test/v1/systemone')) judgeCalled = true;
    return original(url, init);
  };
  const deps = depsFromEnv({ LAYA_JUDGE_URL: 'http://laya.test' });
  const server = await startServer(deps, { port: 0 });
  const base = `http://127.0.0.1:${server.address().port}`;
  try {
    const res = await fetch(`${base}/api/ask`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ q: 'test' }) });
    assert.equal(res.status, 500);
    assert.match((await res.json()).error, /SEARCH1API_API_KEY/);
    assert.equal(judgeCalled, false);
  } finally {
    restore();
    server.close();
  }
});
