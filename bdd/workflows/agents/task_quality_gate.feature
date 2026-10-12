@decision
Feature: task_quality_gate
  Port of task_quality_gate: converted from dsl/agents/task_quality_gate.json (BDD round-trip).

  Background:
    Given the workflow runs at most 2 iterations
  Scenario: gate
    Given a choice question "compile_ok" asks "Did go build / cargo check succeed for the detected language?"
      | answer | meaning |
      | A      | compile passed |
      | B      | compile failed |
    And answer "B" when the text contains "compile_failed", "go_build_failed", "cargo_check_failed" with hit 0.95 and miss 0.05
    And a choice question "test_ok" asks "Did the test command succeed (go test / cargo test)?"
      | answer | meaning |
      | A      | tests passed |
      | B      | tests failed |
    And answer "B" when the text contains "test_failed", "go_test_failed", "cargo_test_failed" with hit 0.95 and miss 0.05
    And a score question "line_cover_pct" asks "Go line coverage percent (0-100). Missing field defaults to 100 so non-Go projects / unparseable coverprofiles do not fail."
      | level |
      | 0 none |
      | 40 low |
      | 80 acceptable |
      | 100 full |
    And score "line_cover_pct" from the numeric field "line_cover_pct" with miss 100.0
    And a score question "branch_cover_pct" asks "Go branch coverage percent from gobco (0-100). Missing field defaults to 100 so the no-gobco case does not fail."
      | level |
      | 0 none |
      | 40 low |
      | 80 acceptable |
      | 100 full |
    And score "branch_cover_pct" from the numeric field "branch_cover_pct" with miss 100.0
    And a choice question "placeholder_clean" asks "Did the grep scan of source files find any placeholder/TODO/FIXME/stub/TBD/Not implemented marker?"
      | answer | meaning |
      | A      | no placeholder found |
      | B      | placeholder found |
    And answer "B" when the text contains "placeholder_found", "todo_found", "fixme_found", "stub_found", "tbd_found", "not_implemented_found" with hit 0.95 and miss 0.05
    And a choice question "files_within_limit" asks "Are all source/doc files within the 500-line limit?"
      | answer | meaning |
      | A      | all within limit |
      | B      | file(s) over 500 lines |
    And answer "B" when the text contains "oversized_file", "files_over_500", "over_500_lines" with hit 0.95 and miss 0.05
    And a score question "agents_md_lines" asks "AGENTS.md line count. Strict greater-than 500 trips FAIL (shell uses -gt 500)."
      | level |
      | 0 missing |
      | 200 fine |
      | 500 fine |
      | 501 too big |
    And score "agents_md_lines" from the numeric field "agents_md_lines" with miss 0.0
    And a choice question "secrets_clean" asks "Are there any hardcoded password/secret/api_key/token/auth literals longer than 8 chars?"
      | answer | meaning |
      | A      | no hardcoded secret |
      | B      | secret pattern hit |
    And answer "B" when the field "secret_hit" matches "(?i)(password|secret|api_key|token|auth)\s*=\s*\"[^\"]{8,}\"" with hit 0.95 and miss 0.05
    And a choice question "sql_clean" asks "Are there any Go SQL-string-concat patterns (fmt.Sprintf / strings.Join + SELECT|INSERT|UPDATE|DELETE)?"
      | answer | meaning |
      | A      | no SQL concat |
      | B      | SQL concat pattern hit |
    And answer "B" when the field "sql_injection" matches "(fmt\.Sprintf|strings\.Join).*(\"SELECT|INSERT|UPDATE|DELETE)" with hit 0.95 and miss 0.05
    And a choice question "doc_naming_ok" asks "Are plan/design/prd/todo/research/meeting/adr/implementation/test_plan/release/review docs in docs/{development,research,records}/ all carrying the _YYYYMMDD date suffix?"
      | answer | meaning |
      | A      | all dated |
      | B      | some missing date suffix |
    And answer "B" when the text contains "bad_doc_naming", "missing_date_suffix" with hit 0.85 and miss 0.15
    When any other answer goes to "STOP"
    Then label "FAIL" when the answer to "compile_ok" is "B"
    And label "FAIL" when the answer to "test_ok" is "B"
    And label "FAIL" when "line_cover_pct" is below 80
    And label "FAIL" when "branch_cover_pct" is below 80
    And label "FAIL" when the answer to "placeholder_clean" is "B"
    And label "FAIL" when the answer to "files_within_limit" is "B"
    And label "FAIL" when "agents_md_lines" is at least 500.5
    And label "FAIL" when the answer to "secrets_clean" is "B"
    And label "FAIL" when the answer to "sql_clean" is "B"
    And label "WARN" when the answer to "doc_naming_ok" is "B"
    And label "PASS" otherwise
    And keep the state key "repo"
    And keep the state key "branch"
    And keep the state key "compile_failed"
    And keep the state key "test_failed"
    And keep the state key "line_cover_pct"
    And keep the state key "branch_cover_pct"
    And keep the state key "placeholder_found"
    And keep the state key "oversized_file"
    And keep the state key "agents_md_lines"
    And keep the state key "secret_hit"
    And keep the state key "sql_injection"
    And keep the state key "bad_doc_naming"
