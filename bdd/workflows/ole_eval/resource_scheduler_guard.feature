@decision
Feature: resource_scheduler_guard
  Guard for LLM-proposed scheduling preferences as a single triage node. The model
  proposes only one typed preference set (job preferences plus a soft objective);
  this workflow owns the fixed authority (jobs, resources, eligibility, DAG,
  windows, capacity, preemption policy), the bounded exhaustive solver and the
  deterministic override of soft preferences, and returns the approve / modify /
  reject decision itself. It never launches jobs or an external scheduler. Port of
  code/qwen_vm/app_scenarios/resource_scheduler_guard.

  The engine's own verbs: APPROVE / MODIFY / REJECT — the preferences are honoured,
  the solver overrode some of them, or the proposal cannot be scheduled.

  Folding boundaries (documented, not computed in the DSL):
  - Proposal parsing (the exact typed scheduler envelope, the duplicate-key
    rejection and the job / resource / objective grammar) folds into the
    `proposal` choice.
  - The authority (jobs, resources, per-job eligible resources, DAG edges windows,
    capacity, preemption policy, the bounded search limit) is a fixed fixture; the
    harness resolves it and hands the DSL one answer per check.
  - The ordered preference-domain checks, the bounded-search refusal and the
    infeasibility are folded into the `reject_reason` choice (the engine's own
    first-appended reason); the honest override of a feasible schedule's soft
    preferences is folded into the `outcome` choice. The ladder mirrors the
    engine's REJECT-before-(APPROVE/MODIFY) order.

  Emitted labels: reject_unknown_job, reject_unknown_resource,
  reject_ineligible_resource, reject_duplicate_job_preference,
  reject_combinatorial_limit_exceeded, reject_infeasible_schedule,
  modify_preferences_overridden, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one typed scheduler proposal {job_preferences, soft_objective} with valid identifiers and a valid objective?"
      | answer    | meaning                                                  |
      | accepted  | exactly one structurally valid typed proposal             |
      | malformed | parse error, duplicate key or an invalid shape            |
    And the answer comes from the field "ev_proposal"
    And a choice question "reject_reason" asks "Against the fixed authority, which single ordered scheduler check refuses first?"
      | answer                        | meaning                                                   |
      | none                          | no refusal: a feasible schedule exists                    |
      | unknown_job                   | a preference names a job outside the authority           |
      | unknown_resource              | a preference names a resource outside the authority      |
      | ineligible_resource           | the resource is not eligible for that job                 |
      | duplicate_job_preference      | a job is given two preferences                            |
      | combinatorial_limit_exceeded  | the bounded exhaustive search refused unbounded expansion |
      | infeasible_schedule           | no discrete schedule satisfies DAG, capacity, window and preemption limits |
    And the answer comes from the field "ev_reject_reason"
    And a choice question "outcome" asks "When a feasible schedule exists, how does the deterministic solver resolve the soft preferences?"
      | answer                    | meaning                                                        |
      | preferences_honoured      | every typed preference matches the feasible schedule           |
      | preferences_overridden    | the solver found a feasible schedule but moved some jobs      |
    And the answer comes from the field "ev_outcome"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_unknown_job" when the answer to "reject_reason" is "unknown_job"
    And label "reject_unknown_resource" when the answer to "reject_reason" is "unknown_resource"
    And label "reject_ineligible_resource" when the answer to "reject_reason" is "ineligible_resource"
    And label "reject_duplicate_job_preference" when the answer to "reject_reason" is "duplicate_job_preference"
    And label "reject_combinatorial_limit_exceeded" when the answer to "reject_reason" is "combinatorial_limit_exceeded"
    And label "reject_infeasible_schedule" when the answer to "reject_reason" is "infeasible_schedule"
    And label "modify_preferences_overridden" when the answer to "outcome" is "preferences_overridden"
    And label "approve" otherwise
    And keep the state key "request_id"
