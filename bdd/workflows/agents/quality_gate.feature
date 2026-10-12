@decision
Feature: agents_quality_gate
  Port of the quality-gate.sh decision logic. Five checks are folded into choice
  questions. Ordered threshold rules reproduce the shell's ERROR (exit 1) vs
  WARNING (exit 0) semantics: staged_placeholder B -> FAIL, any soft violation ->
  WARN, else PASS.

  Scenario: gate
    Given a choice question "staged_placeholder" asks "Do the staged (git diff --cached) code files contain placeholder/TODO/stub patterns that must block the commit?"
      | answer | meaning                          |
      | A      | no staged placeholder/TODO       |
      | B      | yes, staged placeholder/TODO found |
    And answer "B" when the text contains "staged_placeholder", "placeholder_in_staged" or "todo_in_staged" with hit 0.95
    And a choice question "repo_placeholder" asks "Does the repo-wide placeholder/TODO scan (excluding vendored/3rd-party paths) find any files with real stubs?"
      | answer | meaning                  |
      | A      | no repo-wide real stubs  |
      | B      | yes, some files contain stubs |
    And answer "B" when the text contains "repo_placeholder" or "placeholder_count_nonzero" with hit 0.9
    And a choice question "file_lines_over" asks "Do any code/doc files exceed the 500-line limit?"
      | answer | meaning               |
      | A      | all files within limit |
      | B      | yes, files over 500 lines |
    And answer "B" when the text contains "file_lines_over" or "lines_over_500" with hit 0.9
    And a choice question "agents_md_over" asks "Does AGENTS.md exceed 500 lines?"
      | answer | meaning                    |
      | A      | AGENTS.md within 500 lines |
      | B      | yes, AGENTS.md over 500 lines |
    And answer "B" when the text contains "agents_md_over" or "agents_md_lines_over" with hit 0.9
    And a choice question "archived_naming" asks "Do any files in knowledge/docs/archived/ lack the _YYYYMMDD date suffix?"
      | answer | meaning                          |
      | A      | archived docs all dated          |
      | B      | yes, archived docs missing date suffix |
    And answer "B" when the text contains "archived_missing_date" or "naming_violation" with hit 0.9
    Then label "FAIL" when the answer to "staged_placeholder" is "B"
    And label "WARN" when the answer to "repo_placeholder" is "B"
    And label "WARN" when the answer to "file_lines_over" is "B"
    And label "WARN" when the answer to "agents_md_over" is "B"
    And label "NOTE" when the answer to "archived_naming" is "B"
    And label "PASS" otherwise
    And keep the state key "text"
    And keep the state key "repo"
    And keep the state key "branch"
