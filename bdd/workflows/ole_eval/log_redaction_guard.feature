@decision
Feature: log_redaction_guard
  Guard for LLM-proposed log redaction rules as a single triage node. The model
  proposes one typed redaction change (request_id + a non-empty rules list of
  {field_pattern, action, replacement}); this workflow owns schema validation,
  the rule-count cap, blocked actions, policy-required patterns and the approval
  escalation. Port of code/qwen_vm/app_scenarios/log_redaction_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required fields, request_id grammar, valid action enum,
    regex compilation, dangerous-token rejection) fold into the `proposal` choice.
  - The local policy table (max_rules, required_patterns, blocked_actions,
    require_approval) is a fixed fixture; each deterministic check against it
    folds into one verdict choice.

  Emitted labels: reject_proposal, reject_too_many_rules, reject_blocked_action,
  reject_missing_required, escalate_approval, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, rules:[{field_pattern, action, replacement}]} object (request_id grammar, non-empty rules, valid action mask/hash/truncate/drop, compilable regex, no dangerous tokens)?"
      | answer    | meaning                                                                                  |
      | accepted  | exactly one schema-valid redaction change                                                |
      | malformed | parse error, missing field, bad request_id, invalid action, bad regex or dangerous token |
    And the answer comes from the field "ev_proposal"
    And a choice question "rule_count" asks "Is the number of rules within the policy max_rules cap?"
      | answer   | meaning                 |
      | ok       | rule_count <= max_rules |
      | too_many | rule_count > max_rules  |
    And the answer comes from the field "ev_rule_count"
    And a choice question "action" asks "Is every rule action outside the policy blocked_actions set?"
      | answer  | meaning                                 |
      | ok      | no rule uses a blocked action           |
      | blocked | at least one rule uses a blocked action |
    And the answer comes from the field "ev_action"
    And a choice question "required" asks "Do the proposed field_patterns cover every policy-required pattern?"
      | answer  | meaning                                    |
      | ok      | every required pattern is covered          |
      | missing | at least one required pattern is uncovered |
    And the answer comes from the field "ev_required"
    And a choice question "approval" asks "Does the local policy require human approval for this redaction change?"
      | answer       | meaning                         |
      | not_required | no approval flag set, proceed   |
      | required     | policy require_approval is true |
    And the answer comes from the field "ev_approval"
    When any other answer goes to "STOP"
    Then the primary question is "approval"
    And the action reads the question "approval"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_too_many_rules" when the answer to "rule_count" is "too_many"
    And label "reject_blocked_action" when the answer to "action" is "blocked"
    And label "reject_missing_required" when the answer to "required" is "missing"
    And label "escalate_approval" when the answer to "approval" is "required"
    And label "approve" otherwise
    And keep the state key "request_id"
