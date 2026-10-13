@decision
Feature: refactor_plan_guard
  Guard for LLM-proposed refactor plans as a single triage node. The model
  proposes one typed plan (request_id, intent {operation, target_module,
  objective}, steps [{step_id, action, source_module, target_module, test_ids,
  requires}], rollback, proposed_risk); this workflow owns the operation-scope
  check, the step DAG, the per-action payload/scope rules, the atomicity /
  checkpoint / rollback rules, the behaviour-equivalence test coverage and the
  risk alignment. It never edits code or runs a test. Port of
  code/qwen_vm/app_scenarios/refactor_plan_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (exact top-level shape, identifier grammars, action /
    strategy / risk enums, nullable module fields) fold into the `proposal`
    choice.
  - The operation registry and every OperationSpec field (target module,
    objective, created/source modules, allowed / forbidden update sources,
    required / optional tests, computed risk) are a fixed fixture: the harness
    resolves them and hands the DSL one answer per check.
  - The eleven per-step structural rules (a checkpoint carrying a refactor
    payload; a create_module payload or module scope; a move_symbols payload or
    scope; an update_references payload, dependency boundary, source or target
    scope; a run_behavior_tests payload; a step whose `requires` omit the
    mandated predecessor action) are one `step_shape` question with one answer
    per rule, so each keeps its own label without eleven separate questions.

  Emitted labels: reject_proposal, reject_duplicate, reject_unknown_operation,
  reject_intent_target, reject_intent_objective, reject_duplicate_step_id,
  reject_dependency_cycle, reject_unknown_dependency, reject_missing_action,
  reject_checkpoint_payload, reject_create_payload, reject_module_scope,
  reject_move_payload, reject_move_scope, reject_update_payload,
  reject_dependency_boundary, reject_update_source, reject_update_target,
  reject_test_payload, reject_step_order, reject_checkpoint_count,
  reject_rollback_strategy, reject_rollback_checkpoint, reject_checkpoint_order,
  reject_behavior_equivalence, modify_optional_tests, modify_risk_upgraded,
  modify_risk_downgraded, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, intent{operation, target_module, objective}, steps[], rollback{strategy, checkpoint_step}, proposed_risk} object with valid identifiers and enums?"
      | answer    | meaning                                                         |
      | accepted  | exactly one structurally valid refactor plan                    |
      | malformed | parse error, missing/extra field, bad identifier or bad enum    |
    And the answer comes from the field "ev_proposal"
    And a choice question "duplicate" asks "Is the request_id unseen in the in-memory plan ledger?"
      | answer    | meaning                      |
      | fresh     | request_id is new            |
      | duplicate | request_id already evaluated |
    And the answer comes from the field "ev_duplicate"
    And a choice question "operation" asks "Is the intent operation present in the operation registry?"
      | answer  | meaning                     |
      | known   | operation resolved          |
      | unknown | no such operation spec       |
    And the answer comes from the field "ev_operation"
    And a choice question "intent_target" asks "Does the intent target module equal the operation's target module?"
      | answer   | meaning                        |
      | in_scope | target matches the operation   |
      | outside  | target is outside the scope    |
    And the answer comes from the field "ev_intent_target"
    And a choice question "intent_objective" asks "Does the intent objective equal the operation's objective?"
      | answer     | meaning                    |
      | allowed    | objective matches          |
      | not_allowed | objective is not allowed  |
    And the answer comes from the field "ev_intent_objective"
    And a choice question "step_ids" asks "Are all step ids unique?"
      | answer   | meaning                |
      | unique   | no repeated step id     |
      | duplicate | a step id repeats      |
    And the answer comes from the field "ev_step_ids"
    And a choice question "step_dag" asks "Does the step dependency graph admit a topological order?"
      | answer  | meaning              |
      | acyclic | a topological order exists |
      | cycle   | the graph has a cycle |
    And the answer comes from the field "ev_step_dag"
    And a choice question "step_deps" asks "Does every `requires` reference name a declared step id?"
      | answer   | meaning                     |
      | resolved | every dependency resolves    |
      | unknown  | some dependency is unknown   |
    And the answer comes from the field "ev_step_deps"
    And a choice question "required_actions" asks "Does the plan include every required action (checkpoint, create_module, move_symbols, update_references, run_behavior_tests)?"
      | answer   | meaning                       |
      | complete | all required actions present   |
      | missing  | at least one required action is absent |
    And the answer comes from the field "ev_required_actions"
    And a choice question "step_shape" asks "Does every step satisfy its action's payload, module-scope and required-predecessor rules?"
      | answer               | meaning                                                              |
      | ok                   | every step matches its action's payload/scope/order rules             |
      | checkpoint_payload   | a checkpoint step carries a refactor payload                          |
      | create_payload       | a create_module step carries a source module or tests                 |
      | module_scope         | a create_module step targets a module outside the operation           |
      | move_payload         | a move_symbols step carries tests                                     |
      | move_scope           | a move_symbols step is outside the source/created module pair         |
      | update_payload       | an update_references step carries tests                               |
      | dependency_boundary  | an update_references step touches a forbidden dependency              |
      | update_source        | an update_references source is outside the allowed sources            |
      | update_target        | an update_references target is not the created module                 |
      | test_payload         | a run_behavior_tests step carries a module payload                    |
      | order                | a step's `requires` omit the mandated predecessor action              |
    And the answer comes from the field "ev_step_shape"
    And a choice question "checkpoint_count" asks "Does the plan contain exactly one checkpoint step?"
      | answer      | meaning                |
      | exactly_one | exactly one checkpoint |
      | other       | zero or more than one  |
    And the answer comes from the field "ev_checkpoint_count"
    And a choice question "rollback_strategy" asks "Is the rollback strategy restore_checkpoint?"
      | answer  | meaning                          |
      | restore | restore_checkpoint               |
      | other   | any other rollback strategy      |
    And the answer comes from the field "ev_rollback_strategy"
    And a choice question "rollback_checkpoint" asks "Does the rollback checkpoint_step name a declared checkpoint step?"
      | answer  | meaning                          |
      | present | the checkpoint names a real step |
      | missing | no such checkpoint step          |
    And the answer comes from the field "ev_rollback_checkpoint"
    And a choice question "checkpoint_order" asks "Is the checkpoint the first step in the topological order?"
      | answer    | meaning                            |
      | first     | checkpoint precedes every mutation |
      | not_first | a mutation precedes the checkpoint |
    And the answer comes from the field "ev_checkpoint_order"
    And a choice question "behavior" asks "Are all required behaviour tests included in the plan?"
      | answer            | meaning                     |
      | established       | every required test is present |
      | not_established   | a required test is missing  |
    And the answer comes from the field "ev_behavior"
    And a choice question "optional_tests" asks "Are any optional behaviour tests suggested but omitted?"
      | answer    | meaning                              |
      | none      | every optional test is present       |
      | suggested | an optional test is omitted          |
    And the answer comes from the field "ev_optional_tests"
    And a choice question "risk" asks "How does the proposed risk compare to the operation's computed risk?"
      | answer     | meaning                              |
      | aligned    | proposed risk equals computed risk   |
      | upgraded   | the plan must be revised up to the computed risk |
      | downgraded | the plan claims a higher risk than computed |
    And the answer comes from the field "ev_risk"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_duplicate" when the answer to "duplicate" is "duplicate"
    And label "reject_unknown_operation" when the answer to "operation" is "unknown"
    And label "reject_intent_target" when the answer to "intent_target" is "outside"
    And label "reject_intent_objective" when the answer to "intent_objective" is "not_allowed"
    And label "reject_duplicate_step_id" when the answer to "step_ids" is "duplicate"
    And label "reject_dependency_cycle" when the answer to "step_dag" is "cycle"
    And label "reject_unknown_dependency" when the answer to "step_deps" is "unknown"
    And label "reject_missing_action" when the answer to "required_actions" is "missing"
    And label "reject_checkpoint_payload" when the answer to "step_shape" is "checkpoint_payload"
    And label "reject_create_payload" when the answer to "step_shape" is "create_payload"
    And label "reject_module_scope" when the answer to "step_shape" is "module_scope"
    And label "reject_move_payload" when the answer to "step_shape" is "move_payload"
    And label "reject_move_scope" when the answer to "step_shape" is "move_scope"
    And label "reject_update_payload" when the answer to "step_shape" is "update_payload"
    And label "reject_dependency_boundary" when the answer to "step_shape" is "dependency_boundary"
    And label "reject_update_source" when the answer to "step_shape" is "update_source"
    And label "reject_update_target" when the answer to "step_shape" is "update_target"
    And label "reject_test_payload" when the answer to "step_shape" is "test_payload"
    And label "reject_step_order" when the answer to "step_shape" is "order"
    And label "reject_checkpoint_count" when the answer to "checkpoint_count" is "other"
    And label "reject_rollback_strategy" when the answer to "rollback_strategy" is "other"
    And label "reject_rollback_checkpoint" when the answer to "rollback_checkpoint" is "missing"
    And label "reject_checkpoint_order" when the answer to "checkpoint_order" is "not_first"
    And label "reject_behavior_equivalence" when the answer to "behavior" is "not_established"
    And label "modify_optional_tests" when the answer to "optional_tests" is "suggested"
    And label "modify_risk_upgraded" when the answer to "risk" is "upgraded"
    And label "modify_risk_downgraded" when the answer to "risk" is "downgraded"
    And label "approve" otherwise
    And keep the state key "request_id"
