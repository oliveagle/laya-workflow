// The page bridge must survive the executeTool signature split: newer runtimes
// and polyfills take the parsed input object; older native Chrome builds take
// the JSON string. (Observed live: "invalid input object: value is not an
// object" when a string was passed to an object-expecting ModelContext.)
import assert from "node:assert/strict";
import { test } from "node:test";
import { pageCallTool } from "../src/platform/chrome.js";

const ARGS = { items: [{ product: "banana" }] };

function withContext(executeTool) {
  const calls = [];
  const context = {
    async getTools() {
      return [{ name: "add_to_cart" }];
    },
    async executeTool(tool, input) {
      calls.push({ tool: tool.name, input });
      return executeTool(tool, input);
    },
  };
  globalThis.document = { modelContext: context };
  return calls;
}

test("bridge: object-expecting runtime (the reported bug)", async () => {
  const calls = withContext((_tool, input) => {
    if (typeof input !== "object" || input === null) {
      throw new TypeError("Failed to execute 'executeTool' on 'ModelContext': invalid input object: value is not an object");
    }
    return { content: [{ type: "text", text: `added ${input.items[0].product}` }] };
  });
  const outcome = await pageCallTool("add_to_cart", JSON.stringify(ARGS));
  assert.equal(outcome.ok, true);
  assert.match(outcome.result, /added banana/);
  // The first attempt already matched; no string fallback needed.
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].input, ARGS);
});

test("bridge: string-expecting runtime falls back to the JSON string", async () => {
  const calls = withContext((_tool, input) => {
    if (typeof input !== "string") {
      throw new TypeError("Failed to execute 'executeTool' on 'ModelContext': parameter 2 is not of type 'DOMString'");
    }
    return { content: [{ type: "text", text: "ok" }] };
  });
  const outcome = await pageCallTool("add_to_cart", JSON.stringify(ARGS));
  assert.equal(outcome.ok, true);
  assert.match(outcome.result, /ok/);
  assert.equal(calls.length, 2);
  assert.deepEqual(calls[0].input, ARGS); // object tried first
  assert.equal(calls[1].input, JSON.stringify(ARGS)); // string fallback
});

test("bridge: a genuine tool error is reported, not swallowed by the fallback", async () => {
  withContext(() => {
    throw new TypeError("the tool itself exploded");
  });
  const outcome = await pageCallTool("add_to_cart", JSON.stringify(ARGS));
  assert.equal(outcome.ok, false);
  assert.match(outcome.error, /the tool itself exploded/);
});
