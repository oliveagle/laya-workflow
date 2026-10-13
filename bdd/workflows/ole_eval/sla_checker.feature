@decision
Feature: sla_checker
  Deterministic UTC SLA evaluation as a single triage node. The model proposes
  typed facts about one {rule, event} proposal; this workflow owns schema
  validation, conflict detection, duplicate detection, SLO state and severity.
  Port of code/qwen_vm/app_scenarios/sla_checker.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (```json fence stripping, duplicate-key rejection,
    "exactly {rule,event}" shape) fold into the single `proposal` choice.
  - UTC business-window minute accumulation (add_business_minutes /
    business_minutes_between: next-opening, day/window overlap, DST-free UTC
    arithmetic) is deterministic clock arithmetic that does not belong in the
    DSL; it is computed locally and surfaces here only as the SLO `sla` and
    `severity` choices. The port judges the resulting state, never recomputes
    a calendar.
  - response_minutes > resolution_minutes and overlapping business windows are
    both `rule` = conflict / overlap; `_MAX_SLO_MINUTES` / `_MAX_TIMELINE_DAYS`
    bounds fold into the same schema bucket.

  Emitted labels: reject_proposal, reject_rule, reject_rule_conflict,
  reject_duplicate_rule, reject_event, reject_duplicate_event, breach_critical,
  breach_high, breach_medium, breach_low, met, pending.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: sla
    Given a choice question "proposal" asks "Does the proposal parse as exactly one object {rule, event} (UTF-8, no duplicate JSON keys, no fence/extra text)?"
      | answer   | meaning                                                              |
      | accepted | parses to exactly {rule, event}, no duplicate keys                    |
      | malformed | parse error, duplicate JSON key, fence/extra text or wrong top shape |
    And the answer comes from the field "ev_proposal"
    And a choice question "rule" asks "Does the rule pass schema validation and registry admission (rule_id format, non-empty priorities with P1..P99 keys, response_minutes <= resolution_minutes, non-empty non-overlapping windows with start < end, id not already registered)?"
      | answer     | meaning                                                     |
      | ok         | rule is schema-valid, conflict-free and new                  |
      | bad_schema | malformed id / empty priorities / bad P-key / bad duration   |
      | conflict   | a priority has response_minutes > resolution_minutes          |
      | overlap    | overlapping windows, empty days, or start >= end              |
      | duplicate  | rule_id already present in the registry                       |
    And the answer comes from the field "ev_rule"
    And a choice question "event" asks "Does the event pass validation (all six keys present, UTC 'Z' timestamps only, responded_at >= created_at, resolved_at >= responded_at and >= created_at, priority key present in the rule, event_id not already evaluated)?"
      | answer           | meaning                                             |
      | ok               | event is schema-valid, time-consistent and new       |
      | bad_schema       | missing/extra key or non-boolean-safe field shape     |
      | bad_time         | non-UTC/DST offset, or timestamps out of order        |
      | unknown_priority | priority is not one of the rule's priority keys       |
      | duplicate        | event_id was already evaluated in this ledger        |
    And the answer comes from the field "ev_event"
    And a choice question "sla" asks "Overall SLO state (after local UTC business-window arithmetic; PENDING when the event has no respond/resolve yet)."
      | answer  | meaning                                     |
      | met     | every applicable deadline was met            |
      | pending | not yet responded and/or not yet resolved    |
      | breach  | at least one deadline was missed             |
    And the answer comes from the field "ev_sla"
    And a choice question "severity" asks "Breach severity from the SLO overrun ratio (max of response/resolution ratios): NONE when no deadline is missed; CRITICAL when the priority is P1 or any overrun ratio >= 2.0; HIGH at >= 1.5; MEDIUM at >= 1.2; else LOW."
      | answer   | meaning                                            |
      | none     | no breach, severity NONE                            |
      | low      | breach with overrun ratio < 1.2                     |
      | medium   | breach with overrun ratio in [1.2, 1.5)             |
      | high     | breach with overrun ratio in [1.5, 2.0)             |
      | critical | P1 breach, or overrun ratio >= 2.0                  |
    And the answer comes from the field "ev_severity"
    When any other answer goes to "STOP"
    Then the primary question is "severity"
    And the action reads the question "severity"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_rule" when the answer to "rule" is "bad_schema"
    And label "reject_rule_conflict" when the answer to "rule" is one of "conflict", "overlap"
    And label "reject_duplicate_rule" when the answer to "rule" is "duplicate"
    And label "reject_event" when the answer to "event" is one of "bad_schema", "bad_time", "unknown_priority"
    And label "reject_duplicate_event" when the answer to "event" is "duplicate"
    And label "breach_critical" when the answer to "severity" is "critical"
    And label "breach_high" when the answer to "severity" is "high"
    And label "breach_medium" when the answer to "severity" is "medium"
    And label "breach_low" when the answer to "severity" is "low"
    And label "met" when the answer to "sla" is "met"
    And label "pending" otherwise
    And keep the state key "proposal_id"
