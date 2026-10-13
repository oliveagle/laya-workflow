@decision
Feature: self_healing_gate
  Deterministic, fixture-only self-healing decision gate as a single triage
  node. The model proposes exactly one typed diagnosis (fault_class, evidence,
  candidate_remediation); this workflow owns the fault classification, evidence
  verification, action permission, priority, cooldown, rate limit, blast radius,
  rollback and terminal-state handling. Port of
  code/qwen_vm/app_scenarios/self_healing_gate.
  The deterministic classifier is a fixed evidence-priority chain (crash_loop
  beats deployment_changed+error_rate_high beats dependency_timeout beats
  traffic_above_capacity beats zone_unhealthy beats memory_growth beats
  unknown). A malformed / injected / unknown-action proposal folds into one
  schema choice and is rejected before anything else. The service state machine
  (DEGRADED -> STABILIZING / RECOVERED / ROLLED_BACK / TERMINAL) folds into one
  service_state choice; the terminal set {recovered, rolled_back, terminal} is
  denied re-entry. The per-service+action cooldown and the per-window rate limit
  are clock/history state and fold into one stream choice. The in-memory effect
  (action_success / recovered / rollback_success) folds into one effect choice.
  Emitted labels: reject_diagnosis, deny_terminal, escalate_human,
  defer_cooldown, defer_rate_limit, execute_stabilizing, execute_recovered,
  execute_rolled_back, execute_terminal.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: gate
    Given a choice question "proposal_schema" asks "Is the model's proposal one schema-valid typed diagnosis (exactly fault_class / evidence / candidate_remediation, all enum values, non-empty known evidence and one known candidate remediation)?"
      | answer   | meaning                                                            |
      | accepted | exactly one valid typed diagnosis proposal                          |
      | rejected | malformed, injected/unknown field, unknown enum, or unknown action |
    And the answer comes from the field "ev_proposal_schema"
    And a choice question "service_state" asks "Lifecycle state of the target service before this incident."
      | answer      | meaning                              |
      | degraded    | degraded but still actionable        |
      | stabilizing | a prior action is still stabilising  |
      | recovered   | already recovered (terminal)         |
      | rolled_back | already rolled back (terminal)       |
      | terminal    | unrecoverable, terminal              |
    And the answer comes from the field "ev_service_state"
    And a choice question "evidence" asks "Is every proposed evidence key non-empty and actually present in the incident fixture (no invented key)?"
      | answer | meaning                                             |
      | pass   | all proposed evidence is fixture-backed and non-empty |
      | fail   | empty evidence or at least one invented key          |
    And the answer comes from the field "ev_evidence"
    And a choice question "fault" asks "Does the proposed fault_class match the deterministic classifier's fault class for this fixture?"
      | answer | meaning                              |
      | pass   | matches the deterministic class      |
      | fail   | mismatch, requires human escalation  |
    And the answer comes from the field "ev_fault"
    And a choice question "permission" asks "Is the deterministic fault class allowed by the candidate action's policy (restart->process_crash, drain_bad_instance->memory_leak, rollback_deployment->deployment_regression, failover->dependency_latency or zone_isolation, scale_out->traffic_overload)?"
      | answer | meaning                                  |
      | pass   | the action is permitted for this fault   |
      | fail   | the action is not allowed for this fault |
    And the answer comes from the field "ev_permission"
    And a choice question "stream" asks "Per service+action history: is the action clear of the cooldown (ticks since the last action >= cooldown) and under the rate limit (window count < limit)?"
      | answer       | meaning                                             |
      | idle         | outside cooldown and under the rate limit           |
      | cooldown     | an identical action ran within the cooldown window  |
      | rate_limited | the rate-limit window already holds the max count   |
    And the answer comes from the field "ev_stream"
    And a choice question "blast" asks "Blast radius: is this a protected service about to receive a service-wide (non single-instance) action?"
      | answer   | meaning                                        |
      | pass     | single-instance action, or unprotected service |
      | escalate | service-wide action on a protected service     |
    And the answer comes from the field "ev_blast"
    And a choice question "effect" asks "In-memory effect of executing the action (fixture only)."
      | answer      | meaning                                                 |
      | stabilized  | action_success, not recovered -> STABILIZING             |
      | recovered   | action_success and recovered -> RECOVERED (terminal)     |
      | rolled_back | action failed, rollback succeeded -> ROLLED_BACK (terminal) |
      | terminal    | action failed, rollback failed -> TERMINAL               |
    And the answer comes from the field "ev_effect"
    When any other answer goes to "STOP"
    Then the primary question is "effect"
    And label "reject_diagnosis" when the answer to "proposal_schema" is "rejected"
    And label "deny_terminal" when the answer to "service_state" is one of "recovered", "rolled_back", "terminal"
    And label "escalate_human" when the answer to "evidence" is "fail"
    And label "escalate_human" when the answer to "fault" is "fail"
    And label "escalate_human" when the answer to "permission" is "fail"
    And label "defer_cooldown" when the answer to "stream" is "cooldown"
    And label "defer_rate_limit" when the answer to "stream" is "rate_limited"
    And label "escalate_human" when the answer to "blast" is "escalate"
    And label "execute_rolled_back" when the answer to "effect" is "rolled_back"
    And label "execute_terminal" when the answer to "effect" is "terminal"
    And label "execute_recovered" when the answer to "effect" is "recovered"
    And label "execute_stabilizing" otherwise
    And keep the state key "service"
    And keep the state key "incident_id"
