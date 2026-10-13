@decision
Feature: rpa_action_guard
  Guard for LLM-proposed RPA UI actions as a single triage node. The model
  proposes only one typed action (request_id, intent, action, selector, value,
  value_ref, page_state, attempt); this workflow owns the fixed intent policy
  table, the page / DOM fixtures, the URL policy, the selector membership, the
  target capability, the sensitive-field handling, the value-reference rules, the
  retry ledger and the idempotency replay, and returns the allow / review / deny
  decision itself. It never drives a real browser. Port of
  code/qwen_vm/app_scenarios/rpa_action_guard.

  The engine's own verbs: ALLOW / REVIEW / DENY — the action is permitted, sent
  for human review, or refused.

  Folding boundaries (documented, not computed in the DSL):
  - Proposal parsing (the exact typed action shape, the intent / action / selector
    grammar, the idempotency-key shape and the duplicate-key rejection) folds into
    the `proposal` choice.
  - The intent policy table, the page state fixtures, the DOM elements, the
    sensitive-field flags and the retry ledger are fixed fixtures; the harness
    resolves them and hands the DSL one answer per check.
  - The whole ordered deny chain (action whitelist, unknown intent, unknown page
    fixture, URL policy, page-state mismatch, selector-outside-target-schema,
    target-not-found, intent / action mismatch, target capability, disabled target,
    sensitive-field rules, value-reference rules, typing rules, retry limits) is
    folded into the `deny_reason` choice, which carries the engine's own
    first-appended reason; the policy-level outcome (base ALLOW / REVIEW / DENY)
    is folded into the `policy_outcome` choice. The ladder mirrors the engine's
    DENY-before-(ALLOW/REVIEW/DENY-policy) order.

  Emitted labels: deny_proposal, deny_action_not_whitelisted, deny_unknown_intent,
  deny_unknown_page_fixture, deny_url_not_allowed, deny_page_state_mismatch,
  deny_selector_outside_target_schema, deny_target_not_found,
  deny_action_does_not_match_intent, deny_action_not_supported_by_target,
  deny_target_disabled, deny_sensitive_field_read, deny_sensitive_raw_value,
  deny_sensitive_value_ref, deny_value_ref_only_for_sensitive,
  deny_typed_action_requires_value, deny_non_typing_carries_value,
  deny_retry_limit_exceeded, deny_non_contiguous_retry,
  deny_not_permitted_by_policy, review_high_risk_action, allow.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one typed RPA action {request_id, intent, action, selector, value, value_ref, page_state, attempt} with valid enums and a valid idempotency shape?"
      | answer    | meaning                                                    |
      | accepted  | exactly one structurally valid typed action                 |
      | malformed | parse error, bad enum, bad selector or key conflict         |
    And the answer comes from the field "ev_proposal"
    And a choice question "deny_reason" asks "Against the fixed policy table and DOM fixtures, which single ordered check does the deterministic validator refuse first?"
      | answer                        | meaning                                                       |
      | none                          | no refusal: the action is permitted by every deny check        |
      | action_not_whitelisted        | the action is not in the authoritative action set            |
      | unknown_intent                | the intent has no authoritative policy                        |
      | unknown_page_fixture          | the policy page has no fixture                                |
      | url_not_allowed               | the page URL fails the authoritative allow / forbid policy     |
      | page_state_mismatch           | the proposed page_state differs from the fixture              |
      | selector_outside_target_schema | the selector is not in the intent's authoritative set        |
      | target_not_found              | no DOM element matches the selector                           |
      | action_does_not_match_intent  | the action differs from the intent's policy action            |
      | action_not_supported_by_target | the target element does not support the action              |
      | target_disabled               | the target element is disabled                                |
      | sensitive_field_read          | a read is attempted on a sensitive field                      |
      | sensitive_raw_value           | a raw value is carried for a sensitive field                  |
      | sensitive_value_ref           | a value_ref is not in the sensitive field's allowed set       |
      | value_ref_only_for_sensitive  | a value_ref is carried for a non-sensitive field              |
      | typed_action_requires_value   | a type action carries no value                                |
      | non_typing_carries_value      | a non-typing action carries a value or value_ref              |
      | retry_limit_exceeded          | the attempt exceeds the intent's max attempts                 |
      | non_contiguous_retry          | the attempt is not the previous attempt plus one              |
    And the answer comes from the field "ev_deny_reason"
    And a choice question "policy_outcome" asks "When no deny check fires, which base decision does the intent's authoritative policy carry?"
      | answer          | meaning                                                  |
      | allow           | the policy's base decision is ALLOW                       |
      | review          | the policy's base decision is REVIEW                      |
      | policy_deny     | the policy's base decision is DENY                        |
    And the answer comes from the field "ev_policy_outcome"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "deny_proposal" when the answer to "proposal" is "malformed"
    And label "deny_action_not_whitelisted" when the answer to "deny_reason" is "action_not_whitelisted"
    And label "deny_unknown_intent" when the answer to "deny_reason" is "unknown_intent"
    And label "deny_unknown_page_fixture" when the answer to "deny_reason" is "unknown_page_fixture"
    And label "deny_url_not_allowed" when the answer to "deny_reason" is "url_not_allowed"
    And label "deny_page_state_mismatch" when the answer to "deny_reason" is "page_state_mismatch"
    And label "deny_selector_outside_target_schema" when the answer to "deny_reason" is "selector_outside_target_schema"
    And label "deny_target_not_found" when the answer to "deny_reason" is "target_not_found"
    And label "deny_action_does_not_match_intent" when the answer to "deny_reason" is "action_does_not_match_intent"
    And label "deny_action_not_supported_by_target" when the answer to "deny_reason" is "action_not_supported_by_target"
    And label "deny_target_disabled" when the answer to "deny_reason" is "target_disabled"
    And label "deny_sensitive_field_read" when the answer to "deny_reason" is "sensitive_field_read"
    And label "deny_sensitive_raw_value" when the answer to "deny_reason" is "sensitive_raw_value"
    And label "deny_sensitive_value_ref" when the answer to "deny_reason" is "sensitive_value_ref"
    And label "deny_value_ref_only_for_sensitive" when the answer to "deny_reason" is "value_ref_only_for_sensitive"
    And label "deny_typed_action_requires_value" when the answer to "deny_reason" is "typed_action_requires_value"
    And label "deny_non_typing_carries_value" when the answer to "deny_reason" is "non_typing_carries_value"
    And label "deny_retry_limit_exceeded" when the answer to "deny_reason" is "retry_limit_exceeded"
    And label "deny_non_contiguous_retry" when the answer to "deny_reason" is "non_contiguous_retry"
    And label "deny_not_permitted_by_policy" when the answer to "policy_outcome" is "policy_deny"
    And label "review_high_risk_action" when the answer to "policy_outcome" is "review"
    And label "allow" otherwise
    And keep the state key "request_id"
