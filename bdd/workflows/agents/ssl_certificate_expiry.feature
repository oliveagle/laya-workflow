@decision
Feature: agents_ssl_certificate_expiry
  Port of ssl_certificate_expiry: converted from dsl/agents/ssl_certificate_expiry.json (BDD round-trip).

  Scenario: check
    Given a choice question "cert_readable" asks "Could the probe retrieve the certificate info?"
      | answer | meaning |
      | A      | yes, readable |
      | B      | no, unreadable/missing |
    And answer "B" when the field "cert_status" contains "unreadable", "missing", "fetch_failed" with hit 0.95 and miss 0.05
    And a choice question "renew_forced" asks "Was renewal requested with --force?"
      | answer | meaning |
      | A      | no force |
      | B      | yes, forced |
    And answer "B" when the field "force_mode" contains "force", "forced" with hit 0.9 and miss 0.1
    And a choice question "cert_expired" asks "Is the certificate already expired (days_left < 0)?"
      | answer | meaning |
      | A      | not expired |
      | B      | already expired |
    And answer "B" when the field "days_left" matches "^-" with hit 0.95 and miss 0.05
    And a choice question "cert_below_threshold" asks "Is days_left below the renewal threshold (0 <= days_left < 30)?"
      | answer | meaning |
      | A      | at/above threshold |
      | B      | below threshold |
    And answer "B" when the field "days_left" matches "^([0-9]|[12][0-9])$" with hit 0.95 and miss 0.05
    When any other answer goes to "STOP"
    Then label "FAIL_UNREADABLE" when the answer to "cert_readable" is "B"
    And label "RENEW_FORCED" when the answer to "renew_forced" is "B"
    And label "EXPIRED" when the answer to "cert_expired" is "B"
    And label "WARN_RENEW" when the answer to "cert_below_threshold" is "B"
    And label "OK" otherwise
    And keep the state key "cert_status"
    And keep the state key "days_left"
    And keep the state key "force_mode"
    And keep the state key "domain"
