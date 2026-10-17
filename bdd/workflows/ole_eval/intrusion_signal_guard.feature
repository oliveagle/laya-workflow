@decision
Feature: intrusion_signal_guard

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: signals
    Given a choice question "known_signature" asks "Does the request match a known signature (sql_injection / path_traversal / log4j_injection / shellshock, or sqlmap/nikto/masscan/zgrab user-agent)?"
      | answer | meaning              |
      | A      | no signature match             |
      | B      | yes, known signature           |
    And answer "B" when the text contains "sql_injection", "union select", " or 1=1", "sleep(", "../", "/etc/passwd", "${jndi:", "() { :;", "sqlmap", "nikto", "masscan", "zgrab" with hit 0.95 and miss 0.05
    And a choice question "failed_login_velocity" asks "Strong signal: >=5 failed logins in the last 600s?"
      | answer | meaning              |
      | A      | below threshold                |
      | B      | yes, failed-login velocity     |
    And answer "B" when the text contains "failed_login_velocity", "failed login velocity", "5 failed", "repeated_failures" with hit 0.9 and miss 0.1
    And a choice question "port_scan_window" asks "Strong signal: >=20 unique ports probed in the last 60s?"
      | answer | meaning              |
      | A      | below threshold                |
      | B      | yes, port scan                 |
    And answer "B" when the text contains "port_scan", "port scan", "unique_ports", "masscan", "zgrab" with hit 0.9 and miss 0.1
    And a choice question "impossible_travel" asks "Strong signal: country changed with 0 <= delta <= 1800s?"
      | answer | meaning              |
      | A      | no impossible travel           |
      | B      | yes, impossible travel         |
    And answer "B" when the text contains "impossible_travel", "impossible travel", "fast travel" with hit 0.9 and miss 0.1
    And a choice question "unusual_country" asks "Weak signal: current country outside the user's usual countries?"
      | answer | meaning              |
      | A      | usual country                  |
      | B      | yes, unusual country           |
    And answer "B" when the text contains "unusual_country", "unusual country", "unexpected country" with hit 0.8 and miss 0.2
    And a choice question "off_hours_access" asks "Weak signal: event clock hour outside 07:00-20:00?"
      | answer | meaning              |
      | A      | within hours                   |
      | B      | yes, off-hours access          |
    And answer "B" when the text contains "off_hours", "off hours", "night access" with hit 0.8 and miss 0.2
    And a choice question "intrusion_verdict" asks "Which verdict does the signal combination imply? block: signature OR strong>=2 OR (strong>=1 AND weak>=2). challenge: strong==1 OR weak>=2. monitor: weak==1. allow: none."
      | answer | meaning              |
      | allow  | no signals; action=none        |
      | monitor | exactly one weak signal; action=watch |
      | challenge | one strong, or two weak, or strong+weak<2; action=step_up_mfa |
      | block  | signature, or >=2 strong, or >=1 strong and >=2 weak; action=quarantine_proposal |
    And answer by all-group rules on the text with hit 0.9 and miss 0.02 and fallback "allow" at 0.9
      | label | all of |
      | block | "sql_injection" |
      | block | "union select" |
      | block | " or 1=1" |
      | block | "sleep(" |
      | block | "../" |
      | block | "/etc/passwd" |
      | block | "${jndi:" |
      | block | "() { :;" |
      | block | "sqlmap" |
      | block | "nikto" |
      | block | "failed_login_velocity", "failed login velocity" | "port_scan", "port scan", "masscan", "zgrab" |
      | block | "failed_login_velocity", "failed login velocity" | "impossible_travel", "impossible travel" |
      | block | "port_scan", "port scan", "masscan", "zgrab" | "impossible_travel", "impossible travel" |
      | block | "failed_login_velocity", "failed login velocity" | "unusual_country", "unusual country" | "off_hours", "off hours" |
      | block | "port_scan", "port scan", "masscan", "zgrab" | "unusual_country", "unusual country" | "off_hours", "off hours" |
      | block | "impossible_travel", "impossible travel" | "unusual_country", "unusual country" | "off_hours", "off hours" |
      | challenge | "failed_login_velocity", "failed login velocity" |
      | challenge | "port_scan", "port scan", "masscan", "zgrab" |
      | challenge | "impossible_travel", "impossible travel" |
      | challenge | "unusual_country", "unusual country" | "off_hours", "off hours" |
      | monitor | "unusual_country", "unusual country" |
      | monitor | "off_hours", "off hours" |
    When any other answer goes to "STOP"
    Then label "block" when the answer to "known_signature" is "B"
    Then label "block" when the answer to "intrusion_verdict" is "block"
    Then label "challenge" when the answer to "intrusion_verdict" is "challenge"
    Then label "monitor" when the answer to "intrusion_verdict" is "monitor"
    Then label "allow" when the answer to "intrusion_verdict" is "allow"
    And label "allow" otherwise
    Then the primary question is "intrusion_verdict"

