@decision
Feature: ticket_structuring
  Port of ticket_structuring: converted from dsl/capabilities/ticket_structuring.json (BDD round-trip).

  Background:
    Given the workflow runs at most 12 iterations
    Given the policy
      | allow_paths | "${env.LAYA_STORE_DIR}" |
      | max_timeout_ms | 5000 |
      | max_output | 65536 |

  Scenario: normalise
    Given a choice question "well_formed" asks "Is this ticket well-formed enough to route automatically, or does it need a human to clean it up first?"
      | answer | meaning |
      | A | well-formed: it has the fields and structure needed for automatic routing |
      | B | malformed: missing fields, broken structure, or unclear — needs human cleanup |
    When any other answer goes to "STOP"
    Then call the capability "fingerprint"
      | with.text | ${state.text} |
      | project.sha256 | /hex |
      | project.estimated_tokens | /tok/estimated_tokens |
      | project.mem_used_pct | /sys/mem/used_pct |
      | chain.tok.capability | budget |
      | chain.tok.with.text | ${state.text} |
      | chain.sys.capability | metrics |
    Then allow up to 1 retries
