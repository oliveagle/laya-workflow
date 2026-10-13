@decision
Feature: network_bandwidth_allocation_guard
  Guard for LLM-proposed bandwidth allocations as a single triage node. The
  model proposes one typed allocation set (request_id, allocations
  [{link_id, bandwidth_mbps, priority}]); this workflow owns the link catalog,
  the per-link capacity check (capacity minus reserved) and the unknown-link
  rejection. Port of code/qwen_vm/app_scenarios/network_bandwidth_allocation_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required keys, request_id grammar, non-empty
    allocations list, positive bandwidth_mbps, priority in high/medium/low,
    dangerous-token rejection) fold into the `proposal` choice.
  - The link catalog (link_id -> capacity_mbps / reserved_mbps) is a fixed
    fixture. The per-link total-vs-available comparison is folded into one
    `capacity` verdict choice over every referenced link.

  Emitted labels: reject_proposal, reject_unknown_link,
  reject_over_capacity, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, allocations [{link_id, bandwidth_mbps, priority}]} object (request_id grammar, non-empty list, positive bandwidth, priority in high/medium/low, no dangerous tokens)?"
      | answer    | meaning                                                                     |
      | accepted  | exactly one schema-valid allocation set                                      |
      | malformed | parse error, missing key, bad request_id, bad bandwidth or bad priority      |
    And the answer comes from the field "ev_proposal"
    And a choice question "link" asks "Is every referenced link_id present in the local link catalog?"
      | answer  | meaning                          |
      | ok      | every link_id is in the catalog  |
      | unknown | at least one link_id is unknown  |
    And the answer comes from the field "ev_link"
    And a choice question "capacity" asks "For every referenced link, is the summed requested bandwidth within its available capacity (capacity_mbps minus reserved_mbps)?"
      | answer | meaning                                          |
      | ok     | every link's total <= its available capacity     |
      | over   | some link's total > its available capacity       |
    And the answer comes from the field "ev_capacity"
    When any other answer goes to "STOP"
    Then the primary question is "capacity"
    And the action reads the question "capacity"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_unknown_link" when the answer to "link" is "unknown"
    And label "reject_over_capacity" when the answer to "capacity" is "over"
    And label "approve" otherwise
    And keep the state key "request_id"
