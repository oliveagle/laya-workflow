@decision
Feature: routing_gate
  Port of the ole-eval r28d-Gamma model-routing eval
  (code/distill/scripts/r28d_gamma_routing.py): a 60-task x 4-sample pass@4 run that
  routes each task to one of two served models -- easy tasks (index below the threshold
  of 28) to Qwopus v6, the rest to r26s75 -- and then decides whether the routed mix
  beats the best single-model baseline. It is the verdict the R9 routing line leans on
  to call the routing RECOMMENDED or not, so it names its decision rather than
  reporting a number.

  It makes two calls, in the source's own order:

    - route: which arm does this task go to? `route_for(i)` sends tasks with `i < 28`
      to Qwopus v6 (:11035) and `i >= 28` to r26s75 (:11043). Routing never rejects --
      both arms are run and both reach the gate.
    - gate: does the routed overall pass@4 rate beat the best single-model baseline?
      The docstring names the rule `Gate: routed > 61.25% -> RECOMMENDED`; the code
      records it as `gate.passed = overall > 0.6125`, i.e. strictly above the
      r26s75+s10 baseline of 0.6125.

  The walk encodes exactly that: route -> gate. Routing names its arm and continues;
  the gate's below-threshold answer ends the walk.

  Two folds are documented:

    - the upstream never exits on the verdict: it computes `gate.passed`, writes it into
      the output JSON, prints the three rates, and exits 0 either way -- its exit code
      carries nothing about RECOMMENDED. The docstring's "-> RECOMMENDED" is a
      documented convention that the script never even prints. The graph names the
      verdict the JSON records; the port's job is to make that verdict explicit, not to
      invent an exit code the upstream does not have.
    - pass@4 is any-pass over four samples per task, and the rate arithmetic (total pass
      over task count, per arm) stays in the harness; the graph reads only the routing
      bucket and the gate verdict, no number the eval wrote.

  Reference walk (r28d_gamma_routing.py, main()): route each task (i < 28 -> Qwopus,
  else r26s75), run 4 samples, pass@4 per task, overall = total_pass / len(tasks);
  `gate.passed = overall > 0.6125`; save the result JSON.

  Background:
    Given the workflow runs at most 6 iterations

  Scenario: route
    Given a choice question "advise" asks "Which arm does this task go to? The routing sends tasks below the easy threshold (index 28) to Qwopus v6 and the rest to r26s75. It never rejects: both arms run and both reach the promotion gate."
      | answer | meaning |
      | easy | task index < 28: run on Qwopus v6 |
      | hard | task index >= 28: run on r26s75 |
    And the answer comes from the field "ev_route"
    When any other answer goes to "gate"
    Then the primary question is "advise"
    Then label "ROUTE_R26S75" when the answer to "advise" is "hard"
    And label "ROUTE_QWOPUS" otherwise

  Scenario: gate
    Given a choice question "advise" asks "Does the routed overall pass@4 rate beat the best single-model baseline? The docstring's rule is routed > 61.25% -> RECOMMENDED, i.e. strictly above 0.6125. This is the last call: a rate not above the baseline is not recommended."
      | answer | meaning |
      | pass | the routed overall rate is strictly above the baseline (0.6125) |
      | below | the routed overall rate is at or below the baseline |
    And the answer comes from the field "ev_gate"
    When any other answer goes to "STOP"
    Then the primary question is "advise"
    Then label "NOT_RECOMMENDED" when the answer to "advise" is "below"
    And label "RECOMMENDED" otherwise
