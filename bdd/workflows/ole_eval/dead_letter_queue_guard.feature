@decision
Feature: dead_letter_queue_guard
  Guard for LLM-proposed dead-letter-queue actions as a single triage node. The
  model proposes one typed action (request_id, messages [{message_id,
  queue_name, error_type, payload_size_bytes}], action retry/discard/forward/
  archive, retry_count); this workflow owns the retry cap, the payload cap, the
  blocked-queue set and the large-forward escalation. Port of
  code/qwen_vm/app_scenarios/dead_letter_queue_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required fields, non-negative retry_count, non-empty
    messages with positive payload_size_bytes, action enum, dangerous-token
    rejection) fold into the `proposal` choice.
  - The DLQ policy (max_retry_count, max_payload_size_bytes, blocked_queues,
    require_approval_above_bytes) is a fixed fixture; each ordered check folds
    into one verdict choice.

  Emitted labels: reject_proposal, reject_retry, reject_payload,
  reject_queue, escalate_forward, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, messages [{message_id, queue_name, error_type, payload_size_bytes}], action, retry_count} object (valid action enum, non-empty messages, positive payload sizes, no dangerous tokens)?"
      | answer    | meaning                                                              |
      | accepted  | exactly one schema-valid DLQ action                                   |
      | malformed | parse error, missing field, bad action, bad retry_count or dangerous token |
    And the answer comes from the field "ev_proposal"
    And a choice question "retry" asks "Is retry_count within the policy max_retry_count?"
      | answer | meaning                            |
      | ok     | retry_count <= max_retry_count      |
      | over   | retry_count > max_retry_count       |
    And the answer comes from the field "ev_retry"
    And a choice question "payload" asks "Is every message payload within the policy max_payload_size_bytes?"
      | answer | meaning                                        |
      | ok     | every payload <= max_payload_size_bytes        |
      | over   | some payload > max_payload_size_bytes          |
    And the answer comes from the field "ev_payload"
    And a choice question "queue" asks "Is every message queue_name outside the policy blocked_queues set?"
      | answer  | meaning                       |
      | ok      | no queue is blocked           |
      | blocked | some queue is blocklisted     |
    And the answer comes from the field "ev_queue"
    And a choice question "forward_approval" asks "For a forward action, is every message payload at or below require_approval_above_bytes? (not_applicable when action is not forward)"
      | answer            | meaning                                    |
      | not_applicable    | action is not forward                       |
      | ok                | forward with all payloads at or below the threshold |
      | requires_approval | forward with a payload above the threshold  |
    And the answer comes from the field "ev_forward_approval"
    When any other answer goes to "STOP"
    Then the primary question is "retry"
    And the action reads the question "retry"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_retry" when the answer to "retry" is "over"
    And label "reject_payload" when the answer to "payload" is "over"
    And label "reject_queue" when the answer to "queue" is "blocked"
    And label "escalate_forward" when the answer to "forward_approval" is "requires_approval"
    And label "approve" otherwise
    And keep the state key "request_id"
