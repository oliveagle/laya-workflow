@decision
Feature: crawler_schema_guard
  Guard for LLM-proposed crawler field-mappings as a single triage node. The model
  proposes only one typed extraction proposal (request_id, schema_name, page_id,
  field_mappings); this workflow owns the fixed extraction schema, the page
  fixtures, the URL policy, the selector membership, the required-field and
  normalizer rules, the value validation, the cardinality, the PII transform and
  the request ledger, and returns the accept / repair / reject decision itself. It
  never fetches or extracts from a page. Port of
  code/qwen_vm/app_scenarios/crawler_schema_guard.

  The engine's own verbs: ACCEPT / REPAIR / REJECT — the mapping is accepted as
  written, repaired locally, or refused.

  Folding boundaries (documented, not computed in the DSL):
  - Proposal parsing (the exact typed proposal envelope, the JSON balanced-object
    scan, the duplicate-key rejection and the field-mapping shape) folds into the
    `proposal` choice.
  - The extraction schema, the page fixtures, the URL allow / forbid policy and
    the request ledger are fixed fixtures; the harness resolves them and hands
    the DSL one answer per check.
  - The whole ordered reject chain (duplicate request id, unknown schema, unknown
    page fixture, URL policy, selector-outside-schema, required-missing,
    normalizer-not-allowed, value validation, cardinality) is folded into the
    `reject_reason` choice, which carries the engine's own first-appended reason;
    the local repairs are folded into the `repair` choice. The ladder mirrors the
    engine's REJECT-before-REPAIR-before-ACCEPT order.

  Emitted labels: reject_duplicate_request_id, reject_unknown_schema,
  reject_unknown_page_fixture, reject_url_not_allowed,
  reject_selector_outside_schema, reject_required_missing,
  reject_normalizer_not_allowed, reject_value_invalid,
  reject_cardinality_conflict, repair_duplicate_collapse,
  repair_normalization, repair_fallback_selector, repair_pii_applied, accept.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one typed extraction proposal {request_id, schema_name, page_id, field_mappings} with valid mapping shapes?"
      | answer    | meaning                                                       |
      | accepted  | exactly one structurally valid typed proposal                 |
      | malformed | parse error, duplicate key or an invalid mapping shape        |
    And the answer comes from the field "ev_proposal"
    And a choice question "reject_reason" asks "Against the fixed schema and page fixtures, which single ordered check does the deterministic validator refuse first?"
      | answer                     | meaning                                                       |
      | none                       | no rejection: the mapping clears every reject check           |
      | duplicate_request_id       | the request ledger already holds this request_id              |
      | unknown_schema             | the schema_name is not in the authoritative schema table     |
      | unknown_page_fixture       | the page_id has no authoritative page fixture                 |
      | url_not_allowed            | the page URL fails the authoritative allow / forbid policy    |
      | selector_outside_schema    | a mapped selector is not in the field's authoritative set     |
      | required_missing           | a required field has no extracted value                       |
      | normalizer_not_allowed     | a mapped normalizer is not in the field's allowed set         |
      | value_invalid              | a normalized value fails format / range / enum validation     |
      | cardinality_conflict       | the distinct value count is outside the field's min..max      |
    And the answer comes from the field "ev_reject_reason"
    And a choice question "repair" asks "When the mapping clears every reject check, which single local repair does the deterministic engine record first?"
      | answer                  | meaning                                                        |
      | none                    | no repair needed                                               |
      | duplicate_collapse      | repeated identical raw values collapse to one                  |
      | normalization_repair    | a normalizer changed the value                                 |
      | fallback_selector_repair | a fallback selector was used instead of the mapped one        |
      | pii_applied             | a PII action was applied to a present field                    |
    And the answer comes from the field "ev_repair"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_duplicate_request_id" when the answer to "reject_reason" is "duplicate_request_id"
    And label "reject_unknown_schema" when the answer to "reject_reason" is "unknown_schema"
    And label "reject_unknown_page_fixture" when the answer to "reject_reason" is "unknown_page_fixture"
    And label "reject_url_not_allowed" when the answer to "reject_reason" is "url_not_allowed"
    And label "reject_selector_outside_schema" when the answer to "reject_reason" is "selector_outside_schema"
    And label "reject_required_missing" when the answer to "reject_reason" is "required_missing"
    And label "reject_normalizer_not_allowed" when the answer to "reject_reason" is "normalizer_not_allowed"
    And label "reject_value_invalid" when the answer to "reject_reason" is "value_invalid"
    And label "reject_cardinality_conflict" when the answer to "reject_reason" is "cardinality_conflict"
    And label "repair_duplicate_collapse" when the answer to "repair" is "duplicate_collapse"
    And label "repair_normalization" when the answer to "repair" is "normalization_repair"
    And label "repair_fallback_selector" when the answer to "repair" is "fallback_selector_repair"
    And label "repair_pii_applied" when the answer to "repair" is "pii_applied"
    And label "accept" otherwise
    And keep the state key "request_id"
