@decision
Feature: hf_trending_monitor

  Background:
    Given the workflow runs at most 2 iterations

  Scenario: monitor
    Given a choice question "done" asks "Fetch the current HuggingFace ranking and save the models + cards?"
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When any other answer goes to "STOP"
    Then call the capability "hf"
      | with.query      | ${state.query} |
      | with.mode      | ${state.mode} |
      | with.limit      | ${state.limit} |
      | with.sort      | ${state.sort} |
      | with.direction      | ${state.direction} |
      | with.filter      | ${state.filter} |
      | with.author      | ${state.author} |
      | with.pipeline_tag      | ${state.pipeline_tag} |
      | with.out_dir      | ${state.out_dir} |
      | with.fetch_cards      | ${state.fetch_cards} |
      | with.tz      | ${state.tz} |
      | with.tz_offset_minutes      | ${state.tz_offset_minutes} |
      | project.mode  | /mode |
      | project.query  | /query |
      | project.count  | /count |
      | project.out_dir  | /out_dir |
      | project.snapshot_path  | /snapshot_path |
      | project.latest_path  | /latest_path |
      | project.report_path  | /report_path |
      | project.history_path  | /history_path |
      | project.cards_dir  | /cards_dir |
      | project.cards_saved  | /cards_saved |
      | project.cards_failed  | /cards_failed |
      | project.fetched_at  | /fetched_at |
      | project.fetched_at_local  | /fetched_at_local |
      | project.tz  | /tz |
      | project.utc_offset_secs  | /utc_offset_secs |
      | project.models  | /models |

