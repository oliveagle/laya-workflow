@decision
Feature: feature_flag_rollout_guard
  Guard for LLM-proposed feature-flag rollouts as a single triage node. The
  model proposes one typed rollout (request_id, flag_name, rollout_plan
  {target_pct, strategy canary/blue_green/gradual, target_env staging/prod},
  risk_level low/medium/high, staging_evidence); this workflow owns the env
  allowlist, the staging-evidence rule, the high-risk canary rule and the
  single-step percentage cap. Port of
  code/qwen_vm/app_scenarios/feature_flag_rollout_guard.

  Folding boundaries (documented, not computed in the DSL):
  - JSON parsing details (required fields, non-empty flag_name, strategy/env/
    risk enums, numeric target_pct in 0-100, dangerous-token rejection) fold
    into the `proposal` choice.
  - The rollout policy (max_pct_per_step, requires_canary_for_high_risk,
    staging_required_for_prod, allowed_envs) is a fixed fixture. The step-cap
    verdict folds in risk_level, since a high-risk overrun is REJECT while a
    lower-risk overrun is ESCALATE.

  Emitted labels: reject_proposal, reject_env, reject_staging,
  reject_canary, reject_step_pct, escalate_step_pct, approve.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one {request_id, flag_name, rollout_plan {target_pct, strategy, target_env}, risk_level} object with valid strategy/env/risk enums and target_pct within 0-100?"
      | answer    | meaning                                                                |
      | accepted  | exactly one schema-valid rollout                                       |
      | malformed | parse error, missing field, bad enum, bad target_pct or dangerous token |
    And the answer comes from the field "ev_proposal"
    And a choice question "env" asks "Is target_env within the policy allowed_envs set?"
      | answer      | meaning                        |
      | allowed     | env is allowed                 |
      | not_allowed | env is not in allowed_envs     |
    And the answer comes from the field "ev_env"
    And a choice question "staging" asks "When policy requires staging for prod, is staging evidence present? (not_applicable for non-prod or when staging is not required)"
      | answer         | meaning                                     |
      | not_applicable | non-prod target or staging not required     |
      | satisfied      | prod target with staging evidence present   |
      | missing        | prod target without staging evidence        |
    And the answer comes from the field "ev_staging"
    And a choice question "canary" asks "When policy requires canary for high risk, does the strategy use canary? (not_applicable for non-high risk or when canary is not required)"
      | answer         | meaning                                        |
      | not_applicable | risk is not high or canary not required        |
      | canary         | high risk with the canary strategy             |
      | no_canary      | high risk without canary                       |
    And the answer comes from the field "ev_canary"
    And a choice question "step_pct" asks "Is target_pct within the policy max_pct_per_step? (an overrun is reported as over_high when risk_level is high, otherwise over_non_high)"
      | answer         | meaning                                   |
      | ok             | target_pct within the step cap           |
      | over_high      | overrun with risk_level high              |
      | over_non_high  | overrun with risk_level not high          |
    And the answer comes from the field "ev_step_pct"
    When any other answer goes to "STOP"
    Then the primary question is "step_pct"
    And the action reads the question "step_pct"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_env" when the answer to "env" is "not_allowed"
    And label "reject_staging" when the answer to "staging" is "missing"
    And label "reject_canary" when the answer to "canary" is "no_canary"
    And label "reject_step_pct" when the answer to "step_pct" is "over_high"
    And label "escalate_step_pct" when the answer to "step_pct" is "over_non_high"
    And label "approve" otherwise
    And keep the state key "request_id"
