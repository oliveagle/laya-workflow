@decision
Feature: knowledge_graph_guard
  Guard for LLM-proposed knowledge-graph operations as a single triage node. The
  model proposes only one typed graph operation (add_entity / add_relation /
  merge_entities / retire_entity); this workflow owns the graph state, the entity
  ids, the unique keys, the referential integrity, the cycles, the versions, the
  transactions, the local merge / retire repairs and the invariant rollback, and
  returns the accept / revise / reject decision itself. It never mutates a live
  graph. Port of code/qwen_vm/app_scenarios/knowledge_graph_guard.

  The engine's own verbs: ACCEPT / REVISE / REJECT — the operation is applied,
  applied after a local revision, or refused with a rollback.

  Folding boundaries (documented, not computed in the DSL):
  - Proposal parsing (the exact typed proposal envelope, the operation enum, the
    per-operation field sets, the id / key grammar and the safe-text scan) folds
    into the `proposal` choice.
  - The graph state and the DAG relation types are fixed; the harness resolves
    them and hands the DSL one answer per check.
  - The per-operation early refusals and the post-apply invariant rollback are
    folded into the `reject_reason` choice, which carries the engine's own first
    reason; the local merge / retire revisions are folded into the `revision`
    choice. The ladder mirrors the engine's
    REJECT(op)-before-REJECT(invariant-rollback)-before-REVISE-before-ACCEPT
    order.

  Emitted labels: reject_invalid_proposal, reject_duplicate_entity_id,
  reject_unique_key_conflict, reject_referential_integrity_subject,
  reject_referential_integrity_object, reject_self_relation_conflict,
  reject_duplicate_relation_triple, reject_merge_source_missing,
  reject_merge_target_missing, reject_merge_same_entity,
  reject_retire_entity_missing, reject_invariant_subject_missing,
  reject_invariant_object_missing, reject_invariant_duplicate_triple,
  reject_invariant_self_relation, reject_invariant_duplicate_unique_key,
  reject_invariant_cycle, revise_merge_direction_normalized,
  revise_duplicate_relation_retired, revise_dependent_relations_retired, accept.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one typed graph operation (add_entity / add_relation / merge_entities / retire_entity) with the operation's exact fields and a valid safe-text grammar?"
      | answer    | meaning                                                         |
      | accepted  | exactly one structurally valid typed operation                   |
      | malformed | parse error, unknown operation, bad field set or unsafe text     |
    And the answer comes from the field "ev_proposal"
    And a choice question "reject_reason" asks "Against the fixed graph state, which single deterministic refusal does the validator report first (a per-operation check, or a post-apply invariant that triggers a rollback)?"
      | answer                            | meaning                                                    |
      | none                              | no refusal: the operation commits                            |
      | duplicate_entity_id               | the entity id already exists                                |
      | unique_key_conflict               | an active entity shares the type and canonical name         |
      | referential_integrity_subject     | the relation subject is missing or inactive                 |
      | referential_integrity_object      | the relation object is missing or inactive                  |
      | self_relation_conflict            | the relation subject equals its object                      |
      | duplicate_relation_triple         | an identical active triple already exists                   |
      | merge_source_missing              | the merge source is missing or inactive                     |
      | merge_target_missing              | the merge target is missing or inactive                     |
      | merge_same_entity                 | the merge source equals its target                          |
      | retire_entity_missing             | the retire target is missing or inactive                    |
      | invariant_subject_missing         | a post-apply invariant finds an active relation without a live subject |
      | invariant_object_missing          | a post-apply invariant finds an active relation without a live object  |
      | invariant_duplicate_triple        | a post-apply invariant finds a duplicate active triple      |
      | invariant_self_relation           | a post-apply invariant finds an active self relation        |
      | invariant_duplicate_unique_key    | a post-apply invariant finds a duplicate active unique key  |
      | invariant_cycle                   | a post-apply invariant finds a cycle in a DAG relation type |
    And the answer comes from the field "ev_reject_reason"
    And a choice question "revision" asks "When no refusal fires, which single local revision does the deterministic engine record first?"
      | answer                       | meaning                                                    |
      | none                         | no revision: the operation commits as written               |
      | merge_direction_normalized   | the merge direction is swapped so the higher-degree node survives |
      | duplicate_relation_retired   | a relation that became a duplicate is retired after the merge |
      | dependent_relations_retired  | a retired entity's dependent relations are retired in the same transaction |
    And the answer comes from the field "ev_revision"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_invalid_proposal" when the answer to "proposal" is "malformed"
    And label "reject_duplicate_entity_id" when the answer to "reject_reason" is "duplicate_entity_id"
    And label "reject_unique_key_conflict" when the answer to "reject_reason" is "unique_key_conflict"
    And label "reject_referential_integrity_subject" when the answer to "reject_reason" is "referential_integrity_subject"
    And label "reject_referential_integrity_object" when the answer to "reject_reason" is "referential_integrity_object"
    And label "reject_self_relation_conflict" when the answer to "reject_reason" is "self_relation_conflict"
    And label "reject_duplicate_relation_triple" when the answer to "reject_reason" is "duplicate_relation_triple"
    And label "reject_merge_source_missing" when the answer to "reject_reason" is "merge_source_missing"
    And label "reject_merge_target_missing" when the answer to "reject_reason" is "merge_target_missing"
    And label "reject_merge_same_entity" when the answer to "reject_reason" is "merge_same_entity"
    And label "reject_retire_entity_missing" when the answer to "reject_reason" is "retire_entity_missing"
    And label "reject_invariant_subject_missing" when the answer to "reject_reason" is "invariant_subject_missing"
    And label "reject_invariant_object_missing" when the answer to "reject_reason" is "invariant_object_missing"
    And label "reject_invariant_duplicate_triple" when the answer to "reject_reason" is "invariant_duplicate_triple"
    And label "reject_invariant_self_relation" when the answer to "reject_reason" is "invariant_self_relation"
    And label "reject_invariant_duplicate_unique_key" when the answer to "reject_reason" is "invariant_duplicate_unique_key"
    And label "reject_invariant_cycle" when the answer to "reject_reason" is "invariant_cycle"
    And label "revise_merge_direction_normalized" when the answer to "revision" is "merge_direction_normalized"
    And label "revise_duplicate_relation_retired" when the answer to "revision" is "duplicate_relation_retired"
    And label "revise_dependent_relations_retired" when the answer to "revision" is "dependent_relations_retired"
    And label "accept" otherwise
    And keep the state key "proposal_id"
