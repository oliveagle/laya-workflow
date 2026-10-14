@decision
Feature: reasoning_audit
  Guard for LLM-proposed reasoning traces as a three-node DFA, port of the
  immutable request-level reasoning traces and deterministic audit replay
  (code/qwen_vm/app_scenarios/reasoning_audit). The model proposes only a trace
  (a request_id, a claimed_result and steps of opcode/operands); it never
  creates authority. The engine executes the tiny opcode set, hashes every step
  into a SHA-256 chain, appends to an append-only ledger, and replays the trace
  to detect tampering, missing/reordered steps, duplicate request IDs and
  inconsistent conclusions. The verdict and the engine's own first cause are
  computed locally.

  The engine's own verbs: a trace builds and replays (valid) or it is rejected
  with the engine's first raised build error, the ledger's duplicate rejection,
  or the replay's first violation code.

  States: proposal -> build -> replay. The parse node folds the conservative
  JSON recovery (a fenced block or the first balanced object) and the proposal
  schema (exactly the keys request_id, claimed_result and steps). The build node
  folds request_id validation, the candidate-step schema, deterministic opcode
  execution and the append-only ledger's duplicate rejection. The replay node
  folds re-execution, sequence and hash-chain verification, the claim/conclusion
  check and the trusted-root check. A proposal that fails parse never reaches
  the builder; a build failure never reaches the replay.

  Folding boundaries (documented, not computed in the DSL):
  - The JSON recovery and the proposal schema fold into the `proposal` choice.
  - The opcode set (CONST/ADD/SUB/MUL/DIV/MOD/EQ/NE/LT/LE/GT/GE/AND/OR/NOT/
    MAX/MIN), the operand rules (1..8 items, integers/booleans or the literal
    "result"), the numeric safety limit, the 64-step cap, the canonical-JSON
    SHA-256 chain and the append-only ledger are the authority; the harness runs
    them and hands the DSL one answer per check. The engine raises on the first
    build error and collects replay violations in order, so the ladders mirror
    the engine's own raise order and its ordered violation tuple.
  - The replay `violations` is an ordered, de-duplicated tuple; the port carries
    only its first code.

  Emitted labels: reject_proposal, reject_request_id, reject_step_count,
  reject_step_schema, reject_opcode, reject_operands, reject_execution,
  reject_duplicate_request_id, reject_trace_not_object,
  reject_unexpected_trace_field, reject_unsupported_schema_version,
  reject_invalid_request_id, reject_invalid_previous_hash,
  reject_invalid_step_count, reject_invalid_step_schema,
  reject_sequence_mismatch, reject_missing_or_out_of_order_step,
  reject_request_id_mismatch, reject_hash_chain_break, reject_opcode_replay_failed,
  reject_resolved_operand_mismatch, reject_replayed_result_mismatch,
  reject_entry_hash_mismatch, reject_root_hash_mismatch,
  reject_final_result_mismatch, reject_conclusion_mismatch,
  reject_conclusion_inconsistent, reject_trusted_root_mismatch,
  reject_ledger_hash_chain_break, accept.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: proposal
    Given a choice question "proposal" asks "Did the model response yield exactly one balanced JSON object with exactly the keys request_id, claimed_result and steps?"
      | answer    | meaning                                                                        |
      | parsed    | exactly one schema-valid trace proposal                                        |
      | malformed | no balanced object, a non-object value, or a wrong key set                     |
    And the answer comes from the field "ev_proposal"
    When the answer "parsed" goes to "build"
    And any other answer goes to "STOP"
    Then label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "proposal_ok" otherwise
    And keep the state key "request_id"

  Scenario: build
    Given a choice question "build" asks "What is the engine's first build cause for this proposal (validate request_id and the step schema, execute the opcodes, then append to the ledger)?"
      | answer              | meaning                                                                                       |
      | ok                  | the trace built, hashed and appended                                              |
      | request_id          | request_id does not match ^[a-z0-9][a-z0-9._-]{0,127}$                              |
      | step_count          | steps is not a list of between 1 and 64 entries                                    |
      | step_schema         | a step is not exactly an opcode/operands object                                    |
      | opcode              | an opcode is not in the supported set                                              |
      | operands            | operands is not a non-empty list of at most 8 items                                |
      | execution           | a deterministic opcode error: bad arity, a non-integer/boolean operand, a zero divisor, a numeric-safety overflow, an unsupported result type, or a result reference before the first opcode |
      | duplicate_request_id | the request_id is already in the append-only ledger                               |
    And the answer comes from the field "ev_build"
    When the answer "ok" goes to "replay"
    And any other answer goes to "STOP"
    Then label "reject_request_id" when the answer to "build" is "request_id"
    And label "reject_step_count" when the answer to "build" is "step_count"
    And label "reject_step_schema" when the answer to "build" is "step_schema"
    And label "reject_opcode" when the answer to "build" is "opcode"
    And label "reject_operands" when the answer to "build" is "operands"
    And label "reject_execution" when the answer to "build" is "execution"
    And label "reject_duplicate_request_id" when the answer to "build" is "duplicate_request_id"
    And label "build_ok" otherwise
    And keep the state key "request_id"

  Scenario: replay
    Given a choice question "replay" asks "What is the replay's first violation code (or valid) when the trace is re-executed and verified against the trusted root?"
      | answer                              | meaning                                                                                  |
      | valid                               | the trace replayed with no violation                                                     |
      | trace_not_object                    | the trace is not a JSON object                                                          |
      | unexpected_or_missing_trace_field  | the trace key set is not exactly the seven trace keys                                   |
      | unsupported_schema_version          | schema_version is not 1                                                                 |
      | invalid_request_id                  | the trace request_id fails the id pattern                                              |
      | invalid_previous_request_hash       | previous_request_hash is not a lowercase SHA-256 hex digest                            |
      | invalid_step_count                  | steps is not a list of between 1 and 64 entries                                         |
      | invalid_step_schema                 | a stored step's key set is not exactly the eight step keys                              |
      | sequence_mismatch                   | a step's sequence is not its 1-based position                                          |
      | missing_or_out_of_order_step        | a step is missing or out of order                                                      |
      | request_id_mismatch                 | a step's request_id differs from the trace's                                            |
      | hash_chain_break                    | a step's previous_hash does not equal the prior step's entry_hash                      |
      | opcode_replay_failed                | re-executing a step's opcode raised                                                      |
      | resolved_operand_mismatch           | a step's resolved_operands differ from the re-executed operands                        |
      | replayed_result_mismatch            | a step's result differs from the re-executed result                                    |
      | entry_hash_mismatch                 | a step's entry_hash does not match the recomputed hash                                   |
      | root_hash_mismatch                  | root_hash does not match the recomputed chain tail                                      |
      | final_result_mismatch               | final_result differs from the re-executed final result                                  |
      | conclusion_mismatch                 | conclusion is not the one derived from the final result                                 |
      | conclusion_inconsistent_with_claim  | claimed_result differs from final_result                                                |
      | trusted_root_mismatch               | the trace root does not equal the trusted expected root                                 |
      | ledger_hash_chain_break             | the trace's previous_request_hash breaks the ledger chain                               |
    And the answer comes from the field "ev_replay"
    When any other answer goes to "STOP"
    Then label "accept" when the answer to "replay" is "valid"
    And label "reject_trace_not_object" when the answer to "replay" is "trace_not_object"
    And label "reject_unexpected_trace_field" when the answer to "replay" is "unexpected_or_missing_trace_field"
    And label "reject_unsupported_schema_version" when the answer to "replay" is "unsupported_schema_version"
    And label "reject_invalid_request_id" when the answer to "replay" is "invalid_request_id"
    And label "reject_invalid_previous_hash" when the answer to "replay" is "invalid_previous_request_hash"
    And label "reject_invalid_step_count" when the answer to "replay" is "invalid_step_count"
    And label "reject_invalid_step_schema" when the answer to "replay" is "invalid_step_schema"
    And label "reject_sequence_mismatch" when the answer to "replay" is "sequence_mismatch"
    And label "reject_missing_or_out_of_order_step" when the answer to "replay" is "missing_or_out_of_order_step"
    And label "reject_request_id_mismatch" when the answer to "replay" is "request_id_mismatch"
    And label "reject_hash_chain_break" when the answer to "replay" is "hash_chain_break"
    And label "reject_opcode_replay_failed" when the answer to "replay" is "opcode_replay_failed"
    And label "reject_resolved_operand_mismatch" when the answer to "replay" is "resolved_operand_mismatch"
    And label "reject_replayed_result_mismatch" when the answer to "replay" is "replayed_result_mismatch"
    And label "reject_entry_hash_mismatch" when the answer to "replay" is "entry_hash_mismatch"
    And label "reject_root_hash_mismatch" when the answer to "replay" is "root_hash_mismatch"
    And label "reject_final_result_mismatch" when the answer to "replay" is "final_result_mismatch"
    And label "reject_conclusion_mismatch" when the answer to "replay" is "conclusion_mismatch"
    And label "reject_conclusion_inconsistent" when the answer to "replay" is "conclusion_inconsistent_with_claim"
    And label "reject_trusted_root_mismatch" when the answer to "replay" is "trusted_root_mismatch"
    And label "reject_ledger_hash_chain_break" when the answer to "replay" is "ledger_hash_chain_break"
    And label "reject_replay" otherwise
    And keep the state key "request_id"
