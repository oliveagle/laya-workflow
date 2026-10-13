@decision
Feature: experiment_design_gate
  Guard for LLM-proposed design-of-experiments as a single triage node. The model
  proposes only one typed experiment proposal (factors, levels, budget); this
  workflow owns the authoritative factor domain, the typed-factor validation, the
  full-factorial treatment combinatorics, the power/replicate requirement, the
  Decimal budget and the deterministic two-factor reduction, and returns the
  design decision itself. It never launches an experiment. Port of
  code/qwen_vm/app_scenarios/experiment_design_gate.

  The engine's own verbs: APPROVE / MODIFY / REJECT — the proposal is approved as
  written, reduced to a feasible design, or refused.

  Folding boundaries (documented, not computed in the DSL):
  - Proposal parsing (the exact typed proposal envelope, the duplicate-key
    rejection, the factor-name / level grammar and the at-least-two-levels rule)
    folds into the `proposal` choice.
  - The authority (factor-name domain, per-factor level allow-lists, min / max
    factor counts, max treatments, required power to replicate count, max
    replicates per treatment, the Decimal budget and the max total runs) is a
    fixed fixture; the harness resolves it and hands the DSL one answer per
    check.
  - The treatment count and the replicate requirement are folded into the
    `authority` choice; the budget / run feasibility and the deterministic
    two-factor reduction are folded into the `design` choice. The engine returns
    early per reject, so each choice carries the engine's own first reason and
    the ladder mirrors the engine's
    REJECT(invalid)-before-REJECT(infeasible)-before-APPROVE/MODIFY/REJECT order.

  Emitted labels: reject_invalid_proposal, reject_infeasible_treatments,
  reject_infeasible_replicates, reject_no_feasible_reduction,
  modify_reduced_design, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one typed experiment proposal whose factors pass the authoritative schema (unique names, known domains, legal and unique levels, at least two levels, factor count within bounds)?"
      | answer    | meaning                                                  |
      | accepted  | exactly one structurally valid typed proposal             |
      | malformed | parse error, duplicate key, or a factor schema violation  |
    And the answer comes from the field "ev_proposal"
    And a choice question "authority" asks "Within the authoritative design limits, which single hard limit does the deterministic authority report first?"
      | answer              | meaning                                                  |
      | none                | the full-factorial design is inside both hard limits      |
      | too_many_treatments | the full-factorial treatment count exceeds the maximum    |
      | too_many_replicates | the required replicates exceed the per-treatment maximum  |
    And the answer comes from the field "ev_authority"
    And a choice question "design" asks "When the design is inside the hard limits, how does the deterministic budget and run evaluation resolve it?"
      | answer        | meaning                                                    |
      | within_budget | the full design fits the budget and run limits -> approve   |
      | reduced_fits  | only the two-factor reduction fits budget and run limits    |
      | no_reduction  | neither the full design nor the two-factor reduction fits   |
    And the answer comes from the field "ev_design"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_invalid_proposal" when the answer to "proposal" is "malformed"
    And label "reject_infeasible_treatments" when the answer to "authority" is "too_many_treatments"
    And label "reject_infeasible_replicates" when the answer to "authority" is "too_many_replicates"
    And label "reject_no_feasible_reduction" when the answer to "design" is "no_reduction"
    And label "modify_reduced_design" when the answer to "design" is "reduced_fits"
    And label "approve" otherwise
    And keep the state key "experiment_id"
