// Request validation and the /api handlers, mirroring jev-search's contract:
// POST /api/ask → NDJSON (intent, found, lane, done, error) and
// GET /api/models → {models}. Adapted from superagents-lab/jev-search (MIT).
import { SOURCE_IDS, isSourceId, isWindowId } from './sources.js';
import { askStream } from './pipeline.js';
import { health, JUDGE_PROVIDER } from './judge.js';

export function validateAskRequest(input) {
  if (typeof input !== 'object' || input === null) throw new Error('Invalid input');
  const { q, w, s, m } = input;
  if (typeof q !== 'string' || q.trim().length === 0 || q.length > 300) {
    throw new Error('q must be a non-empty string up to 300 characters');
  }
  const out = { q: q.trim() };
  if (m !== undefined) {
    if (typeof m !== 'string' || m.length === 0 || m.length > 200) throw new Error('m must be a model id');
    out.m = m;
  }
  if (typeof w === 'string' && isWindowId(w)) out.w = w;
  if (Array.isArray(s)) {
    // Bound the raw list before filtering so duplicates and invalid entries count too.
    if (s.length > SOURCE_IDS.length) {
      throw new Error(`s must contain at most ${SOURCE_IDS.length} entries`);
    }
    const ids = [...new Set(s.filter((v) => typeof v === 'string' && isSourceId(v)))];
    if (ids.length > 0) out.s = ids;
  }
  return out;
}

/** The one model this deployment serves; `m` must be it or absent. */
export function selectModel(config, m) {
  if (m !== undefined && m !== config.modelId) throw new Error('Invalid model');
  return config;
}

export function sameOrigin(requestUrl, origin, secFetchSite) {
  if (origin) return origin === new URL(requestUrl).origin;
  if (secFetchSite) return secFetchSite === 'same-origin';
  return true; // non-browser clients (curl, tests) carry neither header
}

/** GET /api/models — 503 when the local judge is unreachable. */
export async function modelsHandler(deps) {
  try {
    await health(deps.judge);
    return { status: 200, body: { models: [deps.judge.modelId] } };
  } catch {
    return { status: 503, body: { error: 'Models are unavailable' } };
  }
}

/**
 * POST /api/ask — one JSON event per line, in the order they happen:
 * intent, then each source as it finishes, then done.
 */
export async function askHandler(deps, data, signal) {
  const generator = askStream(
    { search1api: deps.search1api, judge: deps.judge, cache: deps.cache },
    { request: data.q, window: data.w, sources: data.s },
    signal
  );
  return generator;
}

export { JUDGE_PROVIDER };
