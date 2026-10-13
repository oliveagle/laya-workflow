@decision
Feature: config_schema_migration_guard
  Guard for LLM-proposed configuration schema migrations as a single triage
  node. The model proposes one typed migration (request_id, schema_name,
  from_version, to_version, changes [{action add/remove/rename/modify,
  field_path, default_value, new_name}], rollback_plan {steps}); this workflow
  owns the change/removal caps, the critical-field rule, the removal-default
  rule and the dangerous-token scan. Port of
  code/qwen_vm/app_scenarios/config_schema_migration_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required fields, non-empty strings, change action
    enum, remove/rename companion fields, non-empty rollback steps,
    dangerous-token rejection) fold into the `proposal` choice.
  - The migration policy (max_changes, critical_fields, require_approval) is a
    fixed fixture; each ordered check folds into one verdict choice.

  Emitted labels: reject_proposal, reject_change_count, reject_removal_count,
  reject_critical_field, reject_removal_default, reject_dangerous,
  escalate_approval, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, schema_name, from_version, to_version, changes [{action, field_path, default_value, new_name}], rollback_plan {steps}} object with valid action enums, companion fields and a non-empty rollback steps list?"
      | answer    | meaning                                                             |
      | accepted  | exactly one schema-valid migration                                  |
      | malformed | parse error, missing field, bad action or empty rollback steps      |
    And the answer comes from the field "ev_proposal"
    And a choice question "change_count" asks "Is the number of changes within the policy max_changes cap?"
      | answer | meaning                     |
      | ok     | change count <= max_changes |
      | over   | change count > max_changes  |
    And the answer comes from the field "ev_change_count"
    And a choice question "removal_count" asks "Are there at most 5 field removals?"
      | answer | meaning              |
      | ok     | removals <= 5        |
      | over   | removals > 5         |
    And the answer comes from the field "ev_removal_count"
    And a choice question "critical_field" asks "Does the proposal avoid removing any policy critical_field?"
      | answer  | meaning                                  |
      | ok      | no critical field is removed             |
      | removal | a critical field is being removed        |
    And the answer comes from the field "ev_critical_field"
    And a choice question "removal_default" asks "Does every removal carry a non-None default_value?"
      | answer       | meaning                                 |
      | ok           | every removal has a default_value        |
      | none_default | a removal has a None default_value       |
    And the answer comes from the field "ev_removal_default"
    And a choice question "dangerous" asks "Is any string field free of dangerous tokens?"
      | answer  | meaning                             |
      | ok      | no dangerous tokens anywhere        |
      | found   | some string field has a dangerous token |
    And the answer comes from the field "ev_dangerous"
    And a choice question "approval" asks "Does the migration policy require manual approval?"
      | answer       | meaning                     |
      | not_required | no approval flag set        |
      | required     | policy require_approval true |
    And the answer comes from the field "ev_approval"
    When any other answer goes to "STOP"
    Then the primary question is "approval"
    And the action reads the question "approval"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_change_count" when the answer to "change_count" is "over"
    And label "reject_removal_count" when the answer to "removal_count" is "over"
    And label "reject_critical_field" when the answer to "critical_field" is "removal"
    And label "reject_removal_default" when the answer to "removal_default" is "none_default"
    And label "reject_dangerous" when the answer to "dangerous" is "found"
    And label "escalate_approval" when the answer to "approval" is "required"
    And label "approve" otherwise
    And keep the state key "request_id"
