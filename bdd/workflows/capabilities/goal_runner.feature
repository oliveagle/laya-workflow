@decision
Feature: goal_runner

  Background:
    Given the workflow runs at most 6 iterations

  Scenario: route
    Given a choice question "harness" asks "Which agent harness should pursue the goal doc?"
      | answer | meaning              |
      | codex  | codex app-server (cxgo)        |
      | command_code | command-code headless (cmdgo)  |
    When the answer "codex" goes to "run_codex"
    And the answer "command_code" goes to "run_cmd"
    And any other answer goes to "STOP"
    Then keep the answer to "harness"
    And allow up to 1 retries

  Scenario: run_codex
    Given a choice question "run" asks "Run ${state.goal} through cxgo?"
      | answer | meaning              |
      | yes    | start the codex goal loop and keep its externally-checked verdict |
    When the answer "yes" goes to "STOP"
    And any other answer goes to "STOP"
    Then call the capability "codex_goal"
      | with.goal      | ${state.goal} |
      | with.workdir      | ${state.workdir} |
      | project.ok  | /ok |
      | project.exit_code  | /exit_code |
      | project.timed_out  | /timed_out |
      | project.elapsed_ms  | /elapsed_ms |
      | project.report_count  | /report_count |
      | project.reports_dir  | /reports_dir |
      | project.stderr  | /stderr |
    And allow up to 1 retries

  Scenario: run_cmd
    Given a choice question "run" asks "Run ${state.goal} through cmdgo?"
      | answer | meaning              |
      | yes    | start the command-code goal loop and keep its externally-checked verdict |
    When the answer "yes" goes to "STOP"
    And any other answer goes to "STOP"
    Then call the capability "cmd_goal"
      | with.goal      | ${state.goal} |
      | with.workdir      | ${state.workdir} |
      | project.ok  | /ok |
      | project.exit_code  | /exit_code |
      | project.timed_out  | /timed_out |
      | project.elapsed_ms  | /elapsed_ms |
      | project.report_count  | /report_count |
      | project.reports_dir  | /reports_dir |
      | project.stderr  | /stderr |
    And allow up to 1 retries

