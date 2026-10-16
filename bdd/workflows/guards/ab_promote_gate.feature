@decision
Feature: ab_promote_gate
  Port of the ole-eval long-horizon A/B promotion gate
  (code/distill/scripts/eval_longhorizon_gamma.py): an offline, read-only gate that
  reads a baseline and a candidate eval JSON and decides whether the candidate may be
  promoted. It is the gate the Round-9/10/11 training line leans on to call a candidate
  promoted or not, and it makes exactly two calls, each with its own floor:

    - the long-horizon floor: when the candidate is a long-horizon eval (it carries
      `windows_5min` or a `results` list), its overall pass rate -- total pass over total
      steps -- must be at or above `--threshold-pass-rate` (default 0.05). Below it, the
      candidate is not promoted.
    - the coding-regression floor: when the candidate carries a `coding_compare` block,
      the candidate arm's pass rate must be at or above `--threshold-regression`
      (default 0.56). Below it, the candidate is not promoted.

  The gate promotes only when every check that applies clears: `promote_ok` starts true
  and each failed check flips it and appends a reason. So the verdict is the conjunction
  of the two floors, and a check that does not apply is skipped, not failed.

  The walk encodes exactly that: long-horizon floor first, then the coding floor, and the
  first floor that fails ends the walk and names itself; only a candidate that clears
  every applicable floor reaches PROMOTE. The thresholds and the pass-rate arithmetic
  (a ratio over the windows) live in the harness, so each check's own verdict -- does it
  apply, does it clear -- is the graph's observable input, and the graph reads no number
  the eval wrote.

  One fold is documented: the upstream does not stop at the first failed floor. It
  collects every failed floor into a `reasons` list and prints them all, but the verdict
  it prints (`PASS` / `NO-PROMOTE`) is still the conjunction. The graph names the *first*
  floor that fails; the PASS/NO-PROMOTE verdict is identical either way.

  Reference walk (code/distill/scripts/eval_longhorizon_gamma.py, main()): if baseline and
  candidate are both long-horizon shaped and the candidate's overall pass rate is below
  the threshold, fail; if the candidate carries a coding_compare and its candidate pass
  rate is below the regression floor, fail; promote iff no floor failed.

  Background:
    Given the workflow runs at most 6 iterations

  Scenario: longhorizon
    Given a choice question "promote" asks "Does the candidate clear the long-horizon floor? Its overall pass rate (total pass over total steps) must be at or above the threshold. It does not apply when the candidate is not a long-horizon eval."
      | answer | meaning |
      | skipped | the candidate is not a long-horizon eval, so the floor does not apply |
      | meets | the floor applies and the overall pass rate is at or above the threshold |
      | below | the floor applies and the overall pass rate is below the threshold |
    And the answer comes from the field "ev_longhorizon"
    When the answer "below" goes to "STOP"
    And any other answer goes to "coding"
    Then the primary question is "promote"
    Then label "NO_PROMOTE_LONGHORIZON" when the answer to "promote" is "below"
    And label "LH_APPLIES_OK" otherwise

  Scenario: coding
    Given a choice question "promote" asks "Does the candidate clear the coding-regression floor? Its coding arm's pass rate must be at or above the regression floor. It does not apply when the candidate carries no coding comparison. This is the last gate: clearing every applicable floor promotes."
      | answer | meaning |
      | skipped | the candidate carries no coding_compare block, so the floor does not apply |
      | meets | the floor applies and the candidate arm's pass rate is at or above the floor |
      | below | the floor applies and the candidate arm's pass rate is below the floor |
    And the answer comes from the field "ev_coding"
    When any other answer goes to "STOP"
    Then the primary question is "promote"
    Then label "NO_PROMOTE_CODING" when the answer to "promote" is "below"
    And label "PROMOTE" otherwise
