@decision
Feature: toolcall_fix_verify
  Port of the ole-eval sglang multi-turn tool-call fix verification
  (code/inference/fixes/sglang_toolcall_history_400_20260822/verify.py): an
  end-to-end gate over the 2026-08-22 ds-local fix, which taught a patched sglang
  (native chat template, `--tool-call-parser qwen3_coder`) and its proxy (injects
  `enable_thinking=false`, heals double-encoded tool_call history arguments) to
  survive multi-turn tool loops. It is the gate that says whether the fix holds,
  so it names its verdict rather than reporting a number.

  It runs four checks, in the source's own order, each against a live endpoint:

    - A. direct memory (sglang direct, enable_thinking=false): a plain two-turn
      chat keeps memory continuity -- turn 1 returns 200, and turn 2 still knows
      the codeword turn 1 was told.
    - B. poisoned history (direct): a history whose assistant tool_call carries a
      *double-encoded* arguments string (the exact pathological encoding from the
      logs) no longer 400s -- this is the original bug.
    - C. auto tool call (direct): with no tool_choice, the model auto-emits a
      parseable XML tool call, named `bash`, whose `command` argument is exactly
      the one asked for -- the format-mismatch bug.
    - D. proxy tool loop (agent-style, no kwargs): turn 1 auto-emits a tool call;
      turn 2 replays that call (string arguments, as the client drives it) plus
      the tool result and returns 200 with a continuation that references the
      result.

  The walk encodes exactly those named checks, one node each, in source order:
  a1 -> a2 -> b -> c -> d1 -> d2 -> d3. The first check that fails ends the walk
  and names itself; only a run that clears every check reaches
  TOOLCALL_FIX_VERIFIED.

  Two folds are documented:

    - the upstream does not stop at the first failing *test*: `main` runs all
      four and prints every result, but the verdict it returns is the conjunction
      (`return 0 if all(results) else 1`). Inside a test it does early-return at
      the first failing sub-check, and D's turn 2 replays turn 1's call, so the
      graph's first-fail walk is faithful to the early-returns while naming only
      the first failure; the PASS/FAIL verdict is identical either way.
    - the four checks are live HTTP round-trips; the graph is one run of the
      verification, the unit whose exit code (0 iff all) is the verdict.

  Everything the model produced stays in the harness: the returned status codes,
  the codeword in the turn-2 text, the tool-call name and arguments, and the
  turn-2 continuation are all read there; the graph reads none of them, only the
  harness's per-check verdict.

  Reference walk (verify.py, main()): A1 turn-1 200, else A1 fails; A2 turn-2 200
  and remembers, else A2 fails; B poisoned history 200, else B fails; C 200 with a
  parseable `bash` call carrying the asked command, else C fails; D1 turn-1 has a
  tool call, else D1 fails; D2 replay 200, else D2 fails; D3 continuation mentions
  the tool result, else D3 fails; all pass -> verified.

  Background:
    Given the workflow runs at most 12 iterations

  Scenario: a1
    Given a choice question "verify" asks "Check A1: does the first direct turn return 200? A request that does not come back 200 ends the walk -- the fix is not verified."
      | answer | meaning |
      | fail | the first direct turn did not return 200 |
      | pass | the first direct turn returned 200 |
    And the answer comes from the field "ev_a1"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "a2"
    Then the primary question is "verify"
    Then label "A1_DIRECT_TURN1_FAIL" when the answer to "verify" is "fail"
    And label "A1_OK" otherwise

  Scenario: a2
    Given a choice question "verify" asks "Check A2: does the second direct turn return 200 and still know the codeword the first turn was told? The direct endpoint keeps memory continuity; a turn that loses it ends the walk."
      | answer | meaning |
      | fail | the second direct turn did not return 200, or it did not recall the codeword |
      | pass | the second direct turn returned 200 and recalled the codeword |
    And the answer comes from the field "ev_a2"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "b"
    Then the primary question is "verify"
    Then label "A2_DIRECT_MEMORY_FAIL" when the answer to "verify" is "fail"
    And label "A2_OK" otherwise

  Scenario: b
    Given a choice question "verify" asks "Check B: does a direct request whose assistant tool_call carries a double-encoded arguments string return 200? This was the original 400; a request that still 400s ends the walk."
      | answer | meaning |
      | fail | the poisoned-history request did not return 200 |
      | pass | the poisoned-history request returned 200 |
    And the answer comes from the field "ev_b"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "c"
    Then the primary question is "verify"
    Then label "B_POISONED_HISTORY_FAIL" when the answer to "verify" is "fail"
    And label "B_OK" otherwise

  Scenario: c
    Given a choice question "verify" asks "Check C: with no tool_choice, does a direct request auto-emit a parseable `bash` tool call whose `command` argument is exactly the one asked for? A missing or wrong call ends the walk."
      | answer | meaning |
      | fail | no parseable bash call was emitted, or its command argument was wrong |
      | pass | a parseable bash call with the exact asked command was emitted |
    And the answer comes from the field "ev_c"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "d1"
    Then the primary question is "verify"
    Then label "C_AUTO_TOOLCALL_FAIL" when the answer to "verify" is "fail"
    And label "C_OK" otherwise

  Scenario: d1
    Given a choice question "verify" asks "Check D1: does the proxy's first turn auto-emit a tool call? Turn 2 replays this very call, so a turn with no tool call ends the walk."
      | answer | meaning |
      | fail | the proxy's first turn did not return 200 or emitted no tool call |
      | pass | the proxy's first turn emitted a tool call |
    And the answer comes from the field "ev_d1"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "d2"
    Then the primary question is "verify"
    Then label "D1_PROXY_TURN1_FAIL" when the answer to "verify" is "fail"
    And label "D1_OK" otherwise

  Scenario: d2
    Given a choice question "verify" asks "Check D2: does the proxy's second turn -- replaying turn 1's tool call (string arguments, as the client drives it) plus the tool result -- return 200? This was the second 400; a replay that 400s ends the walk."
      | answer | meaning |
      | fail | the proxy's replay request did not return 200 |
      | pass | the proxy's replay request returned 200 |
    And the answer comes from the field "ev_d2"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "d3"
    Then the primary question is "verify"
    Then label "D2_PROXY_REPLAY_FAIL" when the answer to "verify" is "fail"
    And label "D2_OK" otherwise

  Scenario: d3
    Given a choice question "verify" asks "Check D3: does the proxy's second-turn continuation reference the tool result? This is the last check: a continuation that forgets the result ends the walk, and clearing every check verifies the fix."
      | answer | meaning |
      | fail | the continuation did not reference the tool result |
      | pass | the continuation referenced the tool result |
    And the answer comes from the field "ev_d3"
    When any other answer goes to "STOP"
    Then the primary question is "verify"
    Then label "D3_PROXY_CONTINUITY_FAIL" when the answer to "verify" is "fail"
    And label "TOOLCALL_FIX_VERIFIED" otherwise
