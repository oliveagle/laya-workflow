// Request validation and handler contracts — the jev-search API surface.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { modelsHandler, sameOrigin, selectModel, validateAskRequest } from '../src/api.js';

test('validate: q must be a non-empty string up to 300 characters', () => {
  assert.throws(() => validateAskRequest({}), /q must be a non-empty string/);
  assert.throws(() => validateAskRequest({ q: '   ' }), /q must be a non-empty string/);
  assert.throws(() => validateAskRequest({ q: 'x'.repeat(301) }), /q must be a non-empty string/);
  assert.deepEqual(validateAskRequest({ q: '  hello  ' }), { q: 'hello' });
});

test('validate: s is capped, deduped and filtered; invalid w is dropped; m is checked', () => {
  const thirteen = Array.from({ length: 13 }, (_, i) => `src${i}`);
  assert.throws(() => validateAskRequest({ q: 'hi', s: thirteen }), /s must contain at most 12 entries/);
  assert.deepEqual(validateAskRequest({ q: 'hi', s: ['google', 'google', 'nope', 'reddit'] }).s, ['google', 'reddit']);
  assert.equal(validateAskRequest({ q: 'hi', w: 'bogus' }).w, undefined);
  assert.equal(validateAskRequest({ q: 'hi', w: '7d' }).w, '7d');
  assert.throws(() => validateAskRequest({ q: 'hi', m: '' }), /m must be a model id/);
});

test('selectModel: only the configured laya model is accepted', () => {
  const config = { baseUrl: 'http://laya.test', modelId: 'laya-mlx' };
  assert.equal(selectModel(config, undefined), config);
  assert.equal(selectModel(config, 'laya-mlx'), config);
  assert.throws(() => selectModel(config, 'jev-latest'), /Invalid model/);
});

test('sameOrigin: browser origins are enforced, plain clients are allowed', () => {
  assert.equal(sameOrigin('http://x.test/api/ask', 'http://x.test', undefined), true);
  assert.equal(sameOrigin('http://x.test/api/ask', 'http://evil.test', undefined), false);
  assert.equal(sameOrigin('http://x.test/api/ask', undefined, 'same-origin'), true);
  assert.equal(sameOrigin('http://x.test/api/ask', undefined, 'cross-site'), false);
  assert.equal(sameOrigin('http://x.test/api/ask', undefined, undefined), true);
});

test('modelsHandler: 200 with the laya model when the judge is healthy, 503 otherwise', async () => {
  const original = globalThis.fetch;
  const deps = { judge: { baseUrl: 'http://laya.test', modelId: 'laya-mlx' } };
  globalThis.fetch = async () => new Response(JSON.stringify({ ok: true }), { status: 200 });
  assert.deepEqual(await modelsHandler(deps), { status: 200, body: { models: ['laya-mlx'] } });
  globalThis.fetch = async () => new Response('down', { status: 503 });
  const down = await modelsHandler(deps);
  assert.equal(down.status, 503);
  assert.deepEqual(down.body, { error: 'Models are unavailable' });
  globalThis.fetch = original;
});
