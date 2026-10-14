@decision
Feature: verifiable_cot
  Deterministic trace VM for verifiable chain-of-thought as a three-node DFA.
  The model proposes only typed steps ({"trace":[load/const/arith/compare/
  branch/assert]}); the parser, the control-flow contract and the deterministic
  execution are the authority, and the final PASS / REJECT verdict is computed
  locally from the authoritative inputs. Model prose, verdict fields and
  self-assertions are rejected and never read. Port of
  code/qwen_vm/app_scenarios/verifiable_cot.
  States: proposal -> contract -> execution. The parse node folds the JSON
  envelope and the per-step opcode schema; the contract node folds the
  control-flow validation (duplicate ids, undefined jump targets, loops,
  unreachable steps, the input contract); the execution node folds the typed
  evaluation and its termination. A proposal that fails parse never reaches the
  contract node, and a contract failure never reaches execution.

  Folding boundaries (documented, not computed in the DSL):
  - The response envelope, the JSON-object recovery, the {"trace":[...]} shape and
    the per-step schema (opcode allow-list, required/extra fields, bounded
    identifiers, operator sets, the 32-step cap) fold into the `proposal` choice.
  - The control-flow contract and the typed execution are the authority; the
    harness runs them and hands the DSL one answer per check. The execution node
    carries the engine's own terminate reason, which is the first reason the
    deterministic VM reports.

  Emitted labels: reject_proposal, reject_duplicate_step_id,
  reject_undefined_step_target, reject_loop, reject_unreachable_step,
  reject_input_contract, reject_undefined_input, reject_undefined_variable,
  reject_type_error, reject_division_by_zero, reject_number_bound,
  reject_ends_before_assert, reject_step_limit, reject_assert_conflict, PASS.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: proposal
    Given a choice question "proposal" asks "Did the model response parse as exactly one {\"trace\":[...]} envelope whose steps satisfy the opcode schema (allow-listed op, exact fields, bounded identifiers, valid operators, at most 32 steps)?"
      | answer    | meaning                                                              |
      | parsed    | exactly one schema-valid typed trace                                  |
      | malformed | no JSON object, wrong envelope key, empty or oversized trace array, unknown opcode, missing or extra step field, bad identifier or operator |
    And the answer comes from the field "ev_proposal"
    When the answer "parsed" goes to "contract"
    And any other answer goes to "STOP"
    Then label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "parsed" otherwise
    And keep the state key "request_id"

  Scenario: contract
    Given a choice question "contract" asks "Does the parsed trace satisfy the control-flow contract (unique step ids, every branch target defined, no loop, no unreachable step, valid inputs)?"
      | answer                | meaning                                                        |
      | ok                    | the trace satisfies the control-flow contract                   |
      | duplicate_step_id     | two steps share an id                                           |
      | undefined_step_target | a branch or fallthrough targets an undefined step               |
      | loop                  | the control-flow graph contains a cycle                        |
      | unreachable_step      | at least one step is unreachable from the entry step           |
      | input_contract        | an input name is not an identifier, a value is not an int, or it exceeds the numeric bound |
    And the answer comes from the field "ev_contract"
    When the answer "ok" goes to "execution"
    And any other answer goes to "STOP"
    Then label "reject_duplicate_step_id" when the answer to "contract" is "duplicate_step_id"
    And label "reject_undefined_step_target" when the answer to "contract" is "undefined_step_target"
    And label "reject_loop" when the answer to "contract" is "loop"
    And label "reject_unreachable_step" when the answer to "contract" is "unreachable_step"
    And label "reject_input_contract" when the answer to "contract" is "input_contract"
    And label "contract_ok" otherwise
    And keep the state key "request_id"

  Scenario: execution
    Given a choice question "execution" asks "How does the deterministic execution terminate (typed evaluation, then the first assert reached)?"
      | answer               | meaning                                                          |
      | pass                 | an assert matches the executed boolean -> PASS                    |
      | undefined_input      | a load reads an input that was not provided                       |
      | undefined_variable   | arith/compare/branch/assert read an undefined variable            |
      | type_error           | an operand is not an int, or a branch/assert condition is not a bool |
      | division_by_zero     | arith // or % with a zero right operand                           |
      | number_bound         | an arithmetic result exceeds the numeric bound                    |
      | ends_before_assert   | the executed path reaches the end of the trace without an assert  |
      | step_limit           | execution exceeds the step limit                                  |
      | assert_conflict      | an assert contradicts the executed boolean -> REJECT             |
    And the answer comes from the field "ev_execution"
    When any other answer goes to "STOP"
    Then label "reject_undefined_input" when the answer to "execution" is "undefined_input"
    And label "reject_undefined_variable" when the answer to "execution" is "undefined_variable"
    And label "reject_type_error" when the answer to "execution" is "type_error"
    And label "reject_division_by_zero" when the answer to "execution" is "division_by_zero"
    And label "reject_number_bound" when the answer to "execution" is "number_bound"
    And label "reject_ends_before_assert" when the answer to "execution" is "ends_before_assert"
    And label "reject_step_limit" when the answer to "execution" is "step_limit"
    And label "reject_assert_conflict" when the answer to "execution" is "assert_conflict"
    And label "PASS" otherwise
    And keep the state key "request_id"
