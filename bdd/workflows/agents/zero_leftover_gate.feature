@decision
Feature: zero_leftover_gate

  Background:
    Given the workflow runs at most 2 iterations

  Scenario: decision
    Given a choice question "tracked_dirty" asks "Does git status --porcelain show tracked files with uncommitted changes?"
      | answer | meaning              |
      | A      | no tracked changes             |
      | B      | tracked changes present        |
    And answer "B" when the text contains "tracked_dirty", "uncommitted_changes", "leftover_tracked" with hit 0.95
    And a choice question "untracked_present" asks "Are there untracked files that should have been staged+committed (git ls-files --others --exclude-standard)?"
      | answer | meaning              |
      | A      | no untracked files             |
      | B      | untracked files present        |
    And answer "B" when the text contains "untracked_present", "untracked_leftover" with hit 0.95
    And a choice question "submodule_dirty" asks "Do submodules have uncommitted changes (out of this repo's scope, advisory only)?"
      | answer | meaning              |
      | A      | submodules clean               |
      | B      | submodule dirty                |
    And answer "B" when the text contains "submodule_dirty", "submodule_leftover" with hit 0.85
    And a choice question "allow_leftover" asks "Did the caller pass --allow-leftover='<reason>' to explicitly waive the hard constraint (not recommended)?"
      | answer | meaning              |
      | A      | no allow flag                  |
      | B      | allow-leftover set             |
    And answer "B" when the field "allow_flag" contains "allow", "leftover", "waived" with hit 0.9
    When any other answer goes to "STOP"
    Then label "ALLOW" when the answer to "allow_leftover" is "B"
    And label "LEFTOVER" when the answer to "tracked_dirty" is "B"
    And label "LEFTOVER" when the answer to "untracked_present" is "B"
    And label "WARN" when the answer to "submodule_dirty" is "B"
    And label "CLEAN" otherwise
    And keep the state key "tracked_dirty"
    And keep the state key "untracked_present"
    And keep the state key "submodule_dirty"
    And keep the state key "allow_flag"
    And keep the state key "repo"

