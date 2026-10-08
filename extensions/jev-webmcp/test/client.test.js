// The client against a real local HTTP server shaped exactly like
// `laya-mlx --serve` — no Chrome and no TypeSafe API involved.
import assert from "node:assert/strict";
import { test } from "node:test";
import { createServer } from "node:http";
import { buildQuestions } from "../src/core/questions.js";
import { answersFor, basketful } from "./helpers.js";
import { checkServer, systemOne } from "../src/jev.js";

function mockServer() {
  return createServer((request, response) => {
    const json = (status, body) => {
      response.writeHead(status, { "Content-Type": "application/json", "Access-Control-Allow-Origin": "*" });
      response.end(JSON.stringify(body));
    };
    if (request.method === "GET" && request.url === "/health") return json(200, { ok: true, model: "laya-mlx", device: "mlx (gpu)" });
    if (request.method === "OPTIONS") return json(204, {});
    if (request.method === "POST" && request.url === "/v1/systemone") {
      let raw = "";
      request.on("data", (chunk) => (raw += chunk));
      request.on("end", () => {
        const body = JSON.parse(raw);
        if (!body.questions) return json(400, { detail: "missing questions" });
        const { questions } = buildQuestions(basketful, "got anything gluten free in the bakery aisle?");
        const answers = answersFor(questions, { tool: "search_products", answers: { "search_products::department": ["Bakery", 0.98] } });
        json(200, { model: "laya-rl-agent", answers, usage: { input_tokens: 208, output_tokens: 0 }, ms: 291.9 });
      });
      return;
    }
    json(404, { detail: "not found" });
  });
}

test("systemOne: posts to the local server shape and returns answers", async () => {
  const server = mockServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const baseUrl = `http://127.0.0.1:${server.address().port}`;
  try {
    const { questions } = buildQuestions(basketful, "got anything gluten free in the bakery aisle?");
    const result = await systemOne({ baseUrl, state: { user_request: "gluten free bakery" }, questions });
    assert.equal(result.model, "laya-rl-agent");
    assert.equal(result.usage.input_tokens, 208);
    assert.ok(result.ms >= 0);
    assert.ok(result.answers["__tool__"].choice);
  } finally {
    server.close();
  }
});

test("checkServer: health endpoint, and a clear error when the server is down", async () => {
  const server = mockServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const baseUrl = `http://127.0.0.1:${server.address().port}`;
  try {
    assert.equal(await checkServer(baseUrl), true);
  } finally {
    server.close();
  }
  await assert.rejects(() => checkServer(baseUrl), (error) => {
    assert.equal(error.name, "JevError");
    assert.match(error.message, /Cannot reach laya-mlx/);
    return true;
  });
});
