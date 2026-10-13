@decision
Feature: privacy_retention_guard
  Guard for LLM-proposed retention / preservation / deletion choices over fixed
  abstract privacy records, as a single triage node. The model proposes only a
  typed intent (data_class, retention_intent, deletion_intent); this workflow
  owns the record ledger, the data-class policy, the jurisdiction policy, the
  retention clock and deadline, the lawful-basis check, the legal hold, the
  pseudonymization eligibility and the deletion/preservation precedence,
  computing the exact scheduled action and its timestamp itself. It never
  touches a datastore. Port of code/qwen_vm/app_scenarios/privacy_retention_guard.

  Third three-valued port shape with the engine's own verbs: the engine returns
  APPROVE / MODIFY / REJECT — the scheduled action is accepted, locally
  corrected, or the proposal is refused.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (the 4 KiB raw cap, the exact {proposal:{data_class,
    retention_intent, deletion_intent}} shape, the data-class / retention-intent
    / deletion-intent enums, the fixed record-id grammar, the duplicate-key
    scan, the non-finite-number rejection and the forbidden authority /
    side-effect fields) fold into the `proposal` choice.
  - The data-class policy table (retention days, retention clock, allowed
    lawful bases, pseudonymization eligibility) and the jurisdiction policy
    table (deletion grace days, maximum extension days, pseudonymization
    permission) are a fixed fixture; the harness resolves them and hands the
    DSL one answer per check.
  - The record ledger (an abstract record id already evaluated by this guard),
    the authoritative record class, the jurisdiction registration and the whole
    post-gate precedence chain are folded into the `record`, `data_class`,
    `jurisdiction` and `outcome` choices; one `outcome` question carries every
    remaining engine branch, so each reason keeps its own label without ten
    separate questions, and the ladder emits the engine's own first-match in
    its own order.

  Emitted labels: reject_proposal, reject_duplicate, reject_data_class_mismatch,
  reject_jurisdiction, reject_unlawful_basis_requires_deletion,
  approve_delete_unlawful, approve_legal_hold_preservation,
  modify_legal_hold_precedence, approve_retention_expired_delete,
  modify_deletion_precedence_pseudonymization,
  modify_retention_expired_requires_deletion,
  modify_minimum_retention_not_reached, approve_schedule_retention_review,
  approve_retain_until_deadline, approve_pseudonymization_within_retention,
  modify_pseudonymization_instead_of_early_delete,
  modify_early_deletion_not_permitted.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {data_class, retention_intent, deletion_intent} object with valid enums, a valid record id and no authority or side-effect keys?"
      | answer    | meaning                                                                        |
      | accepted  | exactly one structurally valid typed privacy proposal                          |
      | malformed | parse error, missing/extra field, forbidden key, bad enum, oversize or duplicate key |
    And the answer comes from the field "ev_proposal"
    And a choice question "record" asks "Is the abstract record id unseen by this guard?"
      | answer    | meaning                        |
      | fresh     | record id is new               |
      | duplicate | record already evaluated       |
    And the answer comes from the field "ev_record"
    And a choice question "data_class" asks "Does the proposal data class equal the authoritative record class?"
      | answer   | meaning                            |
      | match    | proposal class equals the record   |
      | mismatch | proposal class differs from record |
    And the answer comes from the field "ev_data_class"
    And a choice question "jurisdiction" asks "Does a fixed policy exist for the record's jurisdiction?"
      | answer       | meaning                     |
      | configured   | jurisdiction policy resolved |
      | unconfigured | no fixed policy exists       |
    And the answer comes from the field "ev_jurisdiction"
    And a choice question "outcome" asks "Which single branch does the deterministic precedence chain take once the proposal, record, class and jurisdiction gates pass?"
      | answer                                            | meaning                                                              |
      | reject_unlawful_basis_requires_deletion           | lawful basis not allowed and no deletion requested: refuse           |
      | approve_delete_unlawful                           | lawful basis not allowed but deletion requested: schedule deletion  |
      | approve_legal_hold_preservation                   | legal hold active and no deletion requested: preserve               |
      | modify_legal_hold_precedence                      | legal hold active but deletion requested: hold wins                 |
      | approve_retention_expired_delete                  | retention expired and delete_now requested: schedule deletion       |
      | modify_deletion_precedence_pseudonymization       | retention expired but pseudonymize requested: delete wins           |
      | modify_retention_expired_requires_deletion        | retention expired and nothing requested: deletion required          |
      | modify_minimum_retention_not_reached              | within retention but expire requested: too early                    |
      | approve_schedule_retention_review                 | within retention, review requested: schedule review                 |
      | approve_retain_until_deadline                     | within retention, retain requested: retain until deadline           |
      | approve_pseudonymization_within_retention         | within retention, pseudonymize requested and eligible: pseudonymize |
      | modify_pseudonymization_instead_of_early_delete   | within retention, early delete requested and eligible: pseudonymize |
      | modify_early_deletion_not_permitted               | within retention and pseudonymization ineligible: preserve          |
    And the answer comes from the field "ev_outcome"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_duplicate" when the answer to "record" is "duplicate"
    And label "reject_data_class_mismatch" when the answer to "data_class" is "mismatch"
    And label "reject_jurisdiction" when the answer to "jurisdiction" is "unconfigured"
    And label "reject_unlawful_basis_requires_deletion" when the answer to "outcome" is "reject_unlawful_basis_requires_deletion"
    And label "approve_delete_unlawful" when the answer to "outcome" is "approve_delete_unlawful"
    And label "approve_legal_hold_preservation" when the answer to "outcome" is "approve_legal_hold_preservation"
    And label "modify_legal_hold_precedence" when the answer to "outcome" is "modify_legal_hold_precedence"
    And label "approve_retention_expired_delete" when the answer to "outcome" is "approve_retention_expired_delete"
    And label "modify_deletion_precedence_pseudonymization" when the answer to "outcome" is "modify_deletion_precedence_pseudonymization"
    And label "modify_retention_expired_requires_deletion" when the answer to "outcome" is "modify_retention_expired_requires_deletion"
    And label "modify_minimum_retention_not_reached" when the answer to "outcome" is "modify_minimum_retention_not_reached"
    And label "approve_schedule_retention_review" when the answer to "outcome" is "approve_schedule_retention_review"
    And label "approve_retain_until_deadline" when the answer to "outcome" is "approve_retain_until_deadline"
    And label "approve_pseudonymization_within_retention" when the answer to "outcome" is "approve_pseudonymization_within_retention"
    And label "modify_pseudonymization_instead_of_early_delete" when the answer to "outcome" is "modify_pseudonymization_instead_of_early_delete"
    And label "modify_early_deletion_not_permitted" otherwise
    And keep the state key "record_id"
