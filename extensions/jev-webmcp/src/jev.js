// Client for a locally deployed `laya-mlx` server (`laya-mlx --serve`).
// Same wire shape as the TypeSafe System One endpoint, but it runs on your
// machine's Apple GPU — no API key, no cloud, no per-token cost.
const normalize = (baseUrl) => String(baseUrl || "").trim().replace(/\/+$/, "");

export class JevError extends Error {
  constructor(message, status) {
    super(message);
    this.name = "JevError";
    this.status = status;
  }
}

const REASONS = {
  400: "laya-mlx rejected the questions.",
  404: "That path does not exist on the laya-mlx server. Is it an older build? Rebuild and restart it.",
  405: "Wrong HTTP method for the laya-mlx server.",
  500: "laya-mlx failed to answer the questions.",
  503: "The laya-mlx model worker is not running. Restart the server.",
  504: "laya-mlx timed out answering the questions.",
};

async function explain(response) {
  let detail = "";
  try {
    const body = await response.json();
    detail = typeof body.detail === "string" ? body.detail : (body.error?.message ?? body.message ?? "");
  } catch {}
  return [REASONS[response.status] ?? `laya-mlx returned ${response.status}.`, detail].filter(Boolean).join(" ");
}

export async function systemOne({ baseUrl, state, questions, signal }) {
  const started = performance.now();
  let response;
  try {
    response = await fetch(`${normalize(baseUrl)}/v1/systemone`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ state, questions }),
      signal,
    });
  } catch (error) {
    if (error.name === "AbortError") throw error;
    throw new JevError(`Cannot reach laya-mlx at ${normalize(baseUrl)}. Is it running? (laya-mlx --serve)`, 0);
  }
  if (!response.ok) throw new JevError(await explain(response), response.status);
  const body = await response.json();
  return { answers: body.answers, usage: body.usage, model: body.model ?? "laya-mlx", ms: performance.now() - started };
}

export async function checkServer(baseUrl) {
  let response;
  try {
    response = await fetch(`${normalize(baseUrl)}/health`);
  } catch {
    throw new JevError(`Cannot reach laya-mlx at ${normalize(baseUrl)}. Start it with: laya-mlx --serve`, 0);
  }
  if (!response.ok) throw new JevError(await explain(response), response.status);
  return true;
}
