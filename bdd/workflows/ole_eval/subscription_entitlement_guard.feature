@decision
Feature: subscription_entitlement_guard
  Guard for LLM-proposed subscription entitlement changes as a single triage
  node. The model proposes only a typed intent (request_id, subscription_id,
  requested_effective_date, target_plan, target_seats, add_on_intents); this
  workflow owns the subscription ledger, the plan and add-on catalogs, the term
  and billing-cycle windows, the add-on plan dependencies, the seat policy, the
  commitment, the proration and credit preview and the control-total check,
  computing the exact effective date and preview itself. It never bills a
  customer. Port of code/qwen_vm/app_scenarios/subscription_entitlement_guard.

  The engine's own verbs: APPROVE / MODIFY / REJECT (and REJECTED for a
  malformed envelope) — the entitlement change is accepted, locally corrected,
  or refused.

  Folding boundaries (documented, not computed in the DSL):
  - Proposal parsing (balanced-JSON extraction from a fenced reply, duplicate-key
    scan, the exact {request_id, subscription_id, requested_effective_date,
    target_plan, target_seats, add_on_intents[]} shape, the identifier and date
    grammars, the add-on action whitelist, the seat bound and the dangerous-token
    scan over every string) folds into the `proposal` choice.
  - The subscription ledger, the plan catalog (rank, seat range, price, add-on
    dependencies) and the add-on catalog are a fixed fixture; the harness
    resolves them and hands the DSL one answer per check.
  - The term / billing-cycle windows, the plan dependencies, the seat policy and
    the proration + credit preview are folded into the `reject_reason` and
    `revision` choices; the engine may append several reasons, so each choice
    carries the engine's own first-appended reason and the ladder emits it,
    mirroring the engine's own REJECT-before-MODIFY precedence.

  Emitted labels: rejected_proposal, reject_duplicate_request,
  reject_unknown_subscription, reject_effective_date_outside_term,
  reject_effective_date_outside_billing_cycle, reject_unknown_target_plan,
  reject_unknown_add_on, reject_add_on_dependency_unsatisfied,
  reject_seat_count_out_of_range, reject_plan_seat_limit_exceeded,
  reject_billing_preview_control_total, modify_downgrade_deferred,
  modify_seat_reduction_clamped, modify_missing_add_on_disable_ignored,
  modify_incompatible_add_on_removed, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, subscription_id, requested_effective_date, target_plan, target_seats, add_on_intents[]} object with valid identifiers, a valid date and no dangerous tokens?"
      | answer    | meaning                                                                   |
      | accepted  | exactly one structurally valid typed entitlement intent                    |
      | malformed | parse error, bad shape, bad identifier, bad date or dangerous token        |
    And the answer comes from the field "ev_proposal"
    And a choice question "reject_reason" asks "Once the envelope is resolved, which single reason does the deterministic entitlement guard report first?"
      | answer                              | meaning                                                    |
      | none                                | no rejection reason: the change is at least locally correct |
      | duplicate_request                   | request_id already evaluated: short-circuit                 |
      | unknown_subscription                | subscription_id is not in the subscription ledger           |
      | outside_term                        | the effective date is outside the subscription term         |
      | outside_billing_cycle               | the effective date is outside the billing-cycle window      |
      | unknown_target_plan                 | target_plan is not in the plan catalog                      |
      | unknown_add_on                      | an add-on intent names an add-on outside the catalog        |
      | add_on_dependency_unsatisfied       | an enabled add-on's plan dependency does not hold           |
      | seat_count_out_of_range             | target_seats is outside the absolute 1..1000 bound          |
      | plan_seat_limit_exceeded            | target_seats is outside the target plan's own seat range   |
      | billing_preview_control_total_invalid | the computed preview breaks the control total            |
    And the answer comes from the field "ev_reject_reason"
    And a choice question "revision" asks "When the entitlement change is otherwise accepted, which single local revision does the engine record first?"
      | answer                        | meaning                                                       |
      | none                          | the change needs no local correction                          |
      | downgrade_deferred            | a downgrade is deferred to the term end                       |
      | seat_reduction_clamped        | the seat reduction is clamped to the commitment               |
      | missing_add_on_disable_ignored | a disable names an add-on the subscription does not hold     |
      | incompatible_add_on_removed   | a plan-incompatible add-on is removed from the target set     |
    And the answer comes from the field "ev_revision"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "rejected_proposal" when the answer to "proposal" is "malformed"
    And label "reject_duplicate_request" when the answer to "reject_reason" is "duplicate_request"
    And label "reject_unknown_subscription" when the answer to "reject_reason" is "unknown_subscription"
    And label "reject_effective_date_outside_term" when the answer to "reject_reason" is "outside_term"
    And label "reject_effective_date_outside_billing_cycle" when the answer to "reject_reason" is "outside_billing_cycle"
    And label "reject_unknown_target_plan" when the answer to "reject_reason" is "unknown_target_plan"
    And label "reject_unknown_add_on" when the answer to "reject_reason" is "unknown_add_on"
    And label "reject_add_on_dependency_unsatisfied" when the answer to "reject_reason" is "add_on_dependency_unsatisfied"
    And label "reject_seat_count_out_of_range" when the answer to "reject_reason" is "seat_count_out_of_range"
    And label "reject_plan_seat_limit_exceeded" when the answer to "reject_reason" is "plan_seat_limit_exceeded"
    And label "reject_billing_preview_control_total" when the answer to "reject_reason" is "billing_preview_control_total_invalid"
    And label "modify_downgrade_deferred" when the answer to "revision" is "downgrade_deferred"
    And label "modify_seat_reduction_clamped" when the answer to "revision" is "seat_reduction_clamped"
    And label "modify_missing_add_on_disable_ignored" when the answer to "revision" is "missing_add_on_disable_ignored"
    And label "modify_incompatible_add_on_removed" when the answer to "revision" is "incompatible_add_on_removed"
    And label "approve" otherwise
    And keep the state key "request_id"
