@decision
Feature: reliable_state_reader
  Port of the ole-eval working-memory proposal guard (working_mem/reliable_reader.py,
  pinned upstream by working_mem/test_reliable_reader.py): the model may name an
  action and reference state field names, but it may never dictate a numeric value.
  A proposal that is not the contract shape -- a JSON object, after code fences are
  stripped, whose action is one of buy / sell / hold / reduce / increase / abort and
  whose fields_used, when present, is a list of the seven deterministic state field
  names -- is rejected and retried with a corrective message; when the retry budget
  is spent (max_retries = 3, so four attempts) the reader stops on the safe action
  hold. One fold is documented: fields_used is optional (absent or empty is a valid
  proposal whose payload references no field), and its *contents* never change the
  verdict, so the port keeps only `valid` versus the five rejection reasons.

  The decision graph reads no numeric value at all: none of the seven state fields
  enters it, so a number the model wrote cannot reach the decision. That is the
  reader's guarantee expressed as the graph's shape, rather than as an override.

  Reference walk (working_mem/reliable_reader.py, max_retries = 1 + 3): attempt 0 at
  node fresh, then retry1, then retry2, then exhausted; the first valid proposal
  ends the walk with ACCEPT, and four malformed proposals end it with FALLBACK.

  Background:
    Given the workflow runs at most 6 iterations

  Scenario: fresh
    Given a choice question "proposal" asks "Attempt 0 of 4. Is the model's proposal the contract shape -- a JSON object whose action is one of buy/sell/hold/reduce/increase/abort, and whose fields_used, when present, is a list of the seven deterministic state field names?"
      | answer | meaning |
      | valid | the object names a legal action and, when fields_used is present, only known state field names |
      | no_json | no JSON object could be extracted from the response |
      | not_object | the extracted JSON is not an object |
      | bad_action | action is missing or outside buy/sell/hold/reduce/increase/abort |
      | bad_fields_type | fields_used is present but is not a list of strings |
      | unknown_field | fields_used references a name that is not a state field |
    And the answer comes from the field "ev_fresh"
    When the answer "no_json" goes to "retry1"
    And the answer "not_object" goes to "retry1"
    And the answer "bad_action" goes to "retry1"
    And the answer "bad_fields_type" goes to "retry1"
    And the answer "unknown_field" goes to "retry1"
    And any other answer goes to "STOP"
    Then the primary question is "proposal"
    Then label "ACCEPT" when the answer to "proposal" is "valid"
    And label "RETRY" otherwise

  Scenario: retry1
    Given a choice question "proposal" asks "Attempt 1 of 4, after one corrective retry. Is the proposal the contract shape?"
      | answer | meaning |
      | valid | the object names a legal action and, when fields_used is present, only known state field names |
      | no_json | no JSON object could be extracted from the response |
      | not_object | the extracted JSON is not an object |
      | bad_action | action is missing or outside buy/sell/hold/reduce/increase/abort |
      | bad_fields_type | fields_used is present but is not a list of strings |
      | unknown_field | fields_used references a name that is not a state field |
    And the answer comes from the field "ev_retry1"
    When the answer "no_json" goes to "retry2"
    And the answer "not_object" goes to "retry2"
    And the answer "bad_action" goes to "retry2"
    And the answer "bad_fields_type" goes to "retry2"
    And the answer "unknown_field" goes to "retry2"
    And any other answer goes to "STOP"
    Then the primary question is "proposal"
    Then label "ACCEPT" when the answer to "proposal" is "valid"
    And label "RETRY" otherwise

  Scenario: retry2
    Given a choice question "proposal" asks "Attempt 2 of 4, after two corrective retries. Is the proposal the contract shape?"
      | answer | meaning |
      | valid | the object names a legal action and, when fields_used is present, only known state field names |
      | no_json | no JSON object could be extracted from the response |
      | not_object | the extracted JSON is not an object |
      | bad_action | action is missing or outside buy/sell/hold/reduce/increase/abort |
      | bad_fields_type | fields_used is present but is not a list of strings |
      | unknown_field | fields_used references a name that is not a state field |
    And the answer comes from the field "ev_retry2"
    When the answer "no_json" goes to "exhausted"
    And the answer "not_object" goes to "exhausted"
    And the answer "bad_action" goes to "exhausted"
    And the answer "bad_fields_type" goes to "exhausted"
    And the answer "unknown_field" goes to "exhausted"
    And any other answer goes to "STOP"
    Then the primary question is "proposal"
    Then label "ACCEPT" when the answer to "proposal" is "valid"
    And label "RETRY" otherwise

  Scenario: exhausted
    Given a choice question "proposal" asks "Attempt 3 of 4, the last attempt: no retry budget remains. Is the proposal the contract shape?"
      | answer | meaning |
      | valid | the object names a legal action and, when fields_used is present, only known state field names |
      | no_json | no JSON object could be extracted from the response |
      | not_object | the extracted JSON is not an object |
      | bad_action | action is missing or outside buy/sell/hold/reduce/increase/abort |
      | bad_fields_type | fields_used is present but is not a list of strings |
      | unknown_field | fields_used references a name that is not a state field |
    And the answer comes from the field "ev_exhausted"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    Then label "ACCEPT" when the answer to "proposal" is "valid"
    And label "FALLBACK" otherwise
