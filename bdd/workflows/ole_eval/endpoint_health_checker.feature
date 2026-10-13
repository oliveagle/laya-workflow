@decision
Feature: endpoint_health_checker
  Guard for LLM-proposed endpoint health-check config as a single triage node.
  The model proposes one typed health-check config (request_id + a non-empty
  endpoints list of {url, method, timeout_seconds, expected_status,
  interval_seconds}); this workflow owns schema validation, the endpoint-count cap,
  the HTTPS requirement, the timeout ceiling, the interval floor, blocked domains
  and the approval escalation. Port of
  code/qwen_vm/app_scenarios/endpoint_health_checker.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required fields, request_id grammar, valid method enum,
    positive timeout, expected_status 100-599, non-negative interval,
    dangerous-token rejection) fold into the `proposal` choice.
  - The local policy table (max_endpoints, max_timeout_seconds, min_interval_seconds,
    blocked_domains, require_https, require_approval) is a fixed fixture; each
    deterministic check folds into one verdict choice.

  Emitted labels: reject_proposal, reject_too_many_endpoints, reject_insecure_url,
  reject_timeout, reject_interval_below_min, reject_blocked_domain,
  escalate_approval, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, endpoints:[{url, method, timeout_seconds, expected_status, interval_seconds}]} object (request_id grammar, valid method, positive timeout, expected_status 100-599, non-negative interval, no dangerous tokens)?"
      | answer    | meaning                                                                          |
      | accepted  | exactly one schema-valid health-check config                                     |
      | malformed | parse error, missing field, bad request_id, bad method/status or dangerous token |
    And the answer comes from the field "ev_proposal"
    And a choice question "endpoint_count" asks "Is the number of endpoints within the policy max_endpoints cap?"
      | answer   | meaning                         |
      | ok       | endpoint_count <= max_endpoints |
      | too_many | endpoint_count > max_endpoints  |
    And the answer comes from the field "ev_endpoint_count"
    And a choice question "scheme" asks "Does every endpoint url use HTTPS (when the policy requires HTTPS)?"
      | answer   | meaning                        |
      | ok       | every url starts with https:// |
      | insecure | at least one url is not HTTPS  |
    And the answer comes from the field "ev_scheme"
    And a choice question "timeout" asks "Is every endpoint timeout_seconds within the policy max_timeout_seconds?"
      | answer | meaning                                    |
      | ok     | every timeout <= max_timeout_seconds       |
      | over   | at least one timeout > max_timeout_seconds |
    And the answer comes from the field "ev_timeout"
    And a choice question "interval" asks "Is every endpoint interval_seconds at or above the policy min_interval_seconds?"
      | answer    | meaning                                      |
      | ok        | every interval >= min_interval_seconds       |
      | below_min | at least one interval < min_interval_seconds |
    And the answer comes from the field "ev_interval"
    And a choice question "domain" asks "Is every endpoint url outside the policy blocked_domains set?"
      | answer  | meaning                                |
      | ok      | no url hits a blocked domain           |
      | blocked | at least one url hits a blocked domain |
    And the answer comes from the field "ev_domain"
    And a choice question "approval" asks "Does the local policy require human approval for this health-check config?"
      | answer       | meaning                         |
      | not_required | no approval flag set, proceed   |
      | required     | policy require_approval is true |
    And the answer comes from the field "ev_approval"
    When any other answer goes to "STOP"
    Then the primary question is "approval"
    And the action reads the question "approval"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_too_many_endpoints" when the answer to "endpoint_count" is "too_many"
    And label "reject_insecure_url" when the answer to "scheme" is "insecure"
    And label "reject_timeout" when the answer to "timeout" is "over"
    And label "reject_interval_below_min" when the answer to "interval" is "below_min"
    And label "reject_blocked_domain" when the answer to "domain" is "blocked"
    And label "escalate_approval" when the answer to "approval" is "required"
    And label "approve" otherwise
    And keep the state key "request_id"
