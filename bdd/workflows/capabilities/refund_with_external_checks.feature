@decision
Feature: refund_with_external_checks

  Background:
    Given the workflow runs at most 6 iterations

    Given the policy
      | allow_exec | true |
      | allow_hosts | "127.0.0.1", "localhost" |
      | max_timeout_ms | 5000 |
      | max_output | 65536 |
      | retries | 1 |

    Given the capabilities
      | risk_scorer | {"kind":"http","method":"POST","url":"http://127.0.0.1:8791/score","headers":{"content-type":"application/json"},"body":{"text":"${state.text}"},"timeout_ms":2000} |
      | case_agent | {"kind":"agent","transport":"http","url":"http://127.0.0.1:8791/agent","session":"refund-case","timeout_ms":3000} |
      | local_echo | {"kind":"exec","argv":["/bin/echo","risk-label=${projected_label}"],"timeout_ms":5000,"max_output":4096} |
  Scenario: score_risk
    Given a score question "amount_band" asks "Rough size of the refund request?"
      | level          |
      | tokens / cents                 |
      | single invoice                 |
      | large or recurring             |
    When any other answer goes to "assess"
    Then call the capability "risk_scorer"
      | with.text      | ${state.text} |
      | project.projected_label  | /label |
      | project.projected_risk  | /risk |

  Scenario: assess
    Given a choice question "decision" asks "Based on the external risk label and the amount band, what should happen?"
      | answer | meaning              |
      | approve | low risk and small/medium amount — approve automatically |
      | review | needs a human reviewer         |
      | reject | high risk — decline the refund |
    And answer by rules on the field "text" with hit 0.9 and fallback "approve"
      | label | needles |
      | reject | "fraud", "scripted", "bulk", "urgent" |
    When any other answer goes to "STOP"
    Then label "REJECTED" when the answer is "reject"
    And label "NEEDS_REVIEW" when the answer is "review"
    And label "APPROVED" otherwise

