@decision
Feature: test_case_generator
  Guard for LLM-proposed test cases as a single triage node. The model proposes
  only one typed test case (target, test_intent, inputs, oracle_kind); this
  workflow owns the input boundary / safety validation, the pure fixture oracles,
  the duplicate fingerprint ledger, the deterministic repair catalogue and the
  coverage catalogue, and returns the accept / repair / reject outcome itself. It
  never runs the proposal against real code. Port of
  code/qwen_vm/app_scenarios/test_case_generator.

  The engine's own verbs: ACCEPTED / REPAIRED / REJECTED — the test case is taken
  as written, taken after a local repair, or refused.

  Folding boundaries (documented, not computed in the DSL):
  - Proposal parsing (the exact typed test-case envelope, the balanced-JSON scan,
    the target / intent / oracle-kind enums and the inputs shape) folds into the
    `proposal` choice.
  - The input boundaries, the safety scan, the pure fixture oracles, the repair
    catalogue, the coverage catalogue and the fingerprint ledger are fixed; the
    harness resolves them and hands the DSL one answer per check.
  - The ordered reject chain (bad input boundary / unsafe input, an unrepairable
    duplicate, a repaired duplicate that already exists, no new coverage) is
    folded into the `reject_reason` choice (the engine's own first reason); the
    deterministic repair of the inputs / intent / oracle is folded into the
    `repair` choice. The ladder mirrors the engine's REJECTED-before-REPAIRED-
    before-ACCEPTED order.

  Emitted labels: rejected_proposal, rejected_input_boundary,
  rejected_duplicate_no_safe_repair, rejected_repaired_duplicate_exists,
  rejected_no_new_coverage, repaired, accepted.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one typed test case {target, test_intent, inputs, oracle_kind} (duration_parser / decimal_calculator / order_state; happy_path / boundary / invalid_input / duplicate; value / error)?"
      | answer    | meaning                                                |
      | accepted  | exactly one structurally valid typed test case          |
      | malformed | parse error, bad enum or an invalid inputs shape        |
    And the answer comes from the field "ev_proposal"
    And a choice question "reject_reason" asks "Against the fixed boundaries, oracles and catalogues, which single ordered gate refuses the test case first?"
      | answer                       | meaning                                                       |
      | none                         | no refusal: the test case is taken (after any repair)         |
      | input_boundary_or_safety     | the inputs break a boundary or fail the safety scan            |
      | duplicate_no_safe_repair     | the fingerprint is a duplicate with no safe catalog repair     |
      | repaired_duplicate_exists    | the repaired candidate is itself already present               |
      | no_new_coverage              | the test adds no new coverage tag                             |
    And the answer comes from the field "ev_reject_reason"
    And a choice question "repair" asks "When the gate takes the test case, which deterministic repair does the engine record first?"
      | answer                       | meaning                                                        |
      | none                         | no repair: accepted exactly as written                          |
      | inputs                       | only the inputs were replaced from the catalog                 |
      | inputs_and_intent            | the inputs and the test intent were replaced                    |
      | oracle                       | only the oracle kind was corrected to the local fixture        |
      | inputs_and_oracle            | the inputs and the oracle kind were repaired                    |
      | inputs_intent_and_oracle     | the inputs, the intent and the oracle kind were all repaired    |
    And the answer comes from the field "ev_repair"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "rejected_proposal" when the answer to "proposal" is "malformed"
    And label "rejected_input_boundary" when the answer to "reject_reason" is "input_boundary_or_safety"
    And label "rejected_duplicate_no_safe_repair" when the answer to "reject_reason" is "duplicate_no_safe_repair"
    And label "rejected_repaired_duplicate_exists" when the answer to "reject_reason" is "repaired_duplicate_exists"
    And label "rejected_no_new_coverage" when the answer to "reject_reason" is "no_new_coverage"
    And label "repaired" when the answer to "repair" is "inputs"
    And label "repaired" when the answer to "repair" is "inputs_and_intent"
    And label "repaired" when the answer to "repair" is "oracle"
    And label "repaired" when the answer to "repair" is "inputs_and_oracle"
    And label "repaired" when the answer to "repair" is "inputs_intent_and_oracle"
    And label "accepted" otherwise
    And keep the state key "test_id"
