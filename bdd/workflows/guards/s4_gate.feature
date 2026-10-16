@decision
Feature: s4_gate
  Port of the ole-eval S4 GATE v2 (code/dllm/eval/sanity_ar_base.py): the coherence
  gate that decides whether a *decoded code generation* is a real sample rather than
  the "broken base" degeneration the P1 diagnosis pinned down (meaningless characters
  repeated at random). It is five predicates over the completion text, and the gate
  passes only when all five hold:

    (a) the longest AST-parseable Python prefix is at least 50 characters and holds at
        least one def/class -- the model produced something structural;
    (b) the top-4gram whitespace-token repeat ratio is below 0.3 -- it is not looping;
    (c) the share of CJK "garbage" characters (Unicode category `Lo` above U+4E00) is
        below 0.05 -- it is not spewing nonsense glyphs;
    (d) at least 3 of the Python keywords `def `/`class `/`return `/`if `/`for ` appear;
    (e) the completion holds at least 500 whitespace tokens -- it did not stop at once.

  The walk encodes exactly that ladder in the gate's own order: a, then b, then c,
  then d, then e. The first predicate that fails ends the walk and names itself; only
  a walk that clears all five reaches S4_PASS. That matches the source's
  `ok = a_ok and b_ok and c_ok and d_ok and e_ok` and its `all_pass` over three prompts:
  a single failing check is a failing gate.

  One fold is documented: each predicate is computed from the decoded text (an AST
  walk, a Counter over 4-grams, a unicodedata scan), so "check (a) passed" -- the
  predicate's own verdict -- is the graph's observable input, and the graph reads no
  number the model wrote. The thresholds themselves live in the harness, not the graph.

  One stale header is recorded, and the port follows the *code*: the module docstring
  still says (e) is `>= 1000` tokens and (b) "broken base 通常 >0.5", while the code
  below it reads `n_tokens >= 500` and `repeat_ratio < 0.3`. The code is authoritative;
  the docstring drifted.

  Reference walk (code/dllm/eval/sanity_ar_base.py, the `# === 新判据 ===` block):
  a_ok = def_count >= 1 and len(longest) >= 50; b_ok = repeat_ratio < 0.3;
  c_ok = garbage_ratio < 0.05; d_ok = py_kw >= 3; e_ok = n_tokens >= 500; then
  ok = a_ok and b_ok and c_ok and d_ok and e_ok.

  Background:
    Given the workflow runs at most 6 iterations

  Scenario: a
    Given a choice question "check" asks "(a) Does the longest AST-parseable Python prefix run to at least 50 characters and hold at least one def/class?"
      | answer | meaning |
      | pass | the longest parseable prefix is >= 50 chars and holds >= 1 def/class |
      | fail | it is shorter, or holds no def/class |
    And the answer comes from the field "ev_a"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "b"
    Then the primary question is "check"
    Then label "S4_FAIL_A" when the answer to "check" is "fail"
    And label "S4_A_OK" otherwise

  Scenario: b
    Given a choice question "check" asks "(b) Is the top-4gram whitespace-token repeat ratio below 0.3?"
      | answer | meaning |
      | pass | the most frequent 4-gram covers less than 0.3 of all 4-grams |
      | fail | it covers 0.3 or more -- the text is looping |
    And the answer comes from the field "ev_b"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "c"
    Then the primary question is "check"
    Then label "S4_FAIL_B" when the answer to "check" is "fail"
    And label "S4_B_OK" otherwise

  Scenario: c
    Given a choice question "check" asks "(c) Is the share of CJK garbage characters (Unicode Lo above U+4E00) below 0.05?"
      | answer | meaning |
      | pass | garbage characters are under 5% of the completion |
      | fail | 5% or more -- the text is nonsense glyphs |
    And the answer comes from the field "ev_c"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "d"
    Then the primary question is "check"
    Then label "S4_FAIL_C" when the answer to "check" is "fail"
    And label "S4_C_OK" otherwise

  Scenario: d
    Given a choice question "check" asks "(d) Do at least 3 of the Python keywords def/class/return/if/for appear in the completion?"
      | answer | meaning |
      | pass | 3 or more of the five keywords appear |
      | fail | fewer than 3 appear |
    And the answer comes from the field "ev_d"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "e"
    Then the primary question is "check"
    Then label "S4_FAIL_D" when the answer to "check" is "fail"
    And label "S4_D_OK" otherwise

  Scenario: e
    Given a choice question "check" asks "(e) Does the completion hold at least 500 whitespace tokens? This is the last gate: clearing it makes the S4 GATE pass."
      | answer | meaning |
      | pass | 500 or more whitespace tokens |
      | fail | fewer than 500 tokens -- the model stopped almost immediately |
    And the answer comes from the field "ev_e"
    When any other answer goes to "STOP"
    Then the primary question is "check"
    Then label "S4_PASS" when the answer to "check" is "pass"
    And label "S4_FAIL_E" otherwise
