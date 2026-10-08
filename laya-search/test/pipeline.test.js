// Provider-independent pipeline tests: the judge and Search1API are mocked at
// the fetch boundary, so no keys or running servers are needed.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { askStream, runSearch } from '../src/pipeline.js';
import { memoryCache } from '../src/cache.js';
import { buildCandidates } from '../src/candidates.js';

const deps = () => ({
  search1api: { apiKey: 'test-key', baseUrl: 'http://s1a.test' },
  judge: { baseUrl: 'http://laya.test', modelId: 'laya-mlx' },
  cache: memoryCache(),
  now: () => new Date('2026-10-07T00:00:00Z'),
});

const spread = (keys, pick, p = 0.9) =>
  Object.fromEntries(keys.map((k) => [k, k === pick ? p : (1 - p) / Math.max(keys.length - 1, 1)]));

/** Route judge + search calls; record what the pipeline asked for. */
function mockProviders({ window = '7d', sources = {}, query = 'c1', entity = 'c2', relevance = 0.9, results } = {}) {
  const calls = { judge: [], search: [] };
  const original = globalThis.fetch;
  globalThis.fetch = async (url, init = {}) => {
    const target = String(url);
    if (target.startsWith('http://laya.test/v1/systemone')) {
      const body = JSON.parse(init.body);
      calls.judge.push(body);
      const answers = {};
      for (const [id, q] of Object.entries(body.questions)) {
        if (id === 'window') answers[id] = { type: 'choice', choice: window, probabilities: spread(Object.keys(q.criteria), window), confidence: 0.9 };
        else if (id === 'query') answers[id] = { type: 'choice', choice: query, probabilities: spread(Object.keys(q.criteria), query), confidence: 0.9 };
        else if (id === 'entity') answers[id] = { type: 'choice', choice: entity, probabilities: spread(Object.keys(q.criteria), entity), confidence: 0.8 };
        else if (id.startsWith('source_')) answers[id] = { type: 'noul', noul: sources[id.slice(7)] ?? 0.02 };
        else if (id.startsWith('r')) answers[id] = { type: 'noul', noul: relevance };
      }
      return new Response(JSON.stringify({ model: 'laya-mlx', answers, usage: { input_tokens: 10, output_tokens: 0 } }), { status: 200 });
    }
    if (target.startsWith('http://s1a.test/')) {
      const body = JSON.parse(init.body);
      calls.search.push(body);
      return new Response(JSON.stringify({ results: results ?? [] }), { status: 200 });
    }
    return original(url, init);
  };
  return { calls, restore: () => (globalThis.fetch = original) };
}

test('candidates: time, source and filler phrases are stripped', () => {
  const out = buildCandidates('What are people saying about Rust async runtimes on Hacker News this month?');
  assert.ok(out[0].includes('What are people saying'));
  assert.ok(out.length > 1);
  const stripped = out.slice(1).join(' | ').toLowerCase();
  assert.ok(!stripped.includes('this month'));
  assert.ok(stripped.includes('rust async runtimes'));
});

test('pipeline: intent → lanes → done, with judge choices applied', async () => {
  const d = deps();
  const mock = mockProviders({
    window: '7d',
    sources: { google: 0.9, duckduckgo: 0.9, yandex: 0.9, hackernews: 0.95, reddit: 0.05 },
    query: 'c1',
    entity: 'c2',
    relevance: 0.9,
    results: [
      { title: 'Tokio vs async-std', link: 'https://example.com/rust', snippet: '3 days ago A comparison of Rust async runtimes', published_date: '2026-10-05' },
      { title: 'Duplicate', link: 'https://example.com/rust?utm_source=x', snippet: 'same story', published_date: '2026-10-05' },
    ],
  });
  try {
    const events = [];
    for await (const event of askStream(d, { request: 'Rust async runtimes on Hacker News this month' })) events.push(event);
    const types = events.map((e) => e.type);
    assert.equal(types[0], 'intent');
    assert.equal(types.at(-1), 'done');

    const intent = events[0];
    assert.equal(intent.window, '7d');
    assert.ok(intent.sources.includes('hackernews'));
    assert.ok(!intent.sources.includes('reddit'));
    assert.equal(intent.judge, 'laya-mlx');
    assert.ok(intent.candidates.length >= 1);

    const done = events.at(-1);
    assert.equal(typeof done.totalMs, 'number');
    assert.ok(done.tokens >= 0);

    // The mocked search answered for every lane of every selected source.
    const services = new Set(mock.calls.search.map((c) => c.search_service));
    assert.ok(services.has('google'));
    assert.ok(services.has('hackernews'));
    // The window the judge chose became a time_range on the engines.
    assert.ok(mock.calls.search.some((c) => c.time_range === 'week'));
  } finally {
    mock.restore();
  }
});

test('runSearch: ranks, folds duplicate URLs and reports lanes', async () => {
  const d = deps();
  const mock = mockProviders({
    query: 'c0',
    entity: 'c0',
    relevance: 0.75,
    results: [
      { title: 'Result A', link: 'https://a.test/1', snippet: 'about the subject' },
      { title: 'Result A again', link: 'https://a.test/1?utm_source=google', snippet: 'same url, tracking noise' },
      { title: 'Result B', link: 'https://b.test/2', snippet: 'also about it' },
    ],
  });
  try {
    const out = await runSearch(d, { request: 'rust async runtimes' });
    assert.equal(out.query, out.candidates[0]);
    const urls = out.items.map((it) => it.url);
    assert.equal(urls.filter((u) => u.startsWith('https://a.test/1')).length, 1, 'duplicate URL folded');
    assert.equal(out.items.length, 2);
    assert.ok(out.items.every((it) => it.ranked && it.relevance === 0.75));
    assert.ok(out.lanes.length >= 1);
    assert.equal(out.judge, 'laya-mlx');
  } finally {
    mock.restore();
  }
});

test('pipeline: a failing engine does not fail the request', async () => {
  const d = deps();
  const mock = mockProviders({ query: 'c0', entity: 'c0' });
  const original = globalThis.fetch;
  globalThis.fetch = async (url, init) => {
    const target = String(url);
    if (target.startsWith('http://s1a.test/')) {
      const body = JSON.parse(init.body);
      if (body.search_service !== 'google') {
        return new Response('engine exploded', { status: 502 });
      }
      return new Response(JSON.stringify({ results: [{ title: 'OK', link: 'https://ok.test/1', snippet: 'fine' }] }), { status: 200 });
    }
    return original(url, init);
  };
  try {
    const out = await runSearch(d, { request: 'rust async runtimes' });
    assert.ok(out.items.length >= 1);
    assert.ok(out.errors.length > 0);
    assert.match(out.errors[0].message, /502|exploded/);
  } finally {
    globalThis.fetch = original;
    mock.restore();
  }
});
