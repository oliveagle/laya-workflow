@decision
Feature: compliance_audit_evidence_guard
  Guard for LLM-proposed compliance audit evidence checklist as a single triage
  node. The model proposes one typed checklist (request_id, audit_id,
  evidence_items [{evidence_id, description, status, category}],
  compliance_framework); this workflow owns the required-category coverage, the
  collected-evidence floor, the missing-percentage ceiling and the
  missing+pending follow-up escalation. Port of
  code/qwen_vm/app_scenarios/compliance_audit_evidence_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (request_id / audit_id grammars, non-empty
    evidence_items, status enum, non-empty category, non-empty framework,
    dangerous-token rejection, fenced-code stripping) fold into the `proposal`
    choice.
  - The audit policy (required_categories, max_missing_pct) is a fixed fixture.
    The missing/pending counts, the collected count, the required-category
    coverage and the missing percentage are folded into verdict choices. The
    missing+pending work-in-progress escalation is checked before the reject
    checks, as in the Python source.

  Emitted labels: reject_proposal, escalate_wip, reject_uncovered,
  reject_none_collected, reject_missing_pct, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, audit_id, evidence_items [{evidence_id, description, status, category}], compliance_framework} object (audit_id grammar, non-empty items, valid status enum, no dangerous tokens)?"
      | answer    | meaning                                                              |
      | accepted  | exactly one schema-valid audit checklist                             |
      | malformed | parse error, missing key, bad request_id/audit_id, bad status or dangerous token |
    And the answer comes from the field "ev_proposal"
    And a choice question "progress" asks "Are there simultaneously missing and pending evidence items (work in progress)?"
      | answer  | meaning                                        |
      | clean   | not both missing and pending                    |
      | wip     | at least one missing and at least one pending   |
    And the answer comes from the field "ev_progress"
    And a choice question "coverage" asks "Does the checklist cover every policy required category?"
      | answer    | meaning                                    |
      | ok        | every required category is present          |
      | uncovered | some required category has no item          |
    And the answer comes from the field "ev_coverage"
    And a choice question "collected" asks "Is at least one evidence item collected or marked not_applicable?"
      | answer | meaning                                    |
      | ok     | at least one collected / not_applicable      |
      | none   | nothing collected                            |
    And the answer comes from the field "ev_collected"
    And a choice question "missing_pct" asks "Is the fraction of missing evidence items within the policy max_missing_pct?"
      | answer | meaning                            |
      | ok     | missing fraction <= max_missing_pct |
      | over   | missing fraction > max_missing_pct  |
    And the answer comes from the field "ev_missing_pct"
    When any other answer goes to "STOP"
    Then the primary question is "progress"
    And the action reads the question "progress"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "escalate_wip" when the answer to "progress" is "wip"
    And label "reject_uncovered" when the answer to "coverage" is "uncovered"
    And label "reject_none_collected" when the answer to "collected" is "none"
    And label "reject_missing_pct" when the answer to "missing_pct" is "over"
    And label "approve" otherwise
    And keep the state key "request_id"
