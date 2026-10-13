@decision
Feature: cache_policy_enforcer
  Guard for LLM-proposed HTTP cache policy changes as a single triage node. The
  model proposes one typed cache-policy change (request_id + a non-empty routes
  list of {path_pattern, cache_control, max_age}); this workflow owns schema
  validation, the route-count cap, blocked path patterns, the max_age ceiling and
  the approval escalation. Port of code/qwen_vm/app_scenarios/cache_policy_enforcer.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required fields, request_id grammar, per-route field
    types, dangerous-token rejection, max_age non-negative integer) fold into the
    single `proposal` choice.
  - The local policy table (max_routes, max_age_limit, blocked_patterns,
    require_approval) is a fixed fixture, not model input; each deterministic
    check against it folds into one verdict choice (route count, blocked pattern
    prefix, max_age ceiling).

  Emitted labels: reject_proposal, reject_too_many_routes, reject_blocked_pattern,
  reject_max_age, escalate_approval, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, routes:[{path_pattern, cache_control, max_age}]} object (request_id grammar, non-empty routes, per-route non-empty strings, non-negative integer max_age, no dangerous tokens)?"
      | answer    | meaning                                                                    |
      | accepted  | exactly one schema-valid cache-policy change                               |
      | malformed | parse error, missing field, bad request_id, bad max_age or dangerous token |
    And the answer comes from the field "ev_proposal"
    And a choice question "route_count" asks "Is the number of routes within the policy max_routes cap?"
      | answer   | meaning                   |
      | ok       | route_count <= max_routes |
      | too_many | route_count > max_routes  |
    And the answer comes from the field "ev_route_count"
    And a choice question "pattern" asks "Does any route path_pattern start with a policy-blocked prefix?"
      | answer  | meaning                                   |
      | ok      | no route hits a blocked pattern           |
      | blocked | at least one route hits a blocked pattern |
    And the answer comes from the field "ev_pattern"
    And a choice question "max_age" asks "Is every route max_age within the policy max_age_limit ceiling?"
      | answer | meaning                              |
      | ok     | every max_age <= max_age_limit       |
      | over   | at least one max_age > max_age_limit |
    And the answer comes from the field "ev_max_age"
    And a choice question "approval" asks "Does the local policy require human approval for this cache-policy change?"
      | answer       | meaning                         |
      | not_required | no approval flag set, proceed   |
      | required     | policy require_approval is true |
    And the answer comes from the field "ev_approval"
    When any other answer goes to "STOP"
    Then the primary question is "approval"
    And the action reads the question "approval"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_too_many_routes" when the answer to "route_count" is "too_many"
    And label "reject_blocked_pattern" when the answer to "pattern" is "blocked"
    And label "reject_max_age" when the answer to "max_age" is "over"
    And label "escalate_approval" when the answer to "approval" is "required"
    And label "approve" otherwise
    And keep the state key "request_id"
