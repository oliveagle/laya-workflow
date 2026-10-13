@decision
Feature: webhook_signature_verifier
  Guard for LLM-proposed webhook signature-verification config as a single triage
  node. The model proposes one typed webhook config (request_id, endpoint_url,
  hash_algorithm, encoding, header_name, secret_source, tolerance_seconds); this
  workflow owns schema validation, the HTTPS requirement, the blocked-hash-algorithm
  set, the tolerance ceiling and the approval escalation. Port of
  code/qwen_vm/app_scenarios/webhook_signature_verifier.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required fields, request_id grammar, hash/encoding enums,
    non-empty strings, non-negative tolerance, dangerous-token rejection) fold into
    the `proposal` choice.
  - The local policy table (blocked_algos, max_tolerance, require_https,
    require_approval) is a fixed fixture; each deterministic check folds into one
    verdict choice.

  Emitted labels: reject_proposal, reject_insecure_url, reject_blocked_algo,
  reject_tolerance, escalate_approval, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, endpoint_url, hash_algorithm, encoding, header_name, secret_source, tolerance_seconds} object (request_id grammar, valid hash/encoding enums, non-empty strings, non-negative tolerance, no dangerous tokens)?"
      | answer    | meaning                                                                 |
      | accepted  | exactly one schema-valid webhook config                                 |
      | malformed | parse error, missing field, bad request_id, bad enum or dangerous token |
    And the answer comes from the field "ev_proposal"
    And a choice question "scheme" asks "Does endpoint_url use HTTPS (when the policy requires HTTPS)?"
      | answer | meaning                           |
      | https  | endpoint_url starts with https:// |
      | plain  | plain http:// or another scheme   |
    And the answer comes from the field "ev_scheme"
    And a choice question "algo" asks "Is hash_algorithm outside the policy blocked_algos set?"
      | answer  | meaning                   |
      | ok      | hash_algorithm is allowed |
      | blocked | hash_algorithm is blocked |
    And the answer comes from the field "ev_algo"
    And a choice question "tolerance" asks "Is tolerance_seconds within the policy max_tolerance?"
      | answer | meaning                            |
      | ok     | tolerance_seconds <= max_tolerance |
      | over   | tolerance_seconds > max_tolerance  |
    And the answer comes from the field "ev_tolerance"
    And a choice question "approval" asks "Does the local policy require human approval for this webhook config?"
      | answer       | meaning                         |
      | not_required | no approval flag set, proceed   |
      | required     | policy require_approval is true |
    And the answer comes from the field "ev_approval"
    When any other answer goes to "STOP"
    Then the primary question is "approval"
    And the action reads the question "approval"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_insecure_url" when the answer to "scheme" is "plain"
    And label "reject_blocked_algo" when the answer to "algo" is "blocked"
    And label "reject_tolerance" when the answer to "tolerance" is "over"
    And label "escalate_approval" when the answer to "approval" is "required"
    And label "approve" otherwise
    And keep the state key "request_id"
