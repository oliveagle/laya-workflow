@decision
Feature: api_key_rotation_guard
  Guard for LLM-proposed API key lifecycle actions as a single triage node. The
  model proposes one typed action (request_id, key_id, action
  rotate/revoke/extend, age_days, new_key_length, extend_days, reason); this
  workflow owns the key_id format, the age hard limit, the new-key length, the
  early-rotation rule and the extension bound. Port of
  code/qwen_vm/app_scenarios/api_key_rotation_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (dangerous tokens, JSON object, required fields,
    action enum) fold into the `proposal` choice.
  - The policy table (max_age_days / recommended_age_days /
    force_early_rotation) is a fixed fixture; each check folds into one verdict
    choice. The Python engine accumulates every error; the port expresses
    «first failing check» in the Python source order (key_id, age, rotate
    length, early rotation, extension), documented here. revoke carries no
    extra check and therefore approves whenever the schema is valid.

  Emitted labels: reject_proposal, reject_key_id, reject_age,
  reject_new_key_length, reject_early_rotation, reject_extension, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, key_id, action, reason} object (no dangerous tokens, all required fields present, action in rotate/revoke/extend)?"
      | answer    | meaning                                            |
      | accepted  | exactly one schema-valid action                    |
      | malformed | dangerous token, bad JSON, missing field or bad action |
    And the answer comes from the field "ev_proposal"
    And a choice question "key_id" asks "Does key_id match the key-[A-Za-z0-9]{8,64} format?"
      | answer  | meaning                  |
      | ok      | key_id format is valid   |
      | invalid | key_id format is invalid |
    And the answer comes from the field "ev_key_id"
    And a choice question "age" asks "Is the key age within the policy hard limit (age_days <= max_age_days)?"
      | answer | meaning                    |
      | ok     | age is within hard limit    |
      | over   | age exceeds hard limit      |
    And the answer comes from the field "ev_age"
    And a choice question "new_key" asks "For a rotate action, is the new key length at least 32 characters? (not_rotate when the action is not rotate)"
      | answer     | meaning                          |
      | not_rotate | action is not rotate             |
      | ok         | new key length meets the minimum |
      | too_short  | new key length is below minimum  |
    And the answer comes from the field "ev_new_key"
    And a choice question "early" asks "For a rotate action, is the rotation allowed (key age >= recommended, or force_early_rotation set)? (not_rotate when the action is not rotate)"
      | answer     | meaning                                     |
      | not_rotate | action is not rotate                        |
      | allowed    | rotation timing is allowed                  |
      | too_early  | rotation is earlier than recommended         |
    And the answer comes from the field "ev_early"
    And a choice question "extension" asks "For an extend action, does extend_days stay within the remaining hard limit (extend_days <= max_age_days - age_days)? (not_extend when the action is not extend)"
      | answer     | meaning                                |
      | not_extend | action is not extend                   |
      | ok         | extension stays within the hard limit  |
      | too_long   | extension would exceed the hard limit  |
    And the answer comes from the field "ev_extension"
    When any other answer goes to "STOP"
    Then the primary question is "age"
    And the action reads the question "age"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_key_id" when the answer to "key_id" is "invalid"
    And label "reject_age" when the answer to "age" is "over"
    And label "reject_new_key_length" when the answer to "new_key" is "too_short"
    And label "reject_early_rotation" when the answer to "early" is "too_early"
    And label "reject_extension" when the answer to "extension" is "too_long"
    And label "approve" otherwise
    And keep the state key "request_id"
