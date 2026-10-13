@decision
Feature: data_contract_schema_guard
  Guard for LLM-proposed data-contract field migrations as a single triage node.
  The model proposes only one typed contract proposal (a list of field proposals:
  field, source_field, semantic, migration_intent); this workflow owns the
  authoritative target schema, the source-mapping authority, the type / historic
  / key / PII compatibility tables, the backfill coverage and the deterministic
  default backfill, and returns the accept / revise / reject decision itself. It
  never runs SQL or code. Port of
  code/qwen_vm/app_scenarios/data_contract_schema_guard.

  The engine's own verbs: ACCEPT / REVISE / REJECT — every field proposal is
  accepted, some need a deterministic backfill revision, or at least one violates
  the contract.

  Folding boundaries (documented, not computed in the DSL):
  - Proposal parsing (the exact typed contract envelope, the duplicate-key
    rejection and the per-field shape) folds into the `proposal` choice.
  - The target schema, the authorized source mappings, the type / historic / key /
    PII compatibility tables and the default-backfill set are fixed fixtures; the
    harness resolves them and hands the DSL one answer per check.
  - The per-field ordered reject checks are folded into the `reject_reason`
    choice (the engine's own first-appended reason, taken across the whole
    proposal); the two deterministic default-backfill revisions are folded into
    the `revision` choice. The ladder mirrors the engine's
    REJECT-before-REVISE-before-ACCEPT aggregate order.

  Emitted labels: reject_duplicate_field, reject_unknown_field,
  reject_source_mapping_unauthorized, reject_semantic_conflict,
  reject_migration_intent_not_allowed, reject_type_incompatible,
  reject_historical_values_invalid, reject_primary_key_not_unique,
  reject_foreign_key_violations, reject_pii_reclassification_not_allowed,
  reject_required_without_default, reject_incomplete_backfill_no_default,
  reject_optional_without_default, reject_non_nullable_incomplete_backfill,
  revise_backfill_default_required, revise_default_replaces_nullable, accept.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one typed contract proposal whose field proposals carry the required keys with valid enums?"
      | answer    | meaning                                                       |
      | accepted  | exactly one structurally valid typed contract proposal       |
      | malformed | parse error, duplicate key or an invalid field-proposal shape |
    And the answer comes from the field "ev_proposal"
    And a choice question "reject_reason" asks "Across the whole proposal, which single ordered contract check does the deterministic authority refuse first?"
      | answer                                | meaning                                                   |
      | none                                  | no field proposal is rejected                             |
      | duplicate_field                       | the same field is proposed twice                          |
      | unknown_field                         | the field is not in the authoritative target schema      |
      | source_mapping_unauthorized           | the source mapping is not authorized for the field      |
      | semantic_conflict                     | the proposed semantic conflicts with local authority    |
      | migration_intent_not_allowed          | the migration intent is not allowed for the field       |
      | type_incompatible                     | the source-to-target type change is lossy or incompatible |
      | historical_values_invalid             | historical values fall outside the target enum / range  |
      | primary_key_not_unique                | a primary-key candidate is not unique                   |
      | foreign_key_violations                | the foreign-key mapping has referential violations      |
      | pii_reclassification_not_allowed      | the PII reclassification is not on the semantic allowlist |
      | required_without_default              | a required field without default breaks nullable / incomplete history |
      | incomplete_backfill_no_default        | a required field has incomplete backfill and no default |
      | optional_without_default              | an optional field has no authoritative default          |
      | non_nullable_incomplete_backfill      | a non-nullable conversion has no complete backfill      |
    And the answer comes from the field "ev_reject_reason"
    And a choice question "revision" asks "When no field proposal is rejected, which single deterministic default-backfill revision does the authority record first?"
      | answer                       | meaning                                                        |
      | none                         | no revision: every field proposal is accepted as written       |
      | backfill_default_required    | an authoritative default must backfill the uncovered records   |
      | default_replaces_nullable    | a default must replace the nullable or missing values           |
    And the answer comes from the field "ev_revision"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_duplicate_field" when the answer to "reject_reason" is "duplicate_field"
    And label "reject_unknown_field" when the answer to "reject_reason" is "unknown_field"
    And label "reject_source_mapping_unauthorized" when the answer to "reject_reason" is "source_mapping_unauthorized"
    And label "reject_semantic_conflict" when the answer to "reject_reason" is "semantic_conflict"
    And label "reject_migration_intent_not_allowed" when the answer to "reject_reason" is "migration_intent_not_allowed"
    And label "reject_type_incompatible" when the answer to "reject_reason" is "type_incompatible"
    And label "reject_historical_values_invalid" when the answer to "reject_reason" is "historical_values_invalid"
    And label "reject_primary_key_not_unique" when the answer to "reject_reason" is "primary_key_not_unique"
    And label "reject_foreign_key_violations" when the answer to "reject_reason" is "foreign_key_violations"
    And label "reject_pii_reclassification_not_allowed" when the answer to "reject_reason" is "pii_reclassification_not_allowed"
    And label "reject_required_without_default" when the answer to "reject_reason" is "required_without_default"
    And label "reject_incomplete_backfill_no_default" when the answer to "reject_reason" is "incomplete_backfill_no_default"
    And label "reject_optional_without_default" when the answer to "reject_reason" is "optional_without_default"
    And label "reject_non_nullable_incomplete_backfill" when the answer to "reject_reason" is "non_nullable_incomplete_backfill"
    And label "revise_backfill_default_required" when the answer to "revision" is "backfill_default_required"
    And label "revise_default_replaces_nullable" when the answer to "revision" is "default_replaces_nullable"
    And label "accept" otherwise
    And keep the state key "contract_id"
