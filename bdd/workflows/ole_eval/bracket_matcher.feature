@decision
Feature: bracket_matcher
  Guard for LLM-proposed Python snippets as a single triage node. The model
  proposes only a candidate snippet; this workflow owns the bracket authority
  (a CPython tokenizer with a manual lexical fallback scanner) and returns the
  accept / reject decision itself together with the engine's own first bracket
  issue. Port of code/qwen_vm/app_scenarios/bracket_matcher.

  The engine's own verbs: accepted / rejected — the snippet passes bracket
  validation, or it is rejected with one or more issues.

  Folding boundaries (documented, not computed in the DSL):
  - Snippet extraction (the completed-reasoning split, the fenced-code match and
    the code-start heuristic) and the tokenizer fallback both fold into the
    `proposal` choice.
  - The bracket scan (openers / closers, string- and comment-skipping, type
    matching, the depth cap of 32) is the authority; the harness runs it and
    hands the DSL one answer per check. The engine appends every issue; the port
    carries only the first-appended issue code, so the reject ladder mirrors the
    engine's own scan order (a lexical tokenizer failure first, then
    depth / extra-close / wrong-order in scan order, then the unclosed openers).

  Emitted labels: reject_proposal, reject_lexical_error, reject_depth_exceeded,
  reject_extra_close, reject_wrong_order, reject_unclosed, accept.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Did the model response yield exactly one extractable Python snippet for bracket validation?"
      | answer    | meaning                                              |
      | accepted  | exactly one extractable snippet                      |
      | malformed | no snippet could be extracted, or the transport failed |
    And the answer comes from the field "ev_proposal"
    And a choice question "reject_reason" asks "What is the engine's first-appended bracket issue code for this snippet?"
      | answer         | meaning                                                  |
      | none           | no bracket issue: balanced and within the depth limit    |
      | lexical_error  | the CPython tokenizer could not scan the full source      |
      | depth_exceeded | nesting depth exceeds the limit of 32                     |
      | extra_close    | a closing bracket has no active opening bracket           |
      | wrong_order    | a closing bracket does not match the active opening       |
      | unclosed       | an opening bracket is never closed                        |
    And the answer comes from the field "ev_reject_reason"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_lexical_error" when the answer to "reject_reason" is "lexical_error"
    And label "reject_depth_exceeded" when the answer to "reject_reason" is "depth_exceeded"
    And label "reject_extra_close" when the answer to "reject_reason" is "extra_close"
    And label "reject_wrong_order" when the answer to "reject_reason" is "wrong_order"
    And label "reject_unclosed" when the answer to "reject_reason" is "unclosed"
    And label "accept" otherwise
    And keep the state key "task_id"
