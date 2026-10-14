@decision
Feature: protocol_parser
  Guard for LLM-proposed protocol payloads as a single triage node: the model
  proposes one payload for a declared protocol, the workflow extracts and
  normalizes it (a canonical JSON value, or a single canonical SQL SELECT) and
  returns the accept / reject decision itself. Port of
  code/qwen_vm/app_scenarios/protocol_parser.

  The engine's own verbs: accepted / error(protocol) — the payload normalizes, or
  the extractor or the normalizer raises a ProtocolError tagged with its
  protocol.

  Folding boundaries (documented, not computed in the DSL):
  - The payload envelope (a string response for the declared protocol) folds into
    the `proposal` choice.
  - The extractor (JSON-object recovery for the json protocol; single-statement
    SELECT recovery from fences or labels without sanitizing it for the sql
    protocol) and the normalizer (JSON key/value shapes; the SQL grammar with no
    JOIN, GROUP BY or comment) are the authority; the harness runs them and hands
    the DSL one answer per check. A ProtocolError carries the protocol tag, so
    the reject ladder is keyed on json versus sql.

  Emitted labels: reject_proposal, reject_json, reject_sql,
  reject_unsupported_protocol, accept.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Did the model response yield exactly one payload envelope for the declared protocol?"
      | answer    | meaning                                                     |
      | accepted  | exactly one non-empty response string                        |
      | malformed | the response is empty, not a string, or the transport failed  |
    And the answer comes from the field "ev_proposal"
    And a choice question "reject_reason" asks "When the payload does not normalize, which protocol does the authority's error carry?"
      | answer                | meaning                                                      |
      | none                  | the payload normalized to its canonical form                  |
      | json                  | the json extractor or normalizer raised a json ProtocolError   |
      | sql                   | the sql extractor or normalizer raised a sql ProtocolError     |
      | unsupported_protocol  | the declared protocol is neither json nor sql                  |
    And the answer comes from the field "ev_reject_reason"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_json" when the answer to "reject_reason" is "json"
    And label "reject_sql" when the answer to "reject_reason" is "sql"
    And label "reject_unsupported_protocol" when the answer to "reject_reason" is "unsupported_protocol"
    And label "accept" otherwise
    And keep the state key "task_id"
