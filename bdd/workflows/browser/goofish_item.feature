@decision
Feature: goofish_items
  Port of goofish_item: converted from dsl/browser/goofish_item.json (BDD round-trip).

  Background:
    Given the workflow runs at most 2 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts |  |
      | allow_paths | "${env.HOME}/.laya-workflow", "${env.HOME}/tmp/goofish" |
      | max_timeout_ms | 600000 |
      | max_output | 8388608 |
      | retries | 0 |
    Given the capabilities
      | browser | {"kind":"browser","endpoint":"http://127.0.0.1:9222","launch":true,"startup_timeout_ms":20000,"timeout_ms":90000,"max_text":20000} |
      | goofish | {"kind":"plugin","plugin":"websites/goofish","browser":"browser","timeout_ms":540000,"max_operations":4000000} |

  Scenario: collect
    Given a choice question "done" asks "Collect the requested goofish items / price changes?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then call the capability "goofish"
      | with.query | ${state.query} |
      | with.mode | ${state.mode} |
      | with.item_url | ${state.item_url} |
      | with.count | ${state.count} |
      | with.pages | ${state.pages} |
      | with.browse | ${state.browse} |
      | with.out_dir | ${state.out_dir} |
      | with.markdown | ${state.markdown} |
      | with.track | ${state.track} |
      | with.save_listing | ${state.save_listing} |
      | with.price_min | ${state.price_min} |
      | with.price_max | ${state.price_max} |
      | with.watch_query | ${state.watch_query} |
      | with.watch_limit | ${state.watch_limit} |
      | with.drop_pct | ${state.drop_pct} |
      | with.history_max | ${state.history_max} |
      | with.page_budget_ms | ${state.page_budget_ms} |
      | with.keep_open | ${state.keep_open} |
      | with.selector | ${state.selector} |
      | with.emit_sql | ${state.emit_sql} |
      | project.mode | /mode |
      | project.source_url | /source_url |
      | project.found_count | /found_count |
      | project.failed_count | /failed_count |
      | project.checked_count | /checked_count |
      | project.gone_count | /gone_count |
      | project.tracked_count | /tracked_count |
      | project.watch_file | /watch_file |
      | project.watch_saved | /watch_saved |
      | project.pages_visited | /pages_visited |
      | project.price_filtered_out | /price_filtered_out |
      | project.summary | /summary |
      | project.notes | /notes |
      | project.changes | /changes |
      | project.results | /results |
      | project.failures | /failures |
      | project.evolve | /evolve |
      | project.sql_schema | /sql_schema |
      | project.sql_statements | /sql_statements |
