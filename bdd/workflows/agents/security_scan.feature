@decision
Feature: agents_security_scan
  Port of security_scan: converted from dsl/agents/security_scan.json (BDD round-trip).

  Background:
    Given the workflow runs at most 3 iterations
  Scenario: scan
    Given a choice question "critical_pattern_hit" asks "Does the file contain any CRITICAL-severity pattern (curl_to_shell / eval_base64 / silent_exfil / reverse_shell / suspicious_network)?"
      | answer | meaning |
      | A      | no critical pattern |
      | B      | yes, critical pattern found |
    And answer "B" when the text matches "curl.*\|.*sh", "eval.*base64", ">\s*/dev/tcp", "bash\s+-i\s+>&\s*/dev/tcp", "nc\s+-l\s+-p" with hit 0.95 and miss 0.05
    And a choice question "high_pattern_hit" asks "Does the file contain any HIGH-severity pattern (keylogger / crypto_miner / rm_dash_rf / data_exfil_ssh)?"
      | answer | meaning |
      | A      | no high pattern |
      | B      | yes, high pattern found |
    And answer "B" when the text matches "read.*-s.*pass", "cpuminer", "rm\s+-rf\s+/", "ssh\s+-R" with hit 0.9 and miss 0.1
    And a choice question "file_whitelisted" asks "Is the file on the documented educational/reference whitelist?"
      | answer | meaning |
      | A      | not whitelisted |
      | B      | yes, whitelisted |
    And answer "B" when the text contains "whitelisted", "whitelist_hit" with hit 0.9 and miss 0.1
    And a choice question "binary_file" asks "Is the file binary (non-text, so regex scan does not apply)?"
      | answer | meaning |
      | A      | not binary |
      | B      | yes, binary file |
    And answer "B" when the text contains "binary_file", "non_text_binary" with hit 0.9 and miss 0.1
    When any other answer goes to "STOP"
    Then label "CLEAN" when the answer to "file_whitelisted" is "B"
    And label "SKIP" when the answer to "binary_file" is "B"
    And label "QUARANTINE_CRITICAL" when the answer to "critical_pattern_hit" is "B"
    And label "QUARANTINE_HIGH" when the answer to "high_pattern_hit" is "B"
    And label "CLEAN" otherwise
    And keep the state key "text"
    And keep the state key "file"
    And keep the state key "scan_dir"
