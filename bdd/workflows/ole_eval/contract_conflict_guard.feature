@decision
Feature: contract_conflict_guard
  Guard for LLM-proposed contract clauses as a single triage node. The model
  proposes only one typed clause (clause_type, party, role, modality, start,
  end, priority, exception); this workflow owns the fixed contract fixture, the
  local clause id, the interval overlap, the modal and deadline conflicts, the
  priority effects and the exception references, and returns the review
  outcome itself. It never edits a contract. Port of
  code/qwen_vm/app_scenarios/contract_conflict_guard.

  The engine's own verbs: CONFLICT / NO_CONFLICT / REVIEW — the proposal is
  rejected as a conflict, accepted as conflict-free, or sent for human review.

  Folding boundaries (documented, not computed in the DSL):
  - Proposal parsing (the exact typed clause shape, the clause-type / party /
    role / modality / priority / exception enums, the fixed party-role pairs, the
    ISO date grammar and the start<=end window) folds into the `proposal` choice.
  - The contract fixture (existing clauses) is fixed; the harness resolves it
    and hands the DSL one answer per check.
  - The schema, exception reference, interval overlap and the whole
    conflict / priority / exception comparison are folded into the `exception`
    and `conflict` choices; `conflict` carries the engine's own first
    contributing comparison kind, and the ladder mirrors the engine's
    REVIEW-before-CONFLICT-before-NO_CONFLICT order.

  Emitted labels: review_invalid_proposal, review_missing_exception_reference,
  conflict_modal_contradiction, conflict_deadline_conflict, no_conflict.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one typed clause {clause_type, party, role, modality, start, end, priority, exception} with valid enums and a valid date window?"
      | answer    | meaning                                                              |
      | accepted  | exactly one structurally valid typed clause                           |
      | malformed | parse error, bad enum or bad date window                              |
    And the answer comes from the field "ev_proposal"
    And a choice question "exception" asks "When the clause names an exception, does it resolve a conflict against the fixed contract?"
      | answer   | meaning                                                     |
      | resolved | no exception named, or it references a real exception clause |
      | missing  | the named exception has no reference in the contract         |
    And the answer comes from the field "ev_exception"
    And a choice question "conflict" asks "Against the fixed contract, what is the first contributing conflict the deterministic comparison reports?"
      | answer              | meaning                                                      |
      | none                | no same-scope overlapping clause contributes a conflict       |
      | modal_contradiction | a same-scope overlapping clause contradicts this obligation   |
      | deadline_conflict   | a same-scope overlapping clause conflicts on the deadline     |
    And the answer comes from the field "ev_conflict"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "review_invalid_proposal" when the answer to "proposal" is "malformed"
    And label "review_missing_exception_reference" when the answer to "exception" is "missing"
    And label "conflict_modal_contradiction" when the answer to "conflict" is "modal_contradiction"
    And label "conflict_deadline_conflict" when the answer to "conflict" is "deadline_conflict"
    And label "no_conflict" otherwise
    And keep the state key "clause_id"
