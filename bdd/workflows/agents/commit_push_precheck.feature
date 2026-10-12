@decision
Feature: commit_push_precheck

  Background:
    Given the workflow runs at most 2 iterations

  Scenario: decision
    Given a choice question "git_user_set" asks "Probe-derived status string from git config --local: 'missing' when user.name or user.email is unset, 'configured' otherwise."
      | answer | meaning              |
      | A      | git user configured            |
      | B      | git user missing               |
    And answer "B" when the field "git_user_status" contains "missing", "unset" with hit 0.95
    And a choice question "git_author_valid" asks "Probe-derived status string from bitx-ci/devine-utils-ci git-author check: 'failed' on non-zero exit, 'ok' otherwise."
      | answer | meaning              |
      | A      | author OK                      |
      | B      | author check failed            |
    And answer "B" when the field "git_author_status" contains "failed", "fail", "rejected" with hit 0.95
    And a choice question "worktree_dirty" asks "Probe-derived status string from git status --porcelain: 'dirty' when any changes exist, 'clean' when none."
      | answer | meaning              |
      | A      | worktree clean                 |
      | B      | worktree dirty                 |
    And answer "B" when the field "worktree_status" contains "dirty" with hit 0.95
    When any other answer goes to "STOP"
    Then label "FAIL_NO_GIT_USER" when the answer to "git_user_set" is "B"
    And label "FAIL_AUTHOR_CHECK" when the answer to "git_author_valid" is "B"
    And label "NO_CHANGES" when the answer to "worktree_dirty" is "A"
    And label "PASS" otherwise
    And keep the state key "git_user_status"
    And keep the state key "git_author_status"
    And keep the state key "worktree_status"
    And keep the state key "repo"

