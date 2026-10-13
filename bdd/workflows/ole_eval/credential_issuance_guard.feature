@decision
Feature: credential_issuance_guard
  Guard for LLM-proposed credential-issuance requests as a single triage node.
  The model proposes one typed request (request_id, subject_id, credential_kind,
  operation, requested_audience, device_binding_requested) and *only* that; this
  workflow owns subject/policy resolution, the proofing / assurance / MFA /
  device / risk / revocation / rotation checks and the credential preview. It
  never contacts an identity provider or issues a real credential. Port of
  code/qwen_vm/app_scenarios/credential_issuance_guard.

  Second three-valued port after incident_response_guard, with its own verbs:
  the engine returns ISSUE / MODIFY / REJECT, so the ladder is `reject_*` (hard)
  then `modify_*` (soft, a narrower credential is issued) then `issue`.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (exact six-field shape, identifier grammars, operation
    /audience whitelists, boolean device_binding_requested) fold into the
    `proposal` choice.
  - The subject registry and every CredentialPolicy field (allowed subject
    types, minimum IAL, required proofing steps, minimum MFA strength,
    phishing-resistant / trusted-device / device-binding requirements, audience
    scopes, lifetime, rotation window, rotation interval) are a fixed fixture:
    the harness resolves them and hands the DSL one answer per check.
  - The credential preview re-checks proofing steps and identity-assurance level
    with the *same* policy+subject inputs already evaluated above, so when the
    ladder reaches the preview those two checks are provably empty; they fold
    into the `proofing` and `ial` choices rather than becoming new labels.

  Emitted labels: reject_proposal, reject_duplicate, reject_unknown_subject,
  reject_unknown_kind, reject_subject_type, reject_audience, reject_ial,
  reject_proofing, reject_mfa, reject_phishing, reject_trusted_device,
  reject_device_binding, reject_risk, reject_subject_inactive,
  reject_credential_inactive, reject_rotation_early, modify_rotation_required,
  modify_device_binding_required, modify_risk_lifetime, modify_risk_scope,
  modify_step_up, issue.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, subject_id, credential_kind, operation, requested_audience, device_binding_requested} object with valid identifiers and a boolean flag?"
      | answer    | meaning                                                          |
      | accepted  | exactly one structurally valid typed request                     |
      | malformed | parse error, missing/extra field, bad identifier or bad flag     |
    And the answer comes from the field "ev_proposal"
    And a choice question "duplicate" asks "Is the request_id unseen in the in-memory request ledger?"
      | answer    | meaning                          |
      | fresh     | request_id is new                |
      | duplicate | request_id already evaluated     |
    And the answer comes from the field "ev_duplicate"
    And a choice question "subject" asks "Is the subject_id present in the subject registry?"
      | answer  | meaning          |
      | known   | subject resolved |
      | unknown | no such subject  |
    And the answer comes from the field "ev_subject"
    And a choice question "kind" asks "Is the credential_kind present in the policy registry?"
      | answer  | meaning         |
      | known   | policy resolved |
      | unknown | no such policy  |
    And the answer comes from the field "ev_kind"
    And a choice question "subject_type" asks "Is the subject type allowed by the resolved policy?"
      | answer      | meaning                    |
      | allowed     | type is allowed            |
      | not_allowed | type is not allowed        |
    And the answer comes from the field "ev_subject_type"
    And a choice question "audience" asks "Is the requested audience within the policy's audience scopes (and workload-only for production_deploy)?"
      | answer      | meaning                    |
      | allowed     | audience is allowed        |
      | not_allowed | audience is not allowed    |
    And the answer comes from the field "ev_audience"
    And a choice question "ial" asks "Does the subject's identity assurance level meet the policy minimum?"
      | answer       | meaning                     |
      | ok           | level >= minimum IAL        |
      | insufficient | level < minimum IAL         |
    And the answer comes from the field "ev_ial"
    And a choice question "proofing" asks "Are all required proofing steps satisfied for the subject type?"
      | answer  | meaning                     |
      | ok      | no required step is missing |
      | missing | a required step is missing  |
    And the answer comes from the field "ev_proofing"
    And a choice question "mfa" asks "Does the subject's MFA strength meet the policy minimum?"
      | answer       | meaning                       |
      | ok           | strength >= minimum MFA       |
      | insufficient | strength < minimum MFA        |
    And the answer comes from the field "ev_mfa"
    And a choice question "phishing" asks "When the policy demands phishing-resistant MFA, does the subject hold a phishing-resistant authenticator?"
      | answer   | meaning                                  |
      | ok       | not required, or FIDO2/attestation held  |
      | required | phishing-resistant MFA is required       |
    And the answer comes from the field "ev_phishing"
    And a choice question "device_trust" asks "When the policy demands a trusted device, is the subject's device trusted?"
      | answer   | meaning                              |
      | ok       | not required, or device is trusted    |
      | required | a trusted device is required          |
    And the answer comes from the field "ev_device_trust"
    And a choice question "device_binding" asks "When device binding is requested, can the subject's device be bound?"
      | answer      | meaning                              |
      | ok          | not requested, or device is bound     |
      | unavailable | a binding was requested but is impossible |
    And the answer comes from the field "ev_device_binding"
    And a choice question "risk" asks "Is the subject's risk level acceptable for this credential kind?"
      | answer   | meaning                       |
      | ok       | not high; medium only for api_access |
      | too_high | risk level is too high        |
    And the answer comes from the field "ev_risk"
    And a choice question "subject_status" asks "Is the subject active?"
      | answer     | meaning          |
      | active     | subject is active |
      | not_active | subject is not active |
    And the answer comes from the field "ev_subject_status"
    And a choice question "credential_status" asks "Is the subject's current credential active?"
      | answer     | meaning              |
      | active     | credential is active  |
      | not_active | credential is not active |
    And the answer comes from the field "ev_credential_status"
    And a choice question "rotation_early" asks "Is a rotate operation premature against the policy's minimum rotation interval?"
      | answer    | meaning                       |
      | ok        | not premature                 |
      | too_early | age < minimum rotation interval |
    And the answer comes from the field "ev_rotation_early"
    And a choice question "rotation_required" asks "Does an initial request need an implicit rotation because the current credential is over the max age?"
      | answer   | meaning                     |
      | ok       | no implicit rotation        |
      | required | implicit rotation required  |
    And the answer comes from the field "ev_rotation_required"
    And a choice question "device_binding_required" asks "When the policy requires device binding, did the request ask for it?"
      | answer   | meaning                          |
      | ok       | not required, or it was requested |
      | required | policy requires it but it was not requested |
    And the answer comes from the field "ev_device_binding_required"
    And a choice question "risk_lifetime" asks "Does a medium-risk subject need a reduced credential lifetime?"
      | answer | meaning                       |
      | ok     | not medium risk               |
      | reduce | medium risk, lifetime reduced |
    And the answer comes from the field "ev_risk_lifetime"
    And a choice question "risk_scope" asks "Does a medium-risk subject need narrowed scopes below the policy's default?"
      | answer | meaning                          |
      | ok     | not medium, or scopes already api:read-only |
      | reduce | medium risk, scopes narrowed     |
    And the answer comes from the field "ev_risk_scope"
    And a choice question "recovery" asks "Does a recover operation require a phishing-resistant step-up?"
      | answer  | meaning                        |
      | ok      | not a recover operation        |
      | step_up | recovery step-up required      |
    And the answer comes from the field "ev_recovery"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_duplicate" when the answer to "duplicate" is "duplicate"
    And label "reject_unknown_subject" when the answer to "subject" is "unknown"
    And label "reject_unknown_kind" when the answer to "kind" is "unknown"
    And label "reject_subject_type" when the answer to "subject_type" is "not_allowed"
    And label "reject_audience" when the answer to "audience" is "not_allowed"
    And label "reject_ial" when the answer to "ial" is "insufficient"
    And label "reject_proofing" when the answer to "proofing" is "missing"
    And label "reject_mfa" when the answer to "mfa" is "insufficient"
    And label "reject_phishing" when the answer to "phishing" is "required"
    And label "reject_trusted_device" when the answer to "device_trust" is "required"
    And label "reject_device_binding" when the answer to "device_binding" is "unavailable"
    And label "reject_risk" when the answer to "risk" is "too_high"
    And label "reject_subject_inactive" when the answer to "subject_status" is "not_active"
    And label "reject_credential_inactive" when the answer to "credential_status" is "not_active"
    And label "reject_rotation_early" when the answer to "rotation_early" is "too_early"
    And label "modify_rotation_required" when the answer to "rotation_required" is "required"
    And label "modify_device_binding_required" when the answer to "device_binding_required" is "required"
    And label "modify_risk_lifetime" when the answer to "risk_lifetime" is "reduce"
    And label "modify_risk_scope" when the answer to "risk_scope" is "reduce"
    And label "modify_step_up" when the answer to "recovery" is "step_up"
    And label "issue" otherwise
    And keep the state key "request_id"
