@decision
Feature: code_self_repair
  Guard for LLM-proposed bounded source patches as a single triage node, port of
  the deterministic bounded-patch application and repair gates
  (code/qwen_vm/app_scenarios/code_self_repair). The model proposes only exact
  old/new fragments; this workflow validates the proposal, applies it
  transactionally, and runs the lexical, syntax, safety and unit-test gates
  before accepting any repaired source. It never patches a real file on disk.

  The engine's own verbs: the candidate source is accepted (the patch applies and
  every gate passes) or refused with the engine's own first failing gate.

  Folding boundaries (documented, not computed in the DSL):
  - The patch envelope (the balanced-object scan, the output size cap, exactly the
    keys target and patches, the assigned-target set, the non-empty fragment list,
    the per-fragment old/new shape and the per-fragment / per-bundle size and line
    caps) folds into the `proposal` choice. Because the parser already refuses an
    unassigned target and every size cap, the boundary gate's assigned-path and
    patch-size checks can never fire behind it and fold into the same choice.
  - The fixed target fixtures and their unit tests are the authority; the harness
    runs them and hands the DSL one answer per check.
  - The remaining ordered gate chain (the bounded-patch bounds, the exact-match
    application, the CPython tokenizer, the Python AST parse, the static safety
    scan and the fixed unit tests) folds into the `gate` choice, which carries the
    engine's own first failing gate. The ladder mirrors the engine's gate order.
  - The outer re-propose loop (up to MAX_RETRIES extra attempts) is the agent's,
    not the deterministic gate's, so a single evaluation is one node visit.

  Emitted labels: reject_proposal, reject_bounded_patch, reject_exact_match,
  reject_tokenize, reject_ast, reject_malicious_pattern, reject_unit_tests,
  accept.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one bounded patch proposal {target, patches} with an assigned target and a non-empty list of exact old/new fragments, each within the character and line caps and the bundle within the total new-line cap?"
      | answer    | meaning                                                                                   |
      | accepted  | exactly one structurally valid bounded patch proposal                                     |
      | malformed | no JSON object, an oversized output, a wrong key set, an unassigned or escaping target, an empty fragment list, or a fragment outside the old/new shape or size caps |
    And the answer comes from the field "ev_proposal"
    And a choice question "gate" asks "Against the fixed target fixtures, which single ordered gate refuses the applied candidate first?"
      | answer            | meaning                                                                             |
      | none              | every gate passed: the candidate source is accepted                                 |
      | bounded_patch     | a patch replaces the whole file, replaces too much of it, or grows the changed region beyond the bound |
      | exact_match       | an old fragment does not occur exactly once in the candidate                        |
      | tokenize          | the CPython tokenizer refuses the candidate                                         |
      | ast               | the candidate does not parse as Python                                              |
      | malicious_pattern | the candidate imports, calls, attributes or uses a construct on the forbidden list |
      | unit_tests        | the candidate fails a fixed unit test                                               |
    And the answer comes from the field "ev_gate"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_bounded_patch" when the answer to "gate" is "bounded_patch"
    And label "reject_exact_match" when the answer to "gate" is "exact_match"
    And label "reject_tokenize" when the answer to "gate" is "tokenize"
    And label "reject_ast" when the answer to "gate" is "ast"
    And label "reject_malicious_pattern" when the answer to "gate" is "malicious_pattern"
    And label "reject_unit_tests" when the answer to "gate" is "unit_tests"
    And label "accept" otherwise
    And keep the state key "candidate_id"
