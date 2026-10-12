@decision
Feature: stateful_pipeline
  Port of stateful_pipeline: converted from dsl/capabilities/stateful_pipeline.json (BDD round-trip).

  Background:
    Given the workflow runs at most 10 iterations
    Given the policy
      | allow_exec | false |
      | allow_paths | "${env.LAYA_STORE_DIR}" |
      | max_timeout_ms | 5000 |
      | max_output | 32768 |

  Scenario: intake
    Given a choice question "is_duplicate" asks "Does this message look like a duplicate of something already handled?"
      | answer | meaning |
      | A | no, it is a new distinct message |
      | B | yes, it repeats an earlier message |
    When any other answer goes to "STOP"
    And the answer "A" goes to "enqueue"
    Then call the capability "counters"
      | with.key | intake_count |
      | project.count | /to |
      | project.backlog_len | /queued/length |
      | project.cache_expires_at | /cached/expires_at |
      | project.next_daily_epoch | /cron/next_epoch |
      | project.logged_bytes | /logged/bytes |
      | chain.queued.capability | backlog |
      | chain.queued.with.value | ${state.text} |
      | chain.cached.capability | ttl_cache |
      | chain.cached.with.key | intake |
      | chain.cached.with.value | 1 |
      | chain.cron.capability | next_run |
      | chain.logged.capability | note |
      | chain.logged.with.event | intake |
      | chain.logged.with.message | ${state.text} |
    Then allow up to 1 retries

  Scenario: enqueue
    Given a score question "priority" asks "What priority should this be queued at?"
      | level |
      | low |
      | normal |
      | urgent |
    When any other answer goes to "STOP"
    Then label "PRIORITY_HIGH" when "priority" is at least 1.5
    And label "PRIORITY_NORMAL" otherwise
