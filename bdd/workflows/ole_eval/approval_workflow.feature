@decision
Feature: approval_workflow
  Multi-role order approval as a finite-state decision graph. The model proposes
  exactly one typed proposal per turn (a role plus an action); this workflow owns
  quorum-by-amount-band, one vote per role, requester self-approval and the
  terminal-state rules. Port of code/qwen_vm/app_scenarios/approval_workflow.
  submit folds the amount into a band (small = 0.01..1000.00 manager only;
  medium = 1000.01..10000.00 manager + finance; large = 10000.01..100000.00
  manager + finance + security). A rejected proposal ends the episode and the
  workflow state is unchanged; an accepted proposal advances it.

  Background:
    Given the workflow runs at most 12 iterations

  Scenario: submitted
    Given a choice question "proposal" asks "A new approval request is open and nobody has voted. Classify the utterance into one typed proposal; the band is read from the amount."
      | answer        | meaning |
      | submit_small  | the requester submits 0.01..1000.00 (manager only) |
      | submit_medium | the requester submits 1000.01..10000.00 (manager + finance) |
      | submit_large  | the requester submits 10000.01..100000.00 (manager + finance + security) |
      | cancel        | the original requester cancels before any review |
    And the answer comes from the field "ev_submitted"
    When the answer "submit_small" goes to "await_manager"
    And the answer "submit_medium" goes to "await_mgr_fin"
    And the answer "submit_large" goes to "await_mfs"
    And the answer "cancel" goes to "cancelled"
    And any other answer goes to "STOP"
    Then label "start_review" when the answer to "proposal" is "submit_small"
    And label "start_review" when the answer to "proposal" is "submit_medium"
    And label "start_review" when the answer to "proposal" is "submit_large"
    And label "cancel_request" when the answer to "proposal" is "cancel"
    And label "reject_proposal" otherwise

  Scenario: await_manager
    Given a choice question "proposal" asks "Small-band review: one manager approval already meets quorum. A reject vote fails the request; an extra role, a requester self-vote or anything else is refused."
      | answer            | meaning |
      | manager_approve   | the manager approves; quorum is met |
      | manager_reject    | the manager rejects the request |
      | extra_role        | finance or security tries to approve, but is not required |
      | requester_attempt | the requester tries to approve their own request |
      | self_approval     | a reviewer whose actor id is the requester approves |
    And the answer comes from the field "ev_await_manager"
    When the answer "manager_approve" goes to "approved"
    And the answer "manager_reject" goes to "rejected"
    And any other answer goes to "STOP"
    Then label "approve_request" when the answer to "proposal" is "manager_approve"
    And label "reject_request" when the answer to "proposal" is "manager_reject"
    And label "role_not_required" when the answer to "proposal" is "extra_role"
    And label "requester_cannot_vote" when the answer to "proposal" is "requester_attempt"
    And label "self_approval_forbidden" when the answer to "proposal" is "self_approval"
    And label "reject_proposal" otherwise

  Scenario: await_mgr_fin
    Given a choice question "proposal" asks "Medium-band review with no vote yet: manager and finance must both approve. Either may vote first."
      | answer            | meaning |
      | manager_approve   | the manager approves; finance is still missing |
      | finance_approve   | finance approves; the manager is still missing |
      | manager_reject    | the manager rejects the request |
      | finance_reject    | finance rejects the request |
      | extra_role        | security tries to approve, but is not required |
      | requester_attempt | the requester tries to approve their own request |
      | self_approval     | a reviewer whose actor id is the requester approves |
    And the answer comes from the field "ev_await_mgr_fin"
    When the answer "manager_approve" goes to "await_mgr"
    And the answer "finance_approve" goes to "await_fin"
    And the answer "manager_reject" goes to "rejected"
    And the answer "finance_reject" goes to "rejected"
    And any other answer goes to "STOP"
    Then label "record_approval" when the answer to "proposal" is "manager_approve"
    And label "record_approval" when the answer to "proposal" is "finance_approve"
    And label "reject_request" when the answer to "proposal" is "manager_reject"
    And label "reject_request" when the answer to "proposal" is "finance_reject"
    And label "role_not_required" when the answer to "proposal" is "extra_role"
    And label "requester_cannot_vote" when the answer to "proposal" is "requester_attempt"
    And label "self_approval_forbidden" when the answer to "proposal" is "self_approval"
    And label "reject_proposal" otherwise

  Scenario: await_mgr
    Given a choice question "proposal" asks "Medium-band review with the manager already approved: only finance is missing. A repeat manager vote is a duplicate."
      | answer            | meaning |
      | finance_approve   | finance approves; quorum is met |
      | finance_reject    | finance rejects the request |
      | manager_again     | the manager votes a second time |
      | requester_attempt | the requester tries to approve their own request |
      | self_approval     | a reviewer whose actor id is the requester approves |
    And the answer comes from the field "ev_await_mgr"
    When the answer "finance_approve" goes to "approved"
    And the answer "finance_reject" goes to "rejected"
    And any other answer goes to "STOP"
    Then label "approve_request" when the answer to "proposal" is "finance_approve"
    And label "reject_request" when the answer to "proposal" is "finance_reject"
    And label "duplicate_role_vote" when the answer to "proposal" is "manager_again"
    And label "requester_cannot_vote" when the answer to "proposal" is "requester_attempt"
    And label "self_approval_forbidden" when the answer to "proposal" is "self_approval"
    And label "reject_proposal" otherwise

  Scenario: await_fin
    Given a choice question "proposal" asks "Medium-band review with finance already approved: only the manager is missing. A repeat finance vote is a duplicate."
      | answer            | meaning |
      | manager_approve   | the manager approves; quorum is met |
      | manager_reject    | the manager rejects the request |
      | finance_again     | finance votes a second time |
      | requester_attempt | the requester tries to approve their own request |
      | self_approval     | a reviewer whose actor id is the requester approves |
    And the answer comes from the field "ev_await_fin"
    When the answer "manager_approve" goes to "approved"
    And the answer "manager_reject" goes to "rejected"
    And any other answer goes to "STOP"
    Then label "approve_request" when the answer to "proposal" is "manager_approve"
    And label "reject_request" when the answer to "proposal" is "manager_reject"
    And label "duplicate_role_vote" when the answer to "proposal" is "finance_again"
    And label "requester_cannot_vote" when the answer to "proposal" is "requester_attempt"
    And label "self_approval_forbidden" when the answer to "proposal" is "self_approval"
    And label "reject_proposal" otherwise

  Scenario: await_mfs
    Given a choice question "proposal" asks "Large-band review with no vote yet: manager, finance and security must all approve. Any of the three may vote first."
      | answer            | meaning |
      | manager_approve   | the manager approves; finance and security are still missing |
      | finance_approve   | finance approves; manager and security are still missing |
      | security_approve  | security approves; manager and finance are still missing |
      | manager_reject    | the manager rejects the request |
      | finance_reject    | finance rejects the request |
      | security_reject   | security rejects the request |
      | requester_attempt | the requester tries to approve their own request |
      | self_approval     | a reviewer whose actor id is the requester approves |
    And the answer comes from the field "ev_await_mfs"
    When the answer "manager_approve" goes to "await_mfs_m"
    And the answer "finance_approve" goes to "await_mfs_f"
    And the answer "security_approve" goes to "await_mfs_s"
    And the answer "manager_reject" goes to "rejected"
    And the answer "finance_reject" goes to "rejected"
    And the answer "security_reject" goes to "rejected"
    And any other answer goes to "STOP"
    Then label "record_approval" when the answer to "proposal" is "manager_approve"
    And label "record_approval" when the answer to "proposal" is "finance_approve"
    And label "record_approval" when the answer to "proposal" is "security_approve"
    And label "reject_request" when the answer to "proposal" is "manager_reject"
    And label "reject_request" when the answer to "proposal" is "finance_reject"
    And label "reject_request" when the answer to "proposal" is "security_reject"
    And label "requester_cannot_vote" when the answer to "proposal" is "requester_attempt"
    And label "self_approval_forbidden" when the answer to "proposal" is "self_approval"
    And label "reject_proposal" otherwise

  Scenario: await_mfs_m
    Given a choice question "proposal" asks "Large-band review with the manager already approved: finance and security are still missing. A repeat manager vote is a duplicate."
      | answer            | meaning |
      | finance_approve   | finance approves; security is still missing |
      | security_approve  | security approves; finance is still missing |
      | finance_reject    | finance rejects the request |
      | security_reject   | security rejects the request |
      | manager_again     | the manager votes a second time |
      | requester_attempt | the requester tries to approve their own request |
    And the answer comes from the field "ev_await_mfs_m"
    When the answer "finance_approve" goes to "await_mfs_mf"
    And the answer "security_approve" goes to "await_mfs_ms"
    And the answer "finance_reject" goes to "rejected"
    And the answer "security_reject" goes to "rejected"
    And any other answer goes to "STOP"
    Then label "record_approval" when the answer to "proposal" is "finance_approve"
    And label "record_approval" when the answer to "proposal" is "security_approve"
    And label "reject_request" when the answer to "proposal" is "finance_reject"
    And label "reject_request" when the answer to "proposal" is "security_reject"
    And label "duplicate_role_vote" when the answer to "proposal" is "manager_again"
    And label "requester_cannot_vote" when the answer to "proposal" is "requester_attempt"
    And label "reject_proposal" otherwise

  Scenario: await_mfs_f
    Given a choice question "proposal" asks "Large-band review with finance already approved: manager and security are still missing. A repeat finance vote is a duplicate."
      | answer            | meaning |
      | manager_approve   | the manager approves; security is still missing |
      | security_approve  | security approves; the manager is still missing |
      | manager_reject    | the manager rejects the request |
      | security_reject   | security rejects the request |
      | finance_again     | finance votes a second time |
      | requester_attempt | the requester tries to approve their own request |
    And the answer comes from the field "ev_await_mfs_f"
    When the answer "manager_approve" goes to "await_mfs_mf"
    And the answer "security_approve" goes to "await_mfs_fs"
    And the answer "manager_reject" goes to "rejected"
    And the answer "security_reject" goes to "rejected"
    And any other answer goes to "STOP"
    Then label "record_approval" when the answer to "proposal" is "manager_approve"
    And label "record_approval" when the answer to "proposal" is "security_approve"
    And label "reject_request" when the answer to "proposal" is "manager_reject"
    And label "reject_request" when the answer to "proposal" is "security_reject"
    And label "duplicate_role_vote" when the answer to "proposal" is "finance_again"
    And label "requester_cannot_vote" when the answer to "proposal" is "requester_attempt"
    And label "reject_proposal" otherwise

  Scenario: await_mfs_s
    Given a choice question "proposal" asks "Large-band review with security already approved: manager and finance are still missing. A repeat security vote is a duplicate."
      | answer            | meaning |
      | manager_approve   | the manager approves; finance is still missing |
      | finance_approve   | finance approves; the manager is still missing |
      | manager_reject    | the manager rejects the request |
      | finance_reject    | finance rejects the request |
      | security_again    | security votes a second time |
      | requester_attempt | the requester tries to approve their own request |
    And the answer comes from the field "ev_await_mfs_s"
    When the answer "manager_approve" goes to "await_mfs_ms"
    And the answer "finance_approve" goes to "await_mfs_fs"
    And the answer "manager_reject" goes to "rejected"
    And the answer "finance_reject" goes to "rejected"
    And any other answer goes to "STOP"
    Then label "record_approval" when the answer to "proposal" is "manager_approve"
    And label "record_approval" when the answer to "proposal" is "finance_approve"
    And label "reject_request" when the answer to "proposal" is "manager_reject"
    And label "reject_request" when the answer to "proposal" is "finance_reject"
    And label "duplicate_role_vote" when the answer to "proposal" is "security_again"
    And label "requester_cannot_vote" when the answer to "proposal" is "requester_attempt"
    And label "reject_proposal" otherwise

  Scenario: await_mfs_mf
    Given a choice question "proposal" asks "Large-band review with manager and finance already approved: only security is missing. That approval is the final quorum vote."
      | answer            | meaning |
      | security_approve  | security approves; quorum is met |
      | security_reject   | security rejects the request |
      | manager_again     | the manager votes a second time |
      | finance_again     | finance votes a second time |
      | requester_attempt | the requester tries to approve their own request |
    And the answer comes from the field "ev_await_mfs_mf"
    When the answer "security_approve" goes to "approved"
    And the answer "security_reject" goes to "rejected"
    And any other answer goes to "STOP"
    Then label "approve_request" when the answer to "proposal" is "security_approve"
    And label "reject_request" when the answer to "proposal" is "security_reject"
    And label "duplicate_role_vote" when the answer to "proposal" is "manager_again"
    And label "duplicate_role_vote" when the answer to "proposal" is "finance_again"
    And label "requester_cannot_vote" when the answer to "proposal" is "requester_attempt"
    And label "reject_proposal" otherwise

  Scenario: await_mfs_ms
    Given a choice question "proposal" asks "Large-band review with manager and security already approved: only finance is missing. That approval is the final quorum vote."
      | answer            | meaning |
      | finance_approve   | finance approves; quorum is met |
      | finance_reject    | finance rejects the request |
      | manager_again     | the manager votes a second time |
      | security_again    | security votes a second time |
      | requester_attempt | the requester tries to approve their own request |
    And the answer comes from the field "ev_await_mfs_ms"
    When the answer "finance_approve" goes to "approved"
    And the answer "finance_reject" goes to "rejected"
    And any other answer goes to "STOP"
    Then label "approve_request" when the answer to "proposal" is "finance_approve"
    And label "reject_request" when the answer to "proposal" is "finance_reject"
    And label "duplicate_role_vote" when the answer to "proposal" is "manager_again"
    And label "duplicate_role_vote" when the answer to "proposal" is "security_again"
    And label "requester_cannot_vote" when the answer to "proposal" is "requester_attempt"
    And label "reject_proposal" otherwise

  Scenario: await_mfs_fs
    Given a choice question "proposal" asks "Large-band review with finance and security already approved: only the manager is missing. That approval is the final quorum vote."
      | answer            | meaning |
      | manager_approve   | the manager approves; quorum is met |
      | manager_reject    | the manager rejects the request |
      | finance_again     | finance votes a second time |
      | security_again    | security votes a second time |
      | requester_attempt | the requester tries to approve their own request |
    And the answer comes from the field "ev_await_mfs_fs"
    When the answer "manager_approve" goes to "approved"
    And the answer "manager_reject" goes to "rejected"
    And any other answer goes to "STOP"
    Then label "approve_request" when the answer to "proposal" is "manager_approve"
    And label "reject_request" when the answer to "proposal" is "manager_reject"
    And label "duplicate_role_vote" when the answer to "proposal" is "finance_again"
    And label "duplicate_role_vote" when the answer to "proposal" is "security_again"
    And label "requester_cannot_vote" when the answer to "proposal" is "requester_attempt"
    And label "reject_proposal" otherwise

  Scenario: approved
    Given a choice question "proposal" asks "The request is approved, a terminal state. A fresh proposal here is refused and the state does not change; the accept path enters with noop."
      | answer | meaning |
      | cancel | the requester tries to cancel an approved request |
      | noop   | the accept path enters here; no further action |
    And the answer comes from the field "ev_approved"
    When any other answer goes to "STOP"
    Then keep the answer to "proposal"

  Scenario: rejected
    Given a choice question "proposal" asks "The request was rejected, a terminal state. The decision and reason were already recorded by the voting node."
      | answer | meaning |
      | noop   | no further action |
    And the answer comes from the field "ev_rejected"
    When any other answer goes to "STOP"
    Then keep the answer to "proposal"

  Scenario: cancelled
    Given a choice question "proposal" asks "The request was cancelled by the requester, a terminal state. The decision was already recorded by the submitting node."
      | answer | meaning |
      | noop   | no further action |
    And the answer comes from the field "ev_cancelled"
    When any other answer goes to "STOP"
    Then keep the answer to "proposal"
