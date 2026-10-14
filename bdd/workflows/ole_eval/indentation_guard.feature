@decision
Feature: indentation_guard
  Guard for LLM-proposed Python snippets as a single triage node: the model
  proposes only a snippet, the workflow owns the line-level indentation stack,
  a deterministic indentation-only repair and the final ast.parse safety check,
  and returns the decision itself. Port of
  code/qwen_vm/app_scenarios/indentation_guard.

  The engine's own verbs: raw_accepted / repaired_accepted / rejected — the
  snippet is safe as written, safe only after a deterministic indentation repair,
  or it stays unsafe.

  Folding boundaries (documented, not computed in the DSL):
  - Snippet extraction and transport failures fold into the `proposal` choice.
  - The indentation stack (four-space unit, tab/space mixing, first-statement
    column zero, unexpected indent, unmatched dedent), the deterministic repair
    (rescale mixed or non-unit indentation to four-space units; snap an
    unexpected indent or an unmatched dedent) and the final ast.parse check are
    the authority; the harness runs them and hands the DSL one answer per check.
  - The engine appends every issue; the port carries the first code of the
    *final* (post-repair) validation plus the first repair kind applied. A
    snippet that is already safe reports no repair even when the repairer would
    have rescaled it, because raw safety alone decides raw_accepted.

  Emitted labels: reject_proposal, reject_mixed_tab_space, reject_non_unit_width,
  reject_leading_indented_statement, reject_unexpected_indent,
  reject_unindent_no_level, reject_python_indent_error,
  reject_python_syntax_error, repair_rescaled, repair_realigned, accept.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Did the model response yield exactly one extractable Python snippet for the indentation guard?"
      | answer    | meaning                                                 |
      | accepted  | exactly one extractable snippet                          |
      | malformed | no snippet could be extracted, or the transport failed   |
    And the answer comes from the field "ev_proposal"
    And a choice question "reject_reason" asks "What is the first code of the final (post-repair) indentation validation, when the snippet stays unsafe?"
      | answer                     | meaning                                                     |
      | none                       | the snippet is safe (no final issue)                         |
      | mixed_tab_space            | a line mixes tabs and spaces                                 |
      | non_unit_width             | an indentation width is not a multiple of four               |
      | leading_indented_statement | the first significant statement does not start at column zero |
      | unexpected_indent          | indentation increased without an opening suite               |
      | unindent_no_level          | a dedent does not match an outer indentation level            |
      | python_indent_error        | ast.parse raised IndentationError or TabError                |
      | python_syntax_error        | ast.parse raised a non-indentation SyntaxError               |
    And the answer comes from the field "ev_reject_reason"
    And a choice question "repair" asks "When the snippet is safe only after repair, which repair kind is applied first?"
      | answer    | meaning                                                        |
      | none      | no repair is needed (the snippet is safe as written)             |
      | rescaled  | mixed or non-unit leading whitespace is rescaled to four-space units |
      | realigned | an unexpected indent or an unmatched dedent is snapped to a level |
    And the answer comes from the field "ev_repair"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_mixed_tab_space" when the answer to "reject_reason" is "mixed_tab_space"
    And label "reject_non_unit_width" when the answer to "reject_reason" is "non_unit_width"
    And label "reject_leading_indented_statement" when the answer to "reject_reason" is "leading_indented_statement"
    And label "reject_unexpected_indent" when the answer to "reject_reason" is "unexpected_indent"
    And label "reject_unindent_no_level" when the answer to "reject_reason" is "unindent_no_level"
    And label "reject_python_indent_error" when the answer to "reject_reason" is "python_indent_error"
    And label "reject_python_syntax_error" when the answer to "reject_reason" is "python_syntax_error"
    And label "repair_rescaled" when the answer to "repair" is "rescaled"
    And label "repair_realigned" when the answer to "repair" is "realigned"
    And label "accept" otherwise
    And keep the state key "task_id"
