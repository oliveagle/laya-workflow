@decision
Feature: query_complexity_limiter
  Guard for LLM-proposed database queries as a single triage node. The model
  proposes one typed query (request_id, query_type, tables, joins, subqueries,
  estimated_rows, query_text); this workflow owns schema validation, the blocked
  query-type set, the join/subquery/row-estimate caps, blocked tables, dangerous
  query text and the approval escalation. Port of
  code/qwen_vm/app_scenarios/query_complexity_limiter.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required fields, request_id grammar, query_type enum,
    non-negative integer counters, dangerous-token rejection) fold into the
    `proposal` choice.
  - The local policy table (max_joins, max_subqueries, max_estimated_rows,
    blocked_tables, blocked_query_types, require_approval) is a fixed fixture;
    each deterministic check folds into one verdict choice.

  Emitted labels: reject_proposal, reject_blocked_query_type,
  reject_too_many_joins, reject_too_many_subqueries, reject_row_estimate,
  reject_blocked_table, reject_dangerous_text, escalate_approval, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, query_type, tables, joins, subqueries, estimated_rows, query_text} object (request_id grammar, query_type SELECT/INSERT/UPDATE/DELETE, non-negative integer counters, no dangerous tokens)?"
      | answer    | meaning                                                                       |
      | accepted  | exactly one schema-valid query                                                |
      | malformed | parse error, missing field, bad request_id, bad query_type or dangerous token |
    And the answer comes from the field "ev_proposal"
    And a choice question "query_type" asks "Is query_type outside the policy blocked_query_types set?"
      | answer  | meaning               |
      | ok      | query_type is allowed |
      | blocked | query_type is blocked |
    And the answer comes from the field "ev_query_type"
    And a choice question "joins" asks "Is the join count within the policy max_joins cap?"
      | answer   | meaning            |
      | ok       | joins <= max_joins |
      | too_many | joins > max_joins  |
    And the answer comes from the field "ev_joins"
    And a choice question "subqueries" asks "Is the subquery count within the policy max_subqueries cap?"
      | answer   | meaning                      |
      | ok       | subqueries <= max_subqueries |
      | too_many | subqueries > max_subqueries  |
    And the answer comes from the field "ev_subqueries"
    And a choice question "rows" asks "Is estimated_rows within the policy max_estimated_rows cap?"
      | answer | meaning                              |
      | ok   | estimated_rows <= max_estimated_rows |
      | over | estimated_rows > max_estimated_rows  |
    And the answer comes from the field "ev_rows"
    And a choice question "table" asks "Is every referenced table outside the policy blocked_tables set?"
      | answer  | meaning                       |
      | ok      | no table is blocked           |
      | blocked | at least one table is blocked |
    And the answer comes from the field "ev_table"
    And a choice question "text" asks "Is query_text free of dangerous tokens?"
      | answer    | meaning                              |
      | ok        | query_text serializes clean          |
      | dangerous | query_text contains dangerous tokens |
    And the answer comes from the field "ev_text"
    And a choice question "approval" asks "Does the local policy require human approval for this query?"
      | answer       | meaning                         |
      | not_required | no approval flag set, proceed   |
      | required     | policy require_approval is true |
    And the answer comes from the field "ev_approval"
    When any other answer goes to "STOP"
    Then the primary question is "approval"
    And the action reads the question "approval"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_blocked_query_type" when the answer to "query_type" is "blocked"
    And label "reject_too_many_joins" when the answer to "joins" is "too_many"
    And label "reject_too_many_subqueries" when the answer to "subqueries" is "too_many"
    And label "reject_row_estimate" when the answer to "rows" is "over"
    And label "reject_blocked_table" when the answer to "table" is "blocked"
    And label "reject_dangerous_text" when the answer to "text" is "dangerous"
    And label "escalate_approval" when the answer to "approval" is "required"
    And label "approve" otherwise
    And keep the state key "request_id"
