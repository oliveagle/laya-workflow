@decision
Feature: file_upload_scanner
  Guard for LLM-proposed file uploads as a single triage node. The model proposes
  one typed upload (request_id + a non-empty files list of {filename, mime_type,
  size_bytes} + destination + overwrite_policy); this workflow owns schema
  validation, the file-count cap, the total-size and per-file-size caps, the
  blocked-extension set, the overwrite escalation and the approval escalation. Port
  of code/qwen_vm/app_scenarios/file_upload_scanner.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required fields, request_id grammar, non-empty strings,
    positive size_bytes, valid overwrite_policy enum, dangerous-token rejection)
    fold into the `proposal` choice.
  - The local policy table (max_files, max_file_size_bytes, max_total_size_bytes,
    blocked_extensions, require_approval) is a fixed fixture; each deterministic
    check folds into one verdict choice.

  Emitted labels: reject_proposal, reject_too_many_files, reject_total_size,
  reject_file_size, reject_blocked_extension, escalate_overwrite, escalate_approval,
  approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, files:[{filename, mime_type, size_bytes}], destination, overwrite_policy} object (request_id grammar, non-empty strings, positive size_bytes, overwrite_policy deny/rename/overwrite, no dangerous tokens)?"
      | answer    | meaning                                                                 |
      | accepted  | exactly one schema-valid upload                                         |
      | malformed | parse error, missing field, bad request_id, bad size or dangerous token |
    And the answer comes from the field "ev_proposal"
    And a choice question "file_count" asks "Is the number of files within the policy max_files cap?"
      | answer   | meaning                 |
      | ok       | file_count <= max_files |
      | too_many | file_count > max_files  |
    And the answer comes from the field "ev_file_count"
    And a choice question "total_size" asks "Is the summed size_bytes within the policy max_total_size_bytes?"
      | answer | meaning                       |
      | ok     | total <= max_total_size_bytes |
      | over   | total > max_total_size_bytes  |
    And the answer comes from the field "ev_total_size"
    And a choice question "file_size" asks "Is every file size_bytes within the policy max_file_size_bytes?"
      | answer | meaning                                 |
      | ok     | every size <= max_file_size_bytes       |
      | over   | at least one size > max_file_size_bytes |
    And the answer comes from the field "ev_file_size"
    And a choice question "extension" asks "Is every filename extension outside the policy blocked_extensions set?"
      | answer  | meaning                                   |
      | ok      | no file has a blocked extension           |
      | blocked | at least one file has a blocked extension |
    And the answer comes from the field "ev_extension"
    And a choice question "overwrite" asks "Does the upload use a policy-safe overwrite_policy (not overwrite)?"
      | answer    | meaning                                 |
      | safe      | deny or rename, no overwrite escalation |
      | overwrite | overwrite policy requires approval      |
    And the answer comes from the field "ev_overwrite"
    And a choice question "approval" asks "Does the local policy require human approval for this upload?"
      | answer       | meaning                         |
      | not_required | no approval flag set, proceed   |
      | required     | policy require_approval is true |
    And the answer comes from the field "ev_approval"
    When any other answer goes to "STOP"
    Then the primary question is "approval"
    And the action reads the question "approval"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_too_many_files" when the answer to "file_count" is "too_many"
    And label "reject_total_size" when the answer to "total_size" is "over"
    And label "reject_file_size" when the answer to "file_size" is "over"
    And label "reject_blocked_extension" when the answer to "extension" is "blocked"
    And label "escalate_overwrite" when the answer to "overwrite" is "overwrite"
    And label "escalate_approval" when the answer to "approval" is "required"
    And label "approve" otherwise
    And keep the state key "request_id"
