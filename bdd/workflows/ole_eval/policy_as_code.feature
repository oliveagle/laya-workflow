@decision
Feature: policy_as_code
  Guard for LLM-proposed policies-as-code as a single triage node. The model
  proposes only one typed policy document (fields, rules with priority / condition
  / action); this workflow owns the field-schema validation, the dangerous-text
  scan, the condition / action compilation, the versioned registry (atomic
  activation, explicit rollback) and the first-priority rule evaluation with a
  default DENY, and returns the effect itself. It never executes the policy
  elsewhere. Port of code/qwen_vm/app_scenarios/policy_as_code.

  The engine's own verbs: ALLOW / DENY / REVIEW — the active policy evaluates the
  subject to allow, deny or review it; a proposal or a version step that fails is
  REJECTED.

  Folding boundaries (documented, not computed in the DSL):
  - Policy compilation (the exact typed document envelope, the balanced-JSON
    scan, the duplicate-key / constant rejection, the dangerous-text walk, the
    field-schema validation and the condition / action parse) folds into the
    `proposal` choice.
  - The registry history and the high-water mark are fixed; the harness resolves
    the version steps and hands the DSL one answer per check.
  - The activation / rollback outcome is folded into the `activation` choice; the
    first-priority rule match (or the default DENY when nothing matches) is folded
    into the `effect` choice. The ladder mirrors the engine's
    REJECT-before-effect order.

  Emitted labels: reject_policy, reject_version_conflict, reject_rollback_conflict,
  allow, deny, review, deny_no_matching_rule.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one typed policy document {version, fields, rules} that compiles (field schema, safe text, conditions and actions)?"
      | answer    | meaning                                                   |
      | compiled  | exactly one structurally valid, compilable policy document |
      | malformed | parse error, duplicate key, unsafe text or a compile error |
    And the answer comes from the field "ev_proposal"
    And a choice question "activation" asks "Against the fixed versioned registry, do the atomic activation and the optional rollback succeed?"
      | answer             | meaning                                                      |
      | ok                | activation (and any requested rollback) succeed               |
      | version_conflict  | the activated version is not the expected contiguous one      |
      | rollback_conflict | a rollback was requested and refused (not older / unknown)   |
    And the answer comes from the field "ev_activation"
    And a choice question "effect" asks "Against the active policy, what does the deterministic first-priority rule evaluation return for the subject?"
      | answer           | meaning                                                     |
      | allow            | a matched rule's action effect is ALLOW                       |
      | deny             | a matched rule's action effect is DENY                        |
      | review           | a matched rule's action effect is REVIEW                      |
      | no_matching_rule | no rule matched -> the default DENY "no matching policy rule" |
    And the answer comes from the field "ev_effect"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_policy" when the answer to "proposal" is "malformed"
    And label "reject_version_conflict" when the answer to "activation" is "version_conflict"
    And label "reject_rollback_conflict" when the answer to "activation" is "rollback_conflict"
    And label "allow" when the answer to "effect" is "allow"
    And label "review" when the answer to "effect" is "review"
    And label "deny_no_matching_rule" when the answer to "effect" is "no_matching_rule"
    And label "deny" otherwise
    And keep the state key "policy_version"
