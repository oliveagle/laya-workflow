@decision
Feature: agent_permission_gate

  Port of the ole-eval self_evolving permission workflow (workflows/agent_permission.py):
  a deterministic permission state machine unauthenticated -> authenticated ->
  authorized -> blocked, driven by typed events, plus a calibrated-risk verdict
  (BLOCK / ESCALATE / EXECUTE). The model only proposes the event and its risk
  band; the state machine and the verdict thresholds are deterministic. The
  reference walks a fixed-length trace, so the port makes the trace's end an
  explicit `halt` event and each run is a bounded walk.

  Background:
    Given the workflow runs at most 12 iterations

  Scenario: unauthenticated
    Given a choice question "event" asks "No session yet. login authenticates; logout is a no-op; authorize, request and violation are refused here."
      | answer    | meaning                                        |
      | login     | present credentials and authenticate           |
      | authorize | request a scope -- refused until authenticated |
      | request   | perform a scoped action -- refused             |
      | violation | break the permission rules                     |
      | logout    | end the session                                |
      | halt      | the trace ends here; stop the walk             |
    And the answer comes from the field "ev_unauthenticated"
    And a choice question "risk" asks "Calibrated risk of the proposed action: low below 0.4, uncertain 0.4 up to 0.5, high at or above 0.5?"
      | answer    | meaning                                         |
      | low       | calibrated risk below 0.4                       |
      | uncertain | calibrated risk 0.4 up to 0.5, confidence < 0.6 |
      | high      | calibrated risk at or above 0.5                 |
    And the answer comes from the field "ev_risk"
    When the answer "login" goes to "authenticated"
    And the answer "logout" goes to "unauthenticated"
    And the answer "authorize" goes to "refused"
    And the answer "request" goes to "refused"
    And the answer "violation" goes to "blocked"
    And any other answer goes to "STOP"
    Then the primary question is "event"
    Then label "BLOCK" when the answer to "risk" is "high"
    And label "ESCALATE" when the answer to "risk" is "uncertain"
    And label "EXECUTE" otherwise

  Scenario: authenticated
    Given a choice question "event" asks "Authenticated but unscoped. authorize grants a scope; login re-authenticates; request is refused until authorized."
      | answer    | meaning                                        |
      | login     | present credentials and authenticate           |
      | authorize | request a scope -- granted here                |
      | request   | perform a scoped action -- refused             |
      | violation | break the permission rules                     |
      | logout    | end the session                                |
      | halt      | the trace ends here; stop the walk             |
    And the answer comes from the field "ev_authenticated"
    And a choice question "risk" asks "Calibrated risk of the proposed action: low below 0.4, uncertain 0.4 up to 0.5, high at or above 0.5?"
      | answer    | meaning                                         |
      | low       | calibrated risk below 0.4                       |
      | uncertain | calibrated risk 0.4 up to 0.5, confidence < 0.6 |
      | high      | calibrated risk at or above 0.5                 |
    And the answer comes from the field "ev_risk"
    When the answer "authorize" goes to "authorized"
    And the answer "login" goes to "authenticated"
    And the answer "logout" goes to "unauthenticated"
    And the answer "request" goes to "refused"
    And the answer "violation" goes to "blocked"
    And any other answer goes to "STOP"
    Then the primary question is "event"
    Then label "BLOCK" when the answer to "risk" is "high"
    And label "ESCALATE" when the answer to "risk" is "uncertain"
    And label "EXECUTE" otherwise

  Scenario: authorized
    Given a choice question "event" asks "Authorized with a scope. request is allowed; authorize again is refused; login re-authenticates back to authenticated."
      | answer    | meaning                                        |
      | login     | present credentials and authenticate           |
      | authorize | request a scope -- refused, already authorized |
      | request   | perform a scoped action -- allowed             |
      | violation | break the permission rules                     |
      | logout    | end the session                                |
      | halt      | the trace ends here; stop the walk             |
    And the answer comes from the field "ev_authorized"
    And a choice question "risk" asks "Calibrated risk of the proposed action: low below 0.4, uncertain 0.4 up to 0.5, high at or above 0.5?"
      | answer    | meaning                                         |
      | low       | calibrated risk below 0.4                       |
      | uncertain | calibrated risk 0.4 up to 0.5, confidence < 0.6 |
      | high      | calibrated risk at or above 0.5                 |
    And the answer comes from the field "ev_risk"
    When the answer "request" goes to "authorized"
    And the answer "login" goes to "authenticated"
    And the answer "logout" goes to "unauthenticated"
    And the answer "authorize" goes to "refused"
    And the answer "violation" goes to "blocked"
    And any other answer goes to "STOP"
    Then the primary question is "event"
    Then label "BLOCK" when the answer to "risk" is "high"
    And label "ESCALATE" when the answer to "risk" is "uncertain"
    And label "EXECUTE" otherwise

  Scenario: blocked
    Given a choice question "event" asks "Blocked by a violation. The state machine never leaves blocked, so every event is refused."
      | answer    | meaning                            |
      | login     | present credentials and authenticate |
      | authorize | request a scope                    |
      | request   | perform a scoped action            |
      | violation | break the permission rules         |
      | logout    | end the session                    |
      | halt      | the trace ends here; stop the walk |
    And the answer comes from the field "ev_blocked"
    When the answer "login" goes to "refused"
    And the answer "authorize" goes to "refused"
    And the answer "request" goes to "refused"
    And the answer "violation" goes to "blocked"
    And the answer "logout" goes to "refused"
    And any other answer goes to "STOP"
    Then label "BLOCK" when the answer to "event" is "login"
    And label "BLOCK" otherwise

  Scenario: refused
    Given a choice question "risk" asks "The permission state machine refused this event, so the action is blocked whatever the calibrated risk."
      | answer    | meaning                                         |
      | low       | calibrated risk below 0.4                       |
      | uncertain | calibrated risk 0.4 up to 0.5, confidence < 0.6 |
      | high      | calibrated risk at or above 0.5                 |
    And the answer comes from the field "ev_risk"
    When any other answer goes to "STOP"
    Then label "BLOCK" when the answer to "risk" is "high"
    And label "BLOCK" when the answer to "risk" is "uncertain"
    And label "BLOCK" otherwise
