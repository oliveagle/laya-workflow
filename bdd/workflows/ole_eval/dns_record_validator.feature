@decision
Feature: dns_record_validator
  Guard for LLM-proposed DNS record changes as a single triage node. The model
  proposes one typed DNS change (request_id + action add/modify/delete + a
  non-empty records list of {type, name, value, ttl?}); this workflow owns schema
  validation, the record-count cap, blocked record types, blocked domains, the
  ttl minimum and the approval escalation. Port of
  code/qwen_vm/app_scenarios/dns_record_validator.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required fields, request_id grammar, valid record type
    enum, non-empty name/value, non-negative ttl, dangerous-token rejection) fold
    into the `proposal` choice.
  - The local policy table (max_records, blocked_types, blocked_domains, min_ttl,
    require_approval) is a fixed fixture; each deterministic check against it
    folds into one verdict choice.

  Emitted labels: reject_proposal, reject_too_many_records, reject_blocked_type,
  reject_blocked_domain, reject_ttl_below_min, escalate_approval, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, action:add|modify|delete, records:[{type, name, value, ttl?}]} object (request_id grammar, valid type enum, non-empty name/value, non-negative integer ttl, no dangerous tokens)?"
      | answer    | meaning                                                                              |
      | accepted  | exactly one schema-valid DNS change                                                  |
      | malformed | parse error, missing field, bad request_id, invalid type, bad ttl or dangerous token |
    And the answer comes from the field "ev_proposal"
    And a choice question "record_count" asks "Is the number of records within the policy max_records cap?"
      | answer   | meaning                     |
      | ok       | record_count <= max_records |
      | too_many | record_count > max_records  |
    And the answer comes from the field "ev_record_count"
    And a choice question "record_type" asks "Is every record type outside the policy blocked_types set?"
      | answer  | meaning                                 |
      | ok      | no record uses a blocked type           |
      | blocked | at least one record uses a blocked type |
    And the answer comes from the field "ev_record_type"
    And a choice question "domain" asks "Is every record name outside the policy blocked_domains set?"
      | answer  | meaning                                   |
      | ok      | no record hits a blocked domain           |
      | blocked | at least one record hits a blocked domain |
    And the answer comes from the field "ev_domain"
    And a choice question "ttl" asks "Is every record ttl at or above the policy min_ttl?"
      | answer    | meaning                    |
      | ok        | every ttl >= min_ttl       |
      | below_min | at least one ttl < min_ttl |
    And the answer comes from the field "ev_ttl"
    And a choice question "approval" asks "Does the local policy require human approval for this DNS change?"
      | answer       | meaning                         |
      | not_required | no approval flag set, proceed   |
      | required     | policy require_approval is true |
    And the answer comes from the field "ev_approval"
    When any other answer goes to "STOP"
    Then the primary question is "approval"
    And the action reads the question "approval"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_too_many_records" when the answer to "record_count" is "too_many"
    And label "reject_blocked_type" when the answer to "record_type" is "blocked"
    And label "reject_blocked_domain" when the answer to "domain" is "blocked"
    And label "reject_ttl_below_min" when the answer to "ttl" is "below_min"
    And label "escalate_approval" when the answer to "approval" is "required"
    And label "approve" otherwise
    And keep the state key "request_id"
