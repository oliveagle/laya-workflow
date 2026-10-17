@decision
Feature: dialogue_policy

  Background:
    Given the workflow runs at most 12 iterations

  Scenario: greeting
    Given a choice question "intent" asks "What is the user's intent at this turn?"
      | answer | meaning              |
      | greet  | open the conversation          |
      | escalate | hand off to a human            |
      | abort  | cancel the session             |
    And the answer comes from the field "i_greet"
    When the answer "greet" goes to "identifying"
    And the answer "escalate" goes to "escalated"
    And the answer "abort" goes to "aborted"
    And any other answer goes to "STOP"
    Then keep the answer to "intent"
    And keep the state key "user_id"
    And keep the state key "reason"

  Scenario: identifying
    Given a choice question "intent" asks "In the identifying state the user must identify or authenticate."
      | answer | meaning              |
      | identify | supply user_id                 |
      | authenticate | supply auth_method + mfa_code  |
      | escalate | hand off                       |
      | abort  | cancel                         |
    And the answer comes from the field "i_identify"
    When the answer "identify" goes to "identifying"
    And the answer "authenticate" goes to "authenticated"
    And the answer "escalate" goes to "escalated"
    And the answer "abort" goes to "aborted"
    And any other answer goes to "STOP"
    Then keep the answer to "intent"

  Scenario: authenticated
    Given a choice question "intent" asks "After authentication, the user must choose an operation to start collection for."
      | answer | meaning              |
      | start_collection | declare the operation          |
      | escalate | hand off                       |
      | abort  | cancel                         |
    And the answer comes from the field "i_auth"
    When the answer "start_collection" goes to "collecting"
    And the answer "escalate" goes to "escalated"
    And the answer "abort" goes to "aborted"
    And any other answer goes to "STOP"
    Then keep the answer to "intent"

  Scenario: collecting
    Given a choice question "intent" asks "While collecting transfer slots, the user may provide more slots or move to confirm."
      | answer | meaning              |
      | provide_slot | fill in a slot                 |
      | request_confirm | all slots are present, ask to confirm |
      | escalate | hand off                       |
      | abort  | cancel                         |
    And the answer comes from the field "i_coll"
    When the answer "provide_slot" goes to "collecting"
    And the answer "request_confirm" goes to "confirm"
    And the answer "escalate" goes to "escalated"
    And the answer "abort" goes to "aborted"
    And any other answer goes to "STOP"
    Then keep the answer to "intent"

  Scenario: confirm
    Given a choice question "intent" asks "The user is at the final confirmation step. confirm runs the (single) sensitive execute_transfer once."
      | answer | meaning              |
      | confirm | execute the transfer once      |
      | escalate | hand off                       |
      | abort  | cancel                         |
    And the answer comes from the field "i_confirm"
    When the answer "confirm" goes to "done"
    And the answer "escalate" goes to "escalated"
    And the answer "abort" goes to "aborted"
    And any other answer goes to "STOP"
    Then label "execute_transfer" when the answer is "confirm"
    And label "noop" otherwise
    And keep the state key "request_id"
    And keep the state key "user_id"
    And keep the state key "recipient_id"
    And keep the state key "amount"
    And keep the state key "currency"
    And keep the state key "purpose"

  Scenario: done
    Given a choice question "intent" asks "Terminal DONE state accepts no further intents except a duplicate confirm (rejected)."
      | answer | meaning              |
      | noop   | no action                      |
    And the answer is always "noop"
    When any other answer goes to "STOP"
    Then keep the answer to "intent"

  Scenario: escalated
    Given a choice question "intent" asks "Terminal ESCALATED state accepts no further intents."
      | answer | meaning              |
      | noop   | no action                      |
    And the answer is always "noop"
    When any other answer goes to "STOP"
    Then keep the answer to "intent"

  Scenario: aborted
    Given a choice question "intent" asks "Terminal ABORTED state accepts no further intents."
      | answer | meaning              |
      | noop   | no action                      |
    And the answer is always "noop"
    When any other answer goes to "STOP"
    Then keep the answer to "intent"

