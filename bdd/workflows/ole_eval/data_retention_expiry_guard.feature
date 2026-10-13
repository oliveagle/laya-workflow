@decision
Feature: data_retention_expiry_guard
  Guard for LLM-proposed data-retention plans as a single triage node. The model
  proposes one typed plan (request_id, data_records [{record_id, category,
  created_at, retention_days}], retention_policy {max_retention_days,
  requires_approval_categories}); this workflow owns the max-retention check and
  the approval-category escalation. It never deletes or exports data. Port of
  code/qwen_vm/app_scenarios/data_retention_expiry_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required keys, request_id grammar, non-empty record
    list, per-record shape, retention-policy shape, dangerous-token scan) fold
    into the `proposal` choice.
  - The per-record arithmetic (expiry = created_at + retention_days, compared to
    the caller's current_date) is a fixed harness computation: it hands the DSL
    one answer per check. Expired records are reported but never block.

  Emitted labels: reject_proposal, reject_over_retention, escalate_approval,
  approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, data_records[{record_id, category, created_at, retention_days}], retention_policy{max_retention_days, requires_approval_categories}} object with a valid request id and a non-empty record list?"
      | answer    | meaning                                                        |
      | accepted  | exactly one structurally valid retention plan                  |
      | malformed | parse error, missing key, bad request id or unsanitised text   |
    And the answer comes from the field "ev_proposal"
    And a choice question "retention" asks "Is every record's retention_days within the policy max_retention_days?"
      | answer  | meaning                              |
      | within  | all records within the max           |
      | over    | at least one record exceeds the max  |
    And the answer comes from the field "ev_retention"
    And a choice question "approval" asks "Does any not-yet-expired record belong to a requires-approval category?"
      | answer   | meaning                                     |
      | ok       | no non-expired approval-category record     |
      | required | a non-expired approval-category record      |
    And the answer comes from the field "ev_approval"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_over_retention" when the answer to "retention" is "over"
    And label "escalate_approval" when the answer to "approval" is "required"
    And label "approve" otherwise
    And keep the state key "request_id"
