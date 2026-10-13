@decision
Feature: accessibility_content_guard
  Guard for LLM-proposed accessibility remediations as a single triage node. The
  model proposes only one typed remediation (content_id, issue_intent,
  target_node, remediation_intent, reason); this workflow owns the immutable
  content snapshot, the deterministic scoped checklist, the issue detection, the
  intent / target matching and the remediation allow-list, and returns the
  decision itself. It never edits a real page. Port of
  code/qwen_vm/app_scenarios/accessibility_content_guard.

  The engine's own verbs: ACCEPT / REVISE / REJECT — the remediation is
  accepted, locally revised, or refused.

  Folding boundaries (documented, not computed in the DSL):
  - Proposal parsing (the exact typed remediation shape, the content-id and
    intent grammars, the reason length bound and the unsafe-text scan) folds
    into the `proposal` choice.
  - The immutable content snapshot and the deterministic issue list are a fixed
    fixture; the harness resolves them and hands the DSL one answer per check.
  - The checklist, the intent / target matching, the remediation allow-list and
    the resolution state are folded into the `reject_reason` and `revision`
    choices; the engine returns early per reject, so each choice carries the
    engine's own first-appended reason and the ladder mirrors the engine's
    REJECT-before-REVISE-before-ACCEPT order.

  Emitted labels: reject_proposal, reject_content_id_mismatch,
  reject_no_issue_contradiction, reject_unsupported_intent,
  reject_target_mismatch, reject_remediation_not_allowed,
  revise_plural_incomplete, revise_unresolved_issues, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {content_id, issue_intent, target_node, remediation_intent, reason} object with valid identifiers and safe reason text?"
      | answer    | meaning                                                             |
      | accepted  | exactly one structurally valid typed remediation                    |
      | malformed | parse error, bad identifier, bad intent or unsafe reason text        |
    And the answer comes from the field "ev_proposal"
    And a choice question "reject_reason" asks "Against the immutable snapshot, which single scoped-checklist failure does the deterministic reviewer report first?"
      | answer                      | meaning                                                       |
      | none                        | no rejection: the remediation at least clears the hard checks |
      | content_id_mismatch         | the proposal content_id differs from the snapshot             |
      | no_issue_contradiction      | the proposal claims no issue while the snapshot has findings  |
      | unsupported_intent          | the issue_intent is not in the authoritative snapshot         |
      | target_mismatch             | the target_node differs from the authoritative target         |
      | remediation_not_allowed     | the remediation_intent is not allowed for the issue          |
    And the answer comes from the field "ev_reject_reason"
    And a choice question "revision" asks "When the remediation clears the hard checks, which single local revision does the engine record first?"
      | answer            | meaning                                                          |
      | none              | nothing left to revise                                           |
      | plural_incomplete | a single plural form does not cover every missing category       |
      | unresolved_issues | the proposal leaves other deterministic issues unresolved        |
    And the answer comes from the field "ev_revision"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_content_id_mismatch" when the answer to "reject_reason" is "content_id_mismatch"
    And label "reject_no_issue_contradiction" when the answer to "reject_reason" is "no_issue_contradiction"
    And label "reject_unsupported_intent" when the answer to "reject_reason" is "unsupported_intent"
    And label "reject_target_mismatch" when the answer to "reject_reason" is "target_mismatch"
    And label "reject_remediation_not_allowed" when the answer to "reject_reason" is "remediation_not_allowed"
    And label "revise_plural_incomplete" when the answer to "revision" is "plural_incomplete"
    And label "revise_unresolved_issues" when the answer to "revision" is "unresolved_issues"
    And label "approve" otherwise
    And keep the state key "content_id"
