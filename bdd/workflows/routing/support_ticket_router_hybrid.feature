@decision
Feature: support_ticket_router_hybrid
  Port of support_ticket_router_hybrid: converted from dsl/routing/support_ticket_router_hybrid.json (BDD round-trip).

  Background:
    Given the workflow runs at most 20 iterations
    Given the policy
      | allow_exec | false |
      | allow_hosts |  |
      | allow_paths | "${env.HOME}" |
      | max_timeout_ms | 20000 |
      | max_output | 65536 |

  Scenario: classify
    Given a choice question "category" asks "Which team should handle this ticket?"
      | answer | meaning |
      | billing | invoices, payments, refunds |
      | technical | bugs, outages, system errors |
      | sales | pricing, new contracts |
      | other | everything else |
    When any other answer goes to "needle_fallback"
    And the answer "technical" goes to "severity_probe"
    And the answer "billing" goes to "severity_probe"
    Then keep the answer to "category"
    Then allow up to 1 retries

  Scenario: needle_fallback
    Given a choice question "cat" asks "Semantic category from the on-device model."
      | answer | meaning |
      | billing | money, refund, invoice, payment |
      | technical | crash, error, broken, bug, outage |
      | shipping | delivery, package, arrived, order status |
      | other | everything else |
    When any other answer goes to "STOP"
    And the answer "technical" goes to "severity_probe"
    And the answer "billing" goes to "severity_probe"
    And the answer "shipping" goes to "severity_probe"
    Then call the capability "cls"
      | with.system | date: 2026-10-07; locale: en-US; this is a customer support inbox. |
      | with.prompt | ${state.text} |
      | with.tools | [{"type":"function","name":"refund_order","description":"Give the customer their money back for a purchase.","triggers":["\\b(refund\u007creturn\u007ccharge.?back)\\b","\\bmoney\\s*back\\b","\\btake\\s*(it\u007cthem)\\s*back\\b","\\b(cancel\u007creverse)\\s+(this\\s+)?(purchase\u007corder\u007ccharge)\\b","\\bcredit\\s+me\\b"],"parameters":{"type":"object","properties":{"reason":{"type":"string"}},"required":[]}},{"type":"function","name":"fix_technical","description":"Help with a broken device, app, or connection.","triggers":["\\b(wifi\u007cinternet\u007cbluetooth\u007capp\u007csoftware)\\b","\\b(crash\u007cfreeze\u007cstutter\u007cdrop\u007cpair\u007csign\\s*in)\\b","\\b(500\u007cerror\u007cbroken\u007cnot\\s*working\u007cwon.t)\\b"],"parameters":{"type":"object","properties":{"issue":{"type":"string"}},"required":[]}},{"type":"function","name":"track_package","description":"Find where a delivery is or change its address.","triggers":["\\b(package\u007cparcel\u007corder\u007cdelivery\u007cdeliver\u007ccourier\u007cshipment\u007ctrack\u007carriv\u007ccustoms)\\b"],"parameters":{"type":"object","properties":{"order_id":{"type":"string"}},"required":[]}}] |
      | project.category | /function_calls/0/name |
    Then allow up to 1 retries

  Scenario: severity_probe
    Given a score question "severity" asks "How severe is the impact?"
      | level |
      | minor |
      | degraded |
      | outage |
    When any other answer goes to "STOP"
    Then label "PAGE_ONCALL" when "severity" is at least 1.5
    And label "QUEUE_TODAY" when "severity" is at least 0.5
    And label "BACKLOG" otherwise
