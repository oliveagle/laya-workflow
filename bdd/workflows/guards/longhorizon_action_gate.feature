@decision
Feature: longhorizon_action_gate
  Port of the ole-eval long-horizon trusted-state-machine action gate
  (code/longhorizon/state_machine.py, `parse_model_action`, pinned upstream by the
  module's own `_self_test`): the model may only choose an action -- run a candidate
  implementation, or declare itself done -- and the executor decides whether that
  proposal is the contract shape before it is allowed to act. A response whose
  leading code fence is stripped and which is then a JSON object whose action is
  "run" with a non-empty string code, or "done", is accepted; anything else is
  refused and named: not JSON, not an object, an action outside run/done, or a run
  with no code.

  The walk reads no number the model wrote: only the action name and the presence of
  a code string enter the decision. That is the reader's F6 guarantee (the model can
  choose an action, never dictate a value) expressed as the graph's shape.

  One fold is documented: stripping an optional leading code fence is a transform
  with no decision, so it is folded into the `parse` node's observable input (the
  fence-stripped text). Unlike the working-memory reader, this gate has no
  JSON-object *search* fallback - the fence-stripped text must be JSON on its own.

  Reference walk (code/longhorizon/state_machine.py, parse_model_action): strip an
  optional leading fence, json.loads, require a dict, require action in {run, done},
  and for run require a non-empty string code. The first refusal ends the walk.

  Background:
    Given the workflow runs at most 6 iterations

  Scenario: parse
    Given a choice question "proposal" asks "Is the response, after any leading code fence is stripped, valid JSON on its own?"
      | answer | meaning |
      | valid | json.loads accepts the fence-stripped text |
      | no_json | the fence-stripped text is not valid JSON |
    And the answer comes from the field "ev_parse"
    When the answer "no_json" goes to "STOP"
    And any other answer goes to "shape"
    Then the primary question is "proposal"
    Then label "REJECT_NO_JSON" when the answer to "proposal" is "no_json"
    And label "PARSE_OK" otherwise

  Scenario: shape
    Given a choice question "proposal" asks "Is the parsed value a JSON object -- a dict -- and not a list, string or number?"
      | answer | meaning |
      | object | the parsed value is a JSON object |
      | not_object | the parsed value is not an object |
    And the answer comes from the field "ev_shape"
    When the answer "not_object" goes to "STOP"
    And any other answer goes to "action"
    Then the primary question is "proposal"
    Then label "REJECT_NOT_OBJECT" when the answer to "proposal" is "not_object"
    And label "SHAPE_OK" otherwise

  Scenario: action
    Given a choice question "proposal" asks "What does the object's action field say? It must be exactly run or done."
      | answer | meaning |
      | run | action is run: the model wants to execute a candidate implementation |
      | done | action is done: the model declares the task finished |
      | unknown | action is missing, or outside run/done |
    And the answer comes from the field "ev_action"
    When the answer "unknown" goes to "STOP"
    And the answer "done" goes to "STOP"
    And any other answer goes to "code"
    Then the primary question is "proposal"
    Then label "REJECT_UNKNOWN_ACTION" when the answer to "proposal" is "unknown"
    And label "ACCEPT_DONE" when the answer to "proposal" is "done"
    And label "ACTION_RUN" otherwise

  Scenario: code
    Given a choice question "proposal" asks "action is run: is code a non-empty string? A run with no code cannot be executed. This is the last gate: a run with code is accepted."
      | answer | meaning |
      | ok | code is a string holding at least one non-space character |
      | missing | code is absent, is not a string, or is blank |
    And the answer comes from the field "ev_code"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    Then label "ACCEPT_RUN" when the answer to "proposal" is "ok"
    And label "REJECT_MISSING_CODE" otherwise
