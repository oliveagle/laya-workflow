@decision
Feature: formal_checker
  Guard for LLM-proposed regular expressions as a two-node DFA, port of the
  bounded Thompson-NFA-to-DFA formal checker (code/qwen_vm/app_scenarios/
  formal_checker). The model proposes only a fixture object; the parser, the
  NFA/DFA construction and the subset-construction match are the authority, and
  the accept / reject verdict plus the engine's own first cause are computed
  locally. No Python `re` evaluation and no backtracking are used.

  The engine's own verbs: every regex, input, automaton and transition budget is
  bounded before execution starts; a proposal is accepted when the pattern
  compiles and every candidate input is classified exactly as the proposal
  claims, otherwise it is rejected with the engine's first cause (a regex error
  with a precise source position, or a self-inconsistent classification).

  States: proposal -> verify. The parse node folds the balanced-JSON / fenced
  extraction and the fixture schema (exactly the keys regex, accepted and
  rejected; a string pattern; arrays of candidate strings; at most 12
  candidates). A proposal that fails parse never reaches the verifier.

  Folding boundaries (documented, not computed in the DSL):
  - The envelope recovery and the fixture schema fold into the `proposal` choice.
  - The bounded regex subset (anchors only at the ends, code-point character
    classes, the predefined escape classes, the operator set, the depth / length
    caps), the Thompson NFA and the lazy subset-construction DFA are the
    authority; the harness runs them and hands the DSL one answer per check. The
    engine raises on the first error, so the reject ladder mirrors the engine's
    own raise order (the parse-time syntax errors and caps first, then the NFA
    cap, then the lazy DFA cap, then the per-input length and transition caps).
  - A regex error and a self-inconsistent classification are folded into the one
    `check` answer the verifier routes on; `mismatch` is the no-error-but-wrong
    case, an error code is the engine's first raised cause.

  Emitted labels: reject_proposal, reject_regex_too_long, reject_nesting_exceeded,
  reject_unexpected_char, reject_anchor_position, reject_group, reject_charclass,
  reject_quantifier, reject_escape, reject_nfa_limit, reject_dfa_limit,
  reject_input_too_long, reject_transition_limit, reject_mismatch, accept.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: proposal
    Given a choice question "proposal" asks "Did the model response yield exactly one balanced JSON object with exactly the keys regex, accepted and rejected (a string pattern, two arrays of candidate strings, at most 12 candidates)?"
      | answer    | meaning                                                                                |
      | parsed    | exactly one schema-valid fixture object                                              |
      | malformed | no balanced object, wrong key set, wrong types, a non-string candidate, or more than 12 candidates |
    And the answer comes from the field "ev_proposal"
    When the answer "parsed" goes to "verify"
    And any other answer goes to "STOP"
    Then label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "proposal_ok" otherwise
    And keep the state key "request_id"

  Scenario: verify
    Given a choice question "check" asks "What is the deterministic checker's first cause for this fixture (compile the bounded regex, then classify every candidate input)?"
      | answer            | meaning                                                                                 |
      | consistent        | the regex compiled and every candidate classified exactly as the proposal claims       |
      | mismatch          | the regex compiled but at least one candidate's deterministic result differs from the claim |
      | regex_too_long    | the pattern exceeds 192 characters                                                      |
      | nesting_exceeded  | the pattern nests deeper than 48                                                       |
      | unexpected_char   | an unexpected character, or an unexpected end of the pattern                           |
      | anchor_position   | the start or end anchor is misplaced ('$' anywhere but the end)                        |
      | group             | an opening parenthesis is never closed                                                 |
      | charclass         | an unterminated or empty character class, an out-of-order range, or an unmatched closing bracket |
      | quantifier        | a nested or repeated quantifier, a quantifier with no preceding atom, or a quantifier over a nullable expression |
      | escape            | an unsupported escape sequence                                                         |
      | nfa_limit         | the Thompson NFA exceeds 128 states                                                    |
      | dfa_limit         | the subset-construction DFA exceeds 512 states                                          |
      | input_too_long    | an input exceeds 2048 characters                                                       |
      | transition_limit  | matching exceeds 4096 transitions                                                      |
    And the answer comes from the field "ev_check"
    When any other answer goes to "STOP"
    Then label "reject_regex_too_long" when the answer to "check" is "regex_too_long"
    And label "reject_nesting_exceeded" when the answer to "check" is "nesting_exceeded"
    And label "reject_unexpected_char" when the answer to "check" is "unexpected_char"
    And label "reject_anchor_position" when the answer to "check" is "anchor_position"
    And label "reject_group" when the answer to "check" is "group"
    And label "reject_charclass" when the answer to "check" is "charclass"
    And label "reject_quantifier" when the answer to "check" is "quantifier"
    And label "reject_escape" when the answer to "check" is "escape"
    And label "reject_nfa_limit" when the answer to "check" is "nfa_limit"
    And label "reject_dfa_limit" when the answer to "check" is "dfa_limit"
    And label "reject_input_too_long" when the answer to "check" is "input_too_long"
    And label "reject_transition_limit" when the answer to "check" is "transition_limit"
    And label "accept" when the answer to "check" is "consistent"
    And label "reject_mismatch" otherwise
    And keep the state key "request_id"
