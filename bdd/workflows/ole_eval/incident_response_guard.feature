@decision
Feature: incident_response_guard
  Guard for LLM-proposed incident remediation actions as a single triage node.
  The model proposes one typed action (incident_id, incident_semantic,
  action_intent, target_service, reason) and *only* that; this workflow owns the
  incident id/semantic match, the idempotency fingerprint, the runbook allowlist,
  the target validity, the terminal-state check, the approval quorum, the
  action preconditions and the acknowledge/mitigation SLA checks. It never
  executes remediation or touches infrastructure. Port of
  code/qwen_vm/app_scenarios/incident_response_guard.

  This guard is the first port that emits a *three-valued* decision: the
  deterministic engine returns APPROVE / REVISE / REJECT, so the label ladder
  carries a `revise_*` rung between the `reject_*` hard gates and `approve`.
  Every `reject_*` label is a REJECT, every `revise_*` label is a REVISE, and
  `approve` is the only APPROVE.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (exact five-field shape, incident_id grammar
    INC-<digits>, semantic/action enums, bounded identifier, safe reason text)
    fold into the `proposal` choice.
  - The fixed fixtures (RUNBOOK_ALLOWLIST, ACTION_TARGETS, ACTION_RISKS,
    SLA_SECONDS, the dependency graph and the approval-role table) fold into the
    choice questions: the harness evaluates them against the local incident and
    the model proposal, and hands the DSL one answer per question.

  Emitted labels: reject_proposal, reject_incident_mismatch,
  reject_semantic_conflict, reject_replay, reject_runbook, reject_target,
  reject_target_scope, reject_resolved, reject_state, revise_ack_then_page,
  revise_missing_roles, revise_precondition, revise_mitigation_sla, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {incident_id, incident_semantic, action_intent, target_service, reason} object with a bounded identifier, valid enums and safe reason text?"
      | answer    | meaning                                                            |
      | accepted  | exactly one structurally valid typed action                        |
      | malformed | parse error, missing/extra field, bad id, bad enum or unsafe reason |
    And the answer comes from the field "ev_proposal"
    And a choice question "incident_match" asks "Does the proposal incident_id equal the local incident id?"
      | answer   | meaning            |
      | match    | ids are equal      |
      | mismatch | ids differ         |
    And the answer comes from the field "ev_incident_match"
    And a choice question "semantics" asks "Does the proposal incident_semantic equal the authoritative observed semantic?"
      | answer   | meaning                       |
      | match    | semantics agree               |
      | conflict | semantics conflict with record |
    And the answer comes from the field "ev_semantics"
    And a choice question "fingerprint" asks "Is the command fingerprint unused by the local incident?"
      | answer | meaning                          |
      | unused | not in applied_fingerprints      |
      | replay | already applied, idempotent replay |
    And the answer comes from the field "ev_fingerprint"
    And a choice question "runbook" asks "Is the action_intent within the runbook allowlist for this incident's semantic?"
      | answer  | meaning                       |
      | allowed | in the runbook allowlist      |
      | outside | outside the runbook allowlist |
    And the answer comes from the field "ev_runbook"
    And a choice question "target_allowed" asks "Is the target_service a valid target for the proposed action_intent?"
      | answer  | meaning                              |
      | ok      | target is valid for the action       |
      | invalid | target is invalid for the action     |
    And the answer comes from the field "ev_target_allowed"
    And a choice question "target_scope" asks "Does the target_service equal the incident's authoritative affected service?"
      | answer  | meaning                                   |
      | ok      | target is the affected service            |
      | outside | target is outside the affected service    |
    And the answer comes from the field "ev_target_scope"
    And a choice question "resolved" asks "Is the local incident already in the terminal RESOLVED state?"
      | answer | meaning            |
      | no     | not resolved       |
      | yes    | RESOLVED, terminal |
    And the answer comes from the field "ev_resolved"
    And a choice question "state" asks "Is the local incident state a known state?"
      | answer  | meaning                  |
      | known   | state is in the state set |
      | unknown | state is not recognised   |
    And the answer comes from the field "ev_state"
    And a choice question "ack_first" asks "If the incident is OPEN, has the responder been paged first?"
      | answer | meaning                                       |
      | ok     | not OPEN, or the action is page_oncall        |
      | needed | OPEN and the action is not page_oncall        |
    And the answer comes from the field "ev_ack_first"
    And a choice question "roles" asks "Are all approval roles required by the action risk present?"
      | answer   | meaning                        |
      | complete | required roles all approved    |
      | missing  | an approval quorum is missing  |
    And the answer comes from the field "ev_roles"
    And a choice question "precondition" asks "Do the action's preconditions hold (rollback/failover/scale/close)?"
      | answer | meaning                            |
      | ok     | preconditions hold, or none apply  |
      | failed | a precondition failed              |
    And the answer comes from the field "ev_precondition"
    And a choice question "mitigation_sla" asks "Is the mitigation SLA within bounds, or is the action the SLA-exempt page_oncall?"
      | answer | meaning                                        |
      | ok     | within SLA, or the action is page_oncall       |
      | missed | mitigation SLA breached by a non-page action  |
    And the answer comes from the field "ev_mitigation_sla"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_incident_mismatch" when the answer to "incident_match" is "mismatch"
    And label "reject_semantic_conflict" when the answer to "semantics" is "conflict"
    And label "reject_replay" when the answer to "fingerprint" is "replay"
    And label "reject_runbook" when the answer to "runbook" is "outside"
    And label "reject_target" when the answer to "target_allowed" is "invalid"
    And label "reject_target_scope" when the answer to "target_scope" is "outside"
    And label "reject_resolved" when the answer to "resolved" is "yes"
    And label "reject_state" when the answer to "state" is "unknown"
    And label "revise_ack_then_page" when the answer to "ack_first" is "needed"
    And label "revise_missing_roles" when the answer to "roles" is "missing"
    And label "revise_precondition" when the answer to "precondition" is "failed"
    And label "revise_mitigation_sla" when the answer to "mitigation_sla" is "missed"
    And label "approve" otherwise
    And keep the state key "incident_id"
