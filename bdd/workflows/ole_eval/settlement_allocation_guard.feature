@decision
Feature: settlement_allocation_guard
  Guard for LLM-proposed settlement allocation as a single triage node. The
  model proposes only a typed intent (request_id, settlement_case, preferences,
  priority_rationale); this workflow owns the case ledger, the invoice set, the
  canonical order, tax / credit / waterfall allocation, rounding, the ledger and
  cash-control checks and the allocation caps, computing the exact per-invoice
  amounts itself. It never authorizes or executes a payment. Port of
  code/qwen_vm/app_scenarios/settlement_allocation_guard.

  The engine's own verbs: APPROVE / MODIFY / REJECT (and REJECTED for a
  malformed envelope) — the allocation is accepted, locally corrected, or
  refused.

  Folding boundaries (documented, not computed in the DSL):
  - Proposal parsing (balanced-JSON extraction from a fenced reply, duplicate-key
    scan, the exact {request_id, settlement_case, preferences[], priority_rationale}
    shape, the identifier and invoice-id grammars, the rationale whitelist, the
    1..32 preference bound, unique invoice/rank pairs and the dangerous-token
    scan over every string) folds into the `proposal` choice.
  - The settlement case table (invoices, amounts, tax rules, credit, waterfall
    order, canonical rationale, allocation caps) is a fixed fixture; the harness
    resolves it and hands the DSL one answer per check.
  - The ledger, the canonical order, the exact allocation, rounding, the ledger
    and cash-control verification and the caps are folded into the
    `reject_reason` and `revision` choices; the engine may append several
    reasons, so each choice carries the engine's own first-appended reason and
    the ladder emits it, mirroring the engine's own REJECT-before-MODIFY
    precedence.

  Emitted labels: rejected_proposal, reject_duplicate_request,
  reject_unknown_settlement_case, reject_unknown_invoice_preference,
  reject_rank_out_of_range, reject_incomplete_preferences,
  reject_ledger_not_balanced, reject_cash_control_total,
  reject_allocation_exceeds_cap, modify_priority_rationale,
  modify_allocation_order, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, settlement_case, preferences[], priority_rationale} object with valid identifiers, a whitelisted rationale and no dangerous tokens?"
      | answer    | meaning                                                                     |
      | accepted  | exactly one structurally valid typed allocation intent                       |
      | malformed | parse error, bad shape, bad identifier, bad rationale or dangerous token     |
    And the answer comes from the field "ev_proposal"
    And a choice question "reject_reason" asks "Once the envelope is resolved, which single reason does the deterministic allocator report first?"
      | answer                       | meaning                                                     |
      | none                         | no rejection reason: allocation is at least locally correct  |
      | duplicate_request            | request_id already evaluated: short-circuit                  |
      | unknown_settlement_case      | settlement_case is not in the case table                     |
      | unknown_invoice_preference   | a preference names an invoice outside the case               |
      | rank_out_of_range            | a preference rank exceeds the invoice count                  |
      | incomplete_preferences       | the preference set does not cover every invoice with a rank   |
      | ledger_not_balanced          | the computed allocation does not balance the ledger           |
      | cash_control_total_invalid   | the computed allocation breaks the cash-control total         |
      | allocation_exceeds_cap       | a computed allocation exceeds its cap                         |
    And the answer comes from the field "ev_reject_reason"
    And a choice question "revision" asks "When the allocation is otherwise accepted, which single local revision does the engine record first?"
      | answer                    | meaning                                             |
      | none                      | rationale and order match the canonical case         |
      | priority_rationale_modified | the proposal rationale differs from the canonical one |
      | allocation_order_modified | the proposed order differs from the canonical order  |
    And the answer comes from the field "ev_revision"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "rejected_proposal" when the answer to "proposal" is "malformed"
    And label "reject_duplicate_request" when the answer to "reject_reason" is "duplicate_request"
    And label "reject_unknown_settlement_case" when the answer to "reject_reason" is "unknown_settlement_case"
    And label "reject_unknown_invoice_preference" when the answer to "reject_reason" is "unknown_invoice_preference"
    And label "reject_rank_out_of_range" when the answer to "reject_reason" is "rank_out_of_range"
    And label "reject_incomplete_preferences" when the answer to "reject_reason" is "incomplete_preferences"
    And label "reject_ledger_not_balanced" when the answer to "reject_reason" is "ledger_not_balanced"
    And label "reject_cash_control_total" when the answer to "reject_reason" is "cash_control_total_invalid"
    And label "reject_allocation_exceeds_cap" when the answer to "reject_reason" is "allocation_exceeds_cap"
    And label "modify_priority_rationale" when the answer to "revision" is "priority_rationale_modified"
    And label "modify_allocation_order" when the answer to "revision" is "allocation_order_modified"
    And label "approve" otherwise
    And keep the state key "request_id"
