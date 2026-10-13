@decision
Feature: alert_noise_reduction
  Alert noise reduction as a single deterministic triage node. The model proposes
  exactly one typed operational context (cause_group, release_correlation,
  business_period); this workflow owns the metric threshold, the trend, the
  dedupe/cooldown windows, the severity composition, and the final
  suppress/escalate decision. Port of code/qwen_vm/app_scenarios/alert_noise_reduction.
  Thresholds are per metric (cpu_percent 85, p95_latency_ms 500, error_rate_percent
  5, disk_used_percent 85, queue_depth 1000, memory_percent 90); breach folds the
  ratio latest/threshold >= 1 into a band. The dedupe (300s) and cooldown (900s)
  windows are clock/fingerprint state and fold into one stream choice. Effective
  severity folds base severity (ratio band) plus the release-correlation and
  business-period modifiers into one choice. The emitted label is the engine's
  action code: escalate is exactly {page, ticket}; suppress is
  {clear, duplicate, cooldown, low_severity, reject_context}.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: triage
    Given a choice question "context_schema" asks "Is the model's typed context one schema-valid proposal (exactly cause_group / release_correlation / business_period, all enum values, no threshold / trend / severity / decision / action field)?"
      | answer   | meaning                                            |
      | accepted | one valid typed context proposal                    |
      | rejected | malformed, unknown enum, or a forbidden local field |
    And the answer comes from the field "ev_context_schema"
    And a choice question "breach" asks "Did the latest sample breach this metric's configured threshold (ratio = latest / threshold >= 1)?"
      | answer   | meaning                |
      | breached | latest >= threshold    |
      | below    | latest < threshold     |
    And the answer comes from the field "ev_breach"
    And a choice question "stream" asks "Fingerprint state for this service+metric+cause_group: has an identical alert already fired?"
      | answer     | meaning                                                        |
      | first_seen | no alert for this fingerprint inside dedupe (300s) or cooldown (900s) |
      | duplicate  | identical alert already fired within the dedupe window         |
      | cooldown   | identical alert already acted on within the cooldown window     |
    And the answer comes from the field "ev_stream"
    And a choice question "trend" asks "Trend of the sample window (least-squares slope compared with 2% of the threshold)?"
      | answer  | meaning                              |
      | rising  | slope above +2% of the threshold      |
      | falling | slope below -2% of the threshold      |
      | stable  | otherwise, or fewer than three samples |
    And the answer comes from the field "ev_trend"
    And a choice question "release_correlation" asks "Model context: does the alert correlate with an active release?"
      | answer         | meaning                          |
      | none           | no release information           |
      | not_correlated | a release is known but unrelated |
      | correlated     | correlates with an active release |
    And the answer comes from the field "ev_release_correlation"
    And a choice question "business_period" asks "Model context: which business period is the alert in?"
      | answer            | meaning                |
      | normal            | ordinary period        |
      | business_peak     | peak traffic period    |
      | off_peak          | low traffic period     |
      | deployment_window | a planned deploy window |
    And the answer comes from the field "ev_business_period"
    And a choice question "severity" asks "Effective severity of the breach: base from the ratio band (warning 1.0<=r<1.2, high 1.2<=r<1.5, critical r>=1.5), raised when release_correlation=correlated and trend=rising, raised when business_period=business_peak and (ratio>=1.2 or trend=rising), lowered when business_period=off_peak and trend=falling and ratio<1.5."
      | answer   | meaning                                        |
      | info     | lowered to informational; suppressed            |
      | warning  | breaches but below the page line; files a ticket |
      | high     | page-worthy                                      |
      | critical | page-worthy, most severe                        |
    And the answer comes from the field "ev_severity"
    When any other answer goes to "STOP"
    Then the primary question is "severity"
    And label "reject_context" when the answer to "context_schema" is "rejected"
    And label "clear" when the answer to "breach" is "below"
    And label "duplicate" when the answer to "stream" is "duplicate"
    And label "cooldown" when the answer to "stream" is "cooldown"
    And label "low_severity" when the answer to "severity" is "info"
    And label "page" when the answer to "severity" is "critical"
    And label "page" when the answer to "severity" is "high"
    And label "ticket" otherwise
    And keep the state key "alert_id"
    And keep the state key "service"
