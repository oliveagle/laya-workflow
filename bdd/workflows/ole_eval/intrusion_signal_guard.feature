@decision
Feature: intrusion_signal_guard

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: signals
    Given a choice question "known_signature" asks "Does the request match a known signature (sql_injection / path_traversal / log4j_injection / shellshock, or sqlmap/nikto/masscan/zgrab user-agent)?"
      | answer | meaning              |
      | A      | no signature match             |
      | B      | yes, known signature           |
    And a choice question "failed_login_velocity" asks "Strong signal: >=5 failed logins in the last 600s?"
      | answer | meaning              |
      | A      | below threshold                |
      | B      | yes, failed-login velocity     |
    And a choice question "port_scan_window" asks "Strong signal: >=20 unique ports probed in the last 60s?"
      | answer | meaning              |
      | A      | below threshold                |
      | B      | yes, port scan                 |
    And a choice question "impossible_travel" asks "Strong signal: country changed with 0 <= delta <= 1800s?"
      | answer | meaning              |
      | A      | no impossible travel           |
      | B      | yes, impossible travel         |
    And a choice question "unusual_country" asks "Weak signal: current country outside the user's usual countries?"
      | answer | meaning              |
      | A      | usual country                  |
      | B      | yes, unusual country           |
    And a choice question "off_hours_access" asks "Weak signal: event clock hour outside 07:00-20:00?"
      | answer | meaning              |
      | A      | within hours                   |
      | B      | yes, off-hours access          |
    And a choice question "intrusion_verdict" asks "Which verdict does the signal combination imply? block: signature OR strong>=2 OR (strong>=1 AND weak>=2). challenge: strong==1 OR weak>=2. monitor: weak==1. allow: none."
      | answer | meaning              |
      | allow  | no signals; action=none        |
      | monitor | exactly one weak signal; action=watch |
      | challenge | one strong, or two weak, or strong+weak<2; action=step_up_mfa |
      | block  | signature, or >=2 strong, or >=1 strong and >=2 weak; action=quarantine_proposal |
    When any other answer goes to "STOP"
    Then label "block" when the answer to "known_signature" is "B"
    Then label "block" when the answer to "intrusion_verdict" is "block"
    Then label "challenge" when the answer to "intrusion_verdict" is "challenge"
    Then label "monitor" when the answer to "intrusion_verdict" is "monitor"
    Then label "allow" when the answer to "intrusion_verdict" is "allow"
    And label "allow" otherwise
    Then the primary question is "intrusion_verdict"
    And keep the state key "request_id"
    And keep the state key "event_id"

