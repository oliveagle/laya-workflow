@decision
Feature: keyword_order_guard
  Guard for LLM-proposed Python snippets as a single triage node: the model
  returns one {"candidate_python": "..."} object, the workflow tokenizes it,
  enforces selected keyword and clause order with a deterministic block DFA and
  finishes with ast.parse, then returns the decision itself. Port of
  code/qwen_vm/app_scenarios/keyword_order_guard.

  The engine's own verb: ok / not ok — the candidate's selected keyword order and
  Python syntax are valid, or the first violated rule is reported.

  Folding boundaries (documented, not computed in the DSL):
  - The {candidate_python: ...} envelope (JSON-object extraction, the string and
    non-empty checks) folds into the `proposal` choice.
  - The DFA (if/elif/else phases, try/except/else/finally phases, and the
    from-import pair rule) and the final ast.parse check are the authority; the
    harness runs them and hands the DSL one answer per check. The engine reports
    the first violated rule in source order; the port carries that rule class.

  Emitted labels: reject_proposal, reject_clause_order, reject_import_order,
  reject_not_tokenizable, reject_python_syntax, accept.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Did the model response parse as exactly one {candidate_python: string} object with a non-empty snippet?"
      | answer    | meaning                                                       |
      | ok        | exactly one candidate envelope with a non-empty snippet        |
      | malformed | no JSON object, or candidate_python missing, not a string, or empty |
    And the answer comes from the field "ev_proposal"
    And a choice question "reject_reason" asks "Which rule class does the keyword-order guard report first for this candidate?"
      | answer           | meaning                                                       |
      | none             | selected keyword order and Python syntax are valid             |
      | clause_order     | a clause (elif/else/except/finally) violates its owning block order |
      | import_order     | a from/import statement violates the from ... import ... order |
      | not_tokenizable  | the CPython tokenizer could not scan the candidate              |
      | python_syntax    | ast.parse raised a SyntaxError                                  |
    And the answer comes from the field "ev_reject_reason"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_clause_order" when the answer to "reject_reason" is "clause_order"
    And label "reject_import_order" when the answer to "reject_reason" is "import_order"
    And label "reject_not_tokenizable" when the answer to "reject_reason" is "not_tokenizable"
    And label "reject_python_syntax" when the answer to "reject_reason" is "python_syntax"
    And label "accept" otherwise
    And keep the state key "task_id"
