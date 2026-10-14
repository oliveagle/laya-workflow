@decision
Feature: code_review_gate
  Guard for LLM-proposed code-review issues as a two-node DFA, port of the
  deterministic review gate (code/qwen_vm/app_scenarios/code_review_gate). The
  model proposes only typed issues (issue_type, file, hunk_id, line, reason,
  suggestion); the diff/hunk existence, the token and AST evidence, the issue
  validity, the duplicates, the test impact, the severity and the final
  ACCEPT / REVISE / REJECT decision are computed locally from fixed fixtures.
  The gate never reads a repository and never runs code.

  The engine's own verbs: a change is ACCEPTed, sent back for REVISEion, or
  REJECTed; every proposed issue is either accepted as locally evidenced or
  recorded as a rejected proposal with the engine's own first cause.

  States: proposal -> review. The parse node folds the balanced-JSON / fenced
  extraction and the proposal schema (exactly the key proposal, exactly the key
  issues, at most 12 typed issues, each exactly the six required fields). A
  proposal that fails parse never reaches the reviewer.

  Folding boundaries (documented, not computed in the DSL):
  - The envelope recovery, the duplicate-key rejection and the typed-issue schema
    fold into the `proposal` choice.
  - The fixed diff fixtures, the token evidence, the AST evidence, the local
    issue scan, the test-impact catalogue and the severity table are the
    authority; the harness runs them and hands the DSL one answer per check.
  - The engine validates each proposed issue with an ordered early-return chain
    (file-in-diff, hunk-exists, line-in-hunk, line-changed, duplicate, local
    evidence); because a rejected proposal does not change the terminal verdict
    it is folded into the `gate` choice (the engine's own first cause over the
    proposals, in proposal order). A rejected issue is only observable when the
    change itself is still taken.
  - The terminal verdict is computed over the authoritatively validated issue
    set (accepted proposals *plus* the locally missed issues) and folds into the
    `decision` choice. The label ladder mirrors the engine's REJECT-before-
    REVISE-before-ACCEPT order, and, when the change is taken, the first issue
    the gate refused.

  Emitted labels: reject_proposal, reject, revise, accept, accept_issue_file_not_in_diff,
  accept_issue_hunk_not_found, accept_issue_line_not_in_hunk,
  accept_issue_line_not_changed, accept_issue_duplicate_issue,
  accept_issue_no_local_evidence.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: proposal
    Given a choice question "proposal" asks "Did the model response yield exactly one JSON object with exactly the key proposal, whose value has exactly the key issues (a list of at most 12 typed issues, each with exactly the keys issue_type, file, hunk_id, line, reason and suggestion)?"
      | answer    | meaning                                                                                       |
      | parsed    | exactly one schema-valid typed review proposal                                                 |
      | malformed | no JSON object, a duplicate key, a wrong top-level shape, an unknown issue_type, an issue field set that is not exactly the six required keys, or an out-of-range field |
    And the answer comes from the field "ev_proposal"
    When the answer "parsed" goes to "review"
    And any other answer goes to "STOP"
    Then label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "proposal_ok" otherwise
    And keep the state key "review_id"

  Scenario: review
    Given a choice question "gate" asks "Against the fixed diff and the local token/AST evidence, which single ordered check refuses the first proposed issue, if any?"
      | answer            | meaning                                                              |
      | none              | no proposed issue is refused                                          |
      | file_not_in_diff  | the issue's file is not in the fixed diff                            |
      | hunk_not_found    | the hunk_id does not exist for that file                            |
      | line_not_in_hunk  | the line is not present in the hunk                                 |
      | line_not_changed  | the line is hunk context rather than a changed line                 |
      | duplicate_issue   | an identical typed issue was already accepted                       |
      | no_local_evidence | the type and location match no token, AST or test evidence          |
    And the answer comes from the field "ev_gate"
    And a choice question "decision" asks "Over the authoritatively validated issue set (accepted proposals plus the locally missed issues), what is the terminal review verdict?"
      | answer  | meaning                                                        |
      | reject  | a critical issue remains, so the change is rejected            |
      | revise  | a high or medium issue remains, so the change needs revision   |
      | accept  | no high, medium or critical validated issue remains            |
    And the answer comes from the field "ev_decision"
    When any other answer goes to "STOP"
    Then the primary question is "decision"
    And the action reads the question "decision"
    And label "reject" when the answer to "decision" is "reject"
    And label "revise" when the answer to "decision" is "revise"
    And label "accept_issue_file_not_in_diff" when the answer to "gate" is "file_not_in_diff"
    And label "accept_issue_hunk_not_found" when the answer to "gate" is "hunk_not_found"
    And label "accept_issue_line_not_in_hunk" when the answer to "gate" is "line_not_in_hunk"
    And label "accept_issue_line_not_changed" when the answer to "gate" is "line_not_changed"
    And label "accept_issue_duplicate_issue" when the answer to "gate" is "duplicate_issue"
    And label "accept_issue_no_local_evidence" when the answer to "gate" is "no_local_evidence"
    And label "accept" otherwise
    And keep the state key "review_id"
