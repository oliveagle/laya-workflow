// The decision layer: the same two judgments jev-search asks of Jev
// (inferIntent + rerank), answered by the locally deployed laya-mlx server
// (`laya-mlx --serve`, POST /v1/systemone) instead of TypeSafe.
//
// The wire protocol is the same decision dialect — {state, questions} in,
// {model, answers, usage} out — so the question wording from
// superagents-lab/jev-search (MIT) carries over unchanged.
import { SOURCES, WINDOWS } from './sources.js';

/** The single configured provider: a local laya-mlx server. */
export const JUDGE_PROVIDER = 'laya-mlx';

export class JudgeError extends Error {
  constructor(status, message) {
    super(message);
    this.name = 'JudgeError';
    this.status = status;
  }
}

export function judgeConfigFromEnv(env = process.env) {
  return {
    baseUrl: (env.LAYA_JUDGE_URL ?? 'http://127.0.0.1:8400').trim().replace(/\/+$/, ''),
    modelId: env.LAYA_MODEL_ID ?? 'laya-mlx',
  };
}

export async function health(config = judgeConfigFromEnv()) {
  const response = await fetch(`${config.baseUrl}/health`, { signal: AbortSignal.timeout(3000) });
  if (!response.ok) throw new JudgeError(response.status, `laya-mlx /health returned ${response.status}`);
  return response.json();
}

async function systemOne(config, state, questions, signal) {
  let response;
  try {
    response = await fetch(`${config.baseUrl}/v1/systemone`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ state, model: config.modelId, questions }),
      signal,
    });
  } catch (error) {
    if (signal?.aborted) throw error;
    throw new JudgeError(0, `Cannot reach laya-mlx at ${config.baseUrl}. Start it with: laya-mlx --serve`);
  }
  if (!response.ok) {
    let detail = '';
    try {
      const body = await response.json();
      detail = typeof body.detail === 'string' ? body.detail : (body.error?.message ?? body.message ?? '');
    } catch {}
    throw new JudgeError(response.status, [`laya-mlx returned ${response.status}.`, detail].filter(Boolean).join(' '));
  }
  const body = await response.json();
  return {
    model: body.model ?? config.modelId,
    provider: JUDGE_PROVIDER,
    answers: body.answers ?? {},
    usage: {
      input_tokens: body.usage?.input_tokens ?? 0,
      output_tokens: body.usage?.output_tokens ?? 0,
    },
  };
}

// ---------------------------------------------------------------------------
// Judgment 1: what does the request ask for?
// ---------------------------------------------------------------------------

export async function inferIntent(config, input, signal) {
  const questions = {};

  // laya adaptation: the local model separates options far more sharply with
  // short criteria than with the long Jev wording (long texts evict the
  // request from its 512-token window). Measured on "…on Hacker News this
  // month": hackernews 53% -> 93%, reddit 84%, github 72%, arxiv 10%.
  const windowCriteria = { any: 'No time preference', '24h': 'Today', '7d': 'Past week', '30d': 'Past month' };
  questions.window = {
    type: 'choice',
    instructions:
      'Does the request in `request` ask for recent results, and if so how recent? Judge only from what the request says or clearly implies; `now` is the current date. A request with no time cue wants any time.',
    criteria: windowCriteria,
  };

  for (const s of SOURCES) {
    questions[`source_${s.id}`] = {
      type: 'noul',
      instructions: `About \`request\`: ${s.ask.question}`,
      criteria: { true: 'Yes, this source fits the request', false: 'No, this source does not fit the request' },
    };
  }

  if (input.candidates.length > 1) {
    const criteria = {};
    input.candidates.forEach((c, i) => {
      criteria[`c${i}`] = c;
    });
    questions.query = {
      type: 'choice',
      instructions:
        'Which candidate in `candidates` is the best keyword query to send to a web search engine so the results match what the user is asking for in `request`? Prefer the candidate that keeps the subject and drops words about time, sources or phrasing that a search engine would treat as keywords.',
      criteria,
    };
    questions.entity = {
      type: 'choice',
      instructions:
        'Which candidate in `candidates` is just the name or title of the thing the user is asking about in `request`, as you would type it into a catalogue such as IMDb or a library index? Prefer the shortest candidate that is still the full proper name.',
      criteria,
    };
  }

  const state = {
    request: input.request,
    now: input.now.toISOString().slice(0, 10),
    candidates: Object.fromEntries(input.candidates.map((c, i) => [`c${i}`, c])),
  };

  const res = await systemOne(config, state, questions, signal);

  const windowAnswer = res.answers.window;
  const window =
    windowAnswer?.type === 'choice'
      ? { choice: windowAnswer.choice, confidence: windowAnswer.confidence ?? 0 }
      : { choice: 'any', confidence: 0 };

  const sources = {};
  for (const s of SOURCES) {
    const a = res.answers[`source_${s.id}`];
    sources[s.id] = a?.type === 'noul' ? a.noul : 0;
  }

  const queryAnswer = res.answers.query;
  const query =
    queryAnswer?.type === 'choice'
      ? { index: Number(queryAnswer.choice.slice(1)) || 0, confidence: queryAnswer.confidence ?? 0 }
      : { index: 0, confidence: 1 };

  const entityAnswer = res.answers.entity;
  const entity =
    entityAnswer?.type === 'choice'
      ? { index: Number(entityAnswer.choice.slice(1)) || 0, confidence: entityAnswer.confidence ?? 0 }
      : query;

  return { window, sources, query, entity, usage: res.usage, provider: res.provider };
}

// ---------------------------------------------------------------------------
// Judgment 2: is each result about what was asked?
// ---------------------------------------------------------------------------

const RERANK_BATCH = 40;

export async function rerank(config, request, items, signal) {
  const relevance = {};
  const usage = { input_tokens: 0, output_tokens: 0 };
  if (items.length === 0) return { relevance, usage };

  const batches = [];
  for (let i = 0; i < items.length; i += RERANK_BATCH) {
    batches.push(items.slice(i, i + RERANK_BATCH));
  }

  const responses = await Promise.all(
    batches.map((batch) => {
      const questions = {};
      batch.forEach((_, i) => {
        questions[`r${i}`] = {
          type: 'noul',
          instructions: `Is \`results[${i}]\` about the subject the user asked for in \`request\`?`,
          criteria: {
            true: 'The title or snippet discusses the same subject the user asked about, even briefly or as one of several topics',
            false: 'The result is about something else that only shares words with the request (a different meaning of the same word, a different product, a person with the same name) or is unrelated',
          },
        };
      });
      const state = {
        request,
        results: batch.map((it) => ({ source: it.source, title: it.title, snippet: it.snippet })),
      };
      return systemOne(config, state, questions, signal);
    })
  );

  responses.forEach((res, b) => {
    usage.input_tokens += res.usage.input_tokens;
    usage.output_tokens += res.usage.output_tokens;
    batches[b].forEach((item, i) => {
      const a = res.answers[`r${i}`];
      relevance[item.id] = a?.type === 'noul' ? a.noul : 0;
    });
  });

  return { relevance, usage };
}
