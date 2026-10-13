@decision
Feature: cache_invalidation_guard
  Guard for LLM-proposed cache invalidations as a single triage node. The model
  proposes one typed invalidation (request_id, cache_keys [{key, namespace,
  current_ttl}], invalidation_reason, ttl_override); this workflow owns the
  key-count cap, the protected-namespace escalation, the TTL floor and the
  reason requirement. Port of code/qwen_vm/app_scenarios/cache_invalidation_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required fields, non-empty key/namespace, int
    current_ttl >= 0, optional int ttl_override >= 0, dangerous-token
    rejection) fold into the `proposal` choice.
  - The cache policy (max_keys_per_request, protected_namespaces,
    min_ttl_seconds, require_reason) is a fixed fixture; each ordered check
    folds into one verdict choice.

  Emitted labels: reject_proposal, reject_key_count, escalate_namespace,
  reject_ttl, reject_reason, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, cache_keys [{key, namespace, current_ttl}], invalidation_reason} object (non-empty keys, int ttl fields, no dangerous tokens)?"
      | answer    | meaning                                                     |
      | accepted  | exactly one schema-valid invalidation                       |
      | malformed | parse error, missing field, bad key/namespace or dangerous token |
    And the answer comes from the field "ev_proposal"
    And a choice question "key_count" asks "Is the number of cache_keys within the policy max_keys_per_request cap?"
      | answer | meaning                          |
      | ok     | key count <= max_keys_per_request |
      | over   | key count > max_keys_per_request  |
    And the answer comes from the field "ev_key_count"
    And a choice question "namespace" asks "Is every cache_key namespace outside the policy protected_namespaces set?"
      | answer    | meaning                             |
      | ok        | no key touches a protected namespace |
      | protected | a key touches a protected namespace  |
    And the answer comes from the field "ev_namespace"
    And a choice question "ttl" asks "Is ttl_override absent or at least the policy min_ttl_seconds?"
      | answer   | meaning                              |
      | not_set  | ttl_override is absent               |
      | ok       | ttl_override >= min_ttl_seconds      |
      | below    | ttl_override < min_ttl_seconds       |
    And the answer comes from the field "ev_ttl"
    And a choice question "reason" asks "When policy require_reason is set, is invalidation_reason non-empty? (not_required when the policy does not require it)"
      | answer       | meaning                              |
      | not_required | policy does not require a reason      |
      | present      | reason is present                     |
      | missing      | reason required but empty             |
    And the answer comes from the field "ev_reason"
    When any other answer goes to "STOP"
    Then the primary question is "reason"
    And the action reads the question "reason"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_key_count" when the answer to "key_count" is "over"
    And label "escalate_namespace" when the answer to "namespace" is "protected"
    And label "reject_ttl" when the answer to "ttl" is "below"
    And label "reject_reason" when the answer to "reason" is "missing"
    And label "approve" otherwise
    And keep the state key "request_id"
