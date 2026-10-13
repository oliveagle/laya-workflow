@decision
Feature: session_token_validator
  Guard for LLM-proposed session-token lifecycle changes as a single triage node.
  The model proposes one typed token change (request_id + action + token_type +
  scopes + expiry_seconds + metadata); this workflow owns schema validation, the
  revoke-always-approve path, the expiry ceiling, the scope-count cap, blocked
  scopes, dangerous metadata and the approval escalation. Port of
  code/qwen_vm/app_scenarios/session_token_validator.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required fields, request_id grammar, valid token type
    enum, positive expiry, metadata object, dangerous-token rejection) fold into
    the `proposal` choice.
  - A `revoke` action is approved unconditionally and short-circuits every other
    check; it is the first non-malformed label so it wins the ordered ladder.
  - The local policy table (max_expiry_seconds, max_scopes, blocked_scopes,
    require_approval) is a fixed fixture; each deterministic check against it
    folds into one verdict choice.

  Emitted labels: reject_proposal, approve_revoke, reject_expiry,
  reject_scope_count, reject_blocked_scope, reject_metadata, escalate_approval,
  approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, action, token_type, scopes, expiry_seconds, metadata} object (request_id grammar, valid action/token type, positive expiry, scopes list, metadata object, no dangerous tokens)?"
      | answer    | meaning                                                                                        |
      | accepted  | exactly one schema-valid token change                                                          |
      | malformed | parse error, missing field, bad request_id, invalid action/type, bad expiry or dangerous token |
    And the answer comes from the field "ev_proposal"
    And a choice question "action" asks "Is this a token revocation (which is approved unconditionally) or a create/mint?"
      | answer | meaning                                          |
      | revoke | action == revoke, approve without further checks |
      | create | action creates or mints a token, keep checking   |
    And the answer comes from the field "ev_action"
    And a choice question "expiry" asks "Is expiry_seconds within the policy max_expiry_seconds ceiling?"
      | answer | meaning                              |
      | ok     | expiry_seconds <= max_expiry_seconds |
      | over   | expiry_seconds > max_expiry_seconds  |
    And the answer comes from the field "ev_expiry"
    And a choice question "scope_count" asks "Is the number of scopes within the policy max_scopes cap?"
      | answer   | meaning                   |
      | ok       | scope_count <= max_scopes |
      | too_many | scope_count > max_scopes  |
    And the answer comes from the field "ev_scope_count"
    And a choice question "scope" asks "Is every scope outside the policy blocked_scopes set?"
      | answer  | meaning                       |
      | ok      | no scope is blocked           |
      | blocked | at least one scope is blocked |
    And the answer comes from the field "ev_scope"
    And a choice question "metadata" asks "Is the metadata free of dangerous tokens?"
      | answer | meaning                            |
      | ok     | metadata serializes clean          |
      | unsafe | metadata contains dangerous tokens |
    And the answer comes from the field "ev_metadata"
    And a choice question "approval" asks "Does the local policy require human approval for this token change?"
      | answer       | meaning                         |
      | not_required | no approval flag set, proceed   |
      | required     | policy require_approval is true |
    And the answer comes from the field "ev_approval"
    When any other answer goes to "STOP"
    Then the primary question is "approval"
    And the action reads the question "approval"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "approve_revoke" when the answer to "action" is "revoke"
    And label "reject_expiry" when the answer to "expiry" is "over"
    And label "reject_scope_count" when the answer to "scope_count" is "too_many"
    And label "reject_blocked_scope" when the answer to "scope" is "blocked"
    And label "reject_metadata" when the answer to "metadata" is "unsafe"
    And label "escalate_approval" when the answer to "approval" is "required"
    And label "approve" otherwise
    And keep the state key "request_id"
