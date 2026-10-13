@decision
Feature: shift_assignment_guard
  Guard for LLM-proposed shift-assignment adjustments as a single triage node.
  The model proposes only a typed envelope (request_id, plan_id, preferences,
  adjustments); this workflow owns worker skills, availability, hours, rest,
  consecutive-shift limits, role coverage, fairness, the bounded search and the
  final schedule decision. It never changes a real schedule, notifies a worker
  or contacts an HR system. Port of
  code/qwen_vm/app_scenarios/shift_assignment_guard.

  Fourth three-valued port shape with the engine's own verbs: the engine returns
  FEASIBLE / REVISE / INFEASIBLE (and REJECTED for a malformed envelope) —
  the schedule is accepted, locally revised, found infeasible, or refused.

  Folding boundaries (documented, not computed in the DSL):
  - Proposal parsing (balanced-JSON extraction from a fenced reply, duplicate-key
    scan, the exact {request_id, plan_id, preferences[], adjustments[]} shape,
    the identifier grammar, the preference/adjustment key sets, the weight range
    1..5, unique worker/shift pairs, the assign/avoid action whitelist and the
    dangerous-token scan over every string) folds into the `proposal` choice.
  - The plan table (shifts per plan), the worker table (skills, max minutes,
    per-plan unavailability) and the solver bounds (candidate cap, combination
    cap, global minutes, consecutive-shift limit, minimum rest) are a fixed
    fixture; the harness resolves them and hands the DSL one answer per check.
  - The request ledger, the plan lookup, the bounded search and the fairness /
    constraint solve are folded into the `infeasible` and `revision` choices;
    the engine may append several reasons, so each choice carries the engine's
    own first-appended reason and the ladder emits it, mirroring the engine's
    own INFEASIBLE-before-REVISE precedence.

  Emitted labels: rejected_proposal, infeasible_duplicate_request,
  infeasible_unknown_plan, infeasible_search_limit,
  infeasible_no_assignment, revise_unknown_worker, revise_unknown_shift,
  revise_unqualified, revise_unavailable, revise_fairness_adjusted,
  revise_avoid_overridden, feasible.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, plan_id, preferences[], adjustments[]} object with valid identifiers, a whitelisted action and no dangerous tokens?"
      | answer    | meaning                                                                             |
      | accepted  | exactly one structurally valid typed assignment envelope                            |
      | malformed | parse error, bad shape, bad identifier, bad action, duplicate pair or dangerous token |
    And the answer comes from the field "ev_proposal"
    And a choice question "infeasible" asks "Once the envelope and plan are resolved, which single infeasibility reason does the bounded solver report first?"
      | answer               | meaning                                                    |
      | none                 | the search produced at least one feasible assignment        |
      | duplicate_request    | request_id already evaluated: short-circuit                 |
      | unknown_plan         | plan_id is not in the plan table                            |
      | search_limit         | the combination cap was exceeded before a solve             |
      | no_assignment        | no role-covering assignment satisfies the constraints       |
    And the answer comes from the field "ev_infeasible"
    And a choice question "revision" asks "When the schedule is feasible, which single revision does the engine record first for the proposal's own preferences and adjustments?"
      | answer            | meaning                                                              |
      | none              | every preference and adjustment is honoured                          |
      | unknown_worker    | an adjustment names a worker outside the table                       |
      | unknown_shift     | an adjustment names a shift outside the plan                         |
      | unqualified       | an `assign` names a worker without the shift's skill                 |
      | unavailable       | an `assign` names a worker unavailable for the shift by plan         |
      | fairness_adjusted | a valid `assign` was honoured but the schedule was optimised instead |
      | avoid_overridden  | an `avoid` was overridden to cover the shift                         |
    And the answer comes from the field "ev_revision"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "rejected_proposal" when the answer to "proposal" is "malformed"
    And label "infeasible_duplicate_request" when the answer to "infeasible" is "duplicate_request"
    And label "infeasible_unknown_plan" when the answer to "infeasible" is "unknown_plan"
    And label "infeasible_search_limit" when the answer to "infeasible" is "search_limit"
    And label "infeasible_no_assignment" when the answer to "infeasible" is "no_assignment"
    And label "revise_unknown_worker" when the answer to "revision" is "unknown_worker"
    And label "revise_unknown_shift" when the answer to "revision" is "unknown_shift"
    And label "revise_unqualified" when the answer to "revision" is "unqualified"
    And label "revise_unavailable" when the answer to "revision" is "unavailable"
    And label "revise_fairness_adjusted" when the answer to "revision" is "fairness_adjusted"
    And label "revise_avoid_overridden" when the answer to "revision" is "avoid_overridden"
    And label "feasible" otherwise
    And keep the state key "request_id"
