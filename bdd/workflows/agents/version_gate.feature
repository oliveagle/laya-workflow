@decision
Feature: agents_version_gate
  Port of version_gate: converted from dsl/agents/version_gate.json (BDD round-trip).

  Scenario: check
    Given a choice question "version_format_ok" asks "Is the VERSION string a well-formed x.y.z or x.y.z-rc.N? (B = well-formed; heuristic regex hits for valid versions)"
      | answer | meaning |
      | A      | invalid format |
      | B      | yes, well-formed |
    And answer "B" when the field "version" matches "^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$" with hit 0.95 and miss 0.05
    And a choice question "version_is_rc" asks "Is the VERSION a release candidate (x.y.z-rc.N)?"
      | answer | meaning |
      | A      | stable (no -rc) |
      | B      | yes, release candidate |
    And answer "B" when the field "version" matches "^[0-9]+\.[0-9]+\.[0-9]+-rc\.[0-9]+$" with hit 0.9 and miss 0.1
    And a choice question "tag_reachable" asks "Is the expected tag v$VERSION reachable from main HEAD? (probe layer answers)"
      | answer | meaning |
      | A      | reachable |
      | B      | not reachable or missing |
    And answer "B" when the field "tag_status" contains "unreachable", "missing", "not_found" with hit 0.9 and miss 0.1
    And a choice question "origin_diverged" asks "Has main diverged from origin/main (ahead via non-MR path, or behind)?"
      | answer | meaning |
      | A      | no divergence |
      | B      | diverged |
    And answer "B" when the field "origin_status" contains "diverged", "behind", "ahead_unpushed" with hit 0.9 and miss 0.1
    And a choice question "tree_dirty" asks "Is the working tree dirty (untracked ignored, modified/staged tracked files present)?"
      | answer | meaning |
      | A      | clean |
      | B      | dirty |
    And answer "B" when the field "tree_status" contains "dirty", "modified", "staged" with hit 0.9 and miss 0.1
    When any other answer goes to "STOP"
    Then label "FAIL_INVALID_VERSION" when the answer to "version_format_ok" is "A"
    And label "FAIL_TAG_UNREACHABLE" when the answer to "tag_reachable" is "B"
    And label "FAIL_ORIGIN_DIVERGED" when the answer to "origin_diverged" is "B"
    And label "FAIL_DIRTY_TREE" when the answer to "tree_dirty" is "B"
    And label "PASS_RC" when the answer to "version_is_rc" is "B"
    And label "PASS_STABLE" otherwise
    And keep the state key "version"
    And keep the state key "tag_status"
    And keep the state key "origin_status"
    And keep the state key "tree_status"
