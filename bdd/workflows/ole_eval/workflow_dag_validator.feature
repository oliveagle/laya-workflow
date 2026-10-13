@decision
Feature: workflow_dag_validator
  Guard for LLM-proposed workflow DAGs as a single triage node. The model
  proposes one typed DAG (request_id, workflow_name, nodes [{id, type}], edges
  [{from, to, condition}]); this workflow owns the node/edge caps, the
  acyclicity check, the single-start/single-end checks, the blocked node-type
  set and the dangerous-token scan. Port of
  code/qwen_vm/app_scenarios/workflow_dag_validator.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required fields, node id grammar, unique ids, node
    type enum, edge endpoints in the node set, condition enum, dangerous-token
    rejection) fold into the `proposal` choice.
  - The DAG policy (max_nodes, max_edges, blocked_node_types, require_approval)
    is a fixed fixture. The structural graph checks (acyclicity, start/end
    counts) fold into verdict choices.

  Emitted labels: reject_proposal, reject_node_count, reject_edge_count,
  reject_cycle, reject_no_start, reject_multiple_starts, reject_no_end,
  reject_blocked_type, reject_dangerous, escalate_approval, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, workflow_name, nodes [{id, type}], edges [{from, to, condition}]} object with unique valid node ids, endpoint node refs and valid enums?"
      | answer    | meaning                                                             |
      | accepted  | exactly one structurally valid DAG                                  |
      | malformed | parse error, missing field, bad node id, unknown endpoint or bad enum |
    And the answer comes from the field "ev_proposal"
    And a choice question "node_count" asks "Is the node count within the policy max_nodes cap?"
      | answer | meaning                |
      | ok     | nodes <= max_nodes     |
      | over   | nodes > max_nodes      |
    And the answer comes from the field "ev_node_count"
    And a choice question "edge_count" asks "Is the edge count within the policy max_edges cap?"
      | answer | meaning                |
      | ok     | edges <= max_edges     |
      | over   | edges > max_edges      |
    And the answer comes from the field "ev_edge_count"
    And a choice question "acyclic" asks "Is the workflow free of cycles?"
      | answer | meaning                |
      | ok     | no cycle               |
      | cycle  | a cycle is present     |
    And the answer comes from the field "ev_acyclic"
    And a choice question "start_nodes" asks "How many start-type nodes does the workflow have?"
      | answer   | meaning                        |
      | one      | exactly one start node         |
      | none     | no start node                  |
      | multiple | more than one start node       |
    And the answer comes from the field "ev_start_nodes"
    And a choice question "end_nodes" asks "Does the workflow have at least one end-type node?"
      | answer | meaning                |
      | ok     | at least one end node  |
      | none   | no end node            |
    And the answer comes from the field "ev_end_nodes"
    And a choice question "blocked_type" asks "Does the workflow avoid all blocked node types?"
      | answer  | meaning                |
      | ok      | no blocked node type   |
      | blocked | a blocked node type    |
    And the answer comes from the field "ev_blocked_type"
    And a choice question "dangerous" asks "Is every string field free of dangerous tokens?"
      | answer | meaning                         |
      | ok     | no dangerous tokens            |
      | found  | some string field is dangerous  |
    And the answer comes from the field "ev_dangerous"
    And a choice question "approval" asks "Does the DAG policy require manual approval before execution?"
      | answer       | meaning                      |
      | not_required | no approval flag set          |
      | required     | policy require_approval true  |
    And the answer comes from the field "ev_approval"
    When any other answer goes to "STOP"
    Then the primary question is "acyclic"
    And the action reads the question "acyclic"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_node_count" when the answer to "node_count" is "over"
    And label "reject_edge_count" when the answer to "edge_count" is "over"
    And label "reject_cycle" when the answer to "acyclic" is "cycle"
    And label "reject_no_start" when the answer to "start_nodes" is "none"
    And label "reject_multiple_starts" when the answer to "start_nodes" is "multiple"
    And label "reject_no_end" when the answer to "end_nodes" is "none"
    And label "reject_blocked_type" when the answer to "blocked_type" is "blocked"
    And label "reject_dangerous" when the answer to "dangerous" is "found"
    And label "escalate_approval" when the answer to "approval" is "required"
    And label "approve" otherwise
    And keep the state key "request_id"
