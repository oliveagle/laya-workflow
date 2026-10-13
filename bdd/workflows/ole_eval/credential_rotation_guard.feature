@decision
Feature: credential_rotation_guard
  Guard for LLM-proposed credential rotation plans as a single triage node. The
  model proposes one typed plan (request_id, credentials
  [{credential_id, service_name, days_since_last_rotation, max_rotation_days}],
  rotation_plan {method, approval_required}, schedule_window); this workflow
  owns the overdue escalation, the critical-approval rule and the service
  blocklist. Port of code/qwen_vm/app_scenarios/credential_rotation_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (request_id grammar, non-empty credentials list,
    non-negative int counters, method/window enums, dangerous-token rejection)
    fold into the `proposal` choice.
  - The rotation policy (blocked_services, require_approval_for_critical,
    max_rotation_days_critical / _default) is a fixed fixture. The effective
    max per credential is min(credential max, policy max); the overdue scan and
    the critical-approval rule each fold into one verdict choice.

  Emitted labels: reject_proposal, escalate_overdue,
  reject_critical_approval, reject_blocked_service, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, credentials [{credential_id, service_name, days_since_last_rotation, max_rotation_days}], rotation_plan {method, approval_required}, schedule_window} object with valid enums and no dangerous tokens?"
      | answer    | meaning                                                                    |
      | accepted  | exactly one schema-valid rotation plan                                     |
      | malformed | parse error, missing field, bad request_id, bad enum or dangerous token    |
    And the answer comes from the field "ev_proposal"
    And a choice question "overdue" asks "Is any credential past its effective rotation max (min of its own max_rotation_days and the policy max for its service)?"
      | answer | meaning                          |
      | none   | every credential is within max   |
      | some   | at least one credential overdue  |
    And the answer comes from the field "ev_overdue"
    And a choice question "critical_approval" asks "For a critical (blocklisted) service, is the require_approval_for_critical rule satisfied? (not_applicable when the policy does not require approval or no critical service is used)"
      | answer       | meaning                                                     |
      | not_applicable | policy does not require approval or no critical service    |
      | satisfied    | plan sets approval_required true for a critical service     |
      | missing      | critical service used but plan approval_required is false   |
    And the answer comes from the field "ev_critical_approval"
    And a choice question "blocked_service" asks "Is any credential service_name present in the policy blocklist?"
      | answer  | meaning                       |
      | none    | no service is blocklisted      |
      | blocked | at least one service blocked   |
    And the answer comes from the field "ev_blocked_service"
    When any other answer goes to "STOP"
    Then the primary question is "overdue"
    And the action reads the question "overdue"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "escalate_overdue" when the answer to "overdue" is "some"
    And label "reject_critical_approval" when the answer to "critical_approval" is "missing"
    And label "reject_blocked_service" when the answer to "blocked_service" is "blocked"
    And label "approve" otherwise
    And keep the state key "request_id"
