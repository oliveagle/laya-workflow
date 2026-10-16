@decision
Feature: report_gen_gate
  Port of the ole-eval unified eval-report generator's argument gate
  (code/eval_index/report_gen.py): the front door of the eval-index reporting tool,
  which renders eval JSON (held in a SQLite index) into a standard markdown report.
  It decides whether the requested report is well-formed enough to render -- it exits
  non-zero when the requested input or arguments are missing -- so it names its verdict
  rather than reporting a number.

  Three questions are asked, in the source's own order:

    - mode: was `--auto` asked for? `main` checks `--auto` first: when set it renders
      every model's reports into the output directory and never looks at `--model` or
      `--compare` at all.
    - model: was `--model` given? On the explicit (non-auto) path, a request with no
      `--model` prints `错误: 需要指定 --model` and exits 1. This check comes before the
      type dispatch, so it fires first even when the type's own argument is missing too.
    - compare: is `--compare` present whenever it is needed? Only `--type comparison`
      requires it; a comparison request with no `--compare` prints
      `错误: 对比报告需要指定 --compare` and exits 1.

  The walk encodes exactly that: mode -> model -> compare. A request that clears every
  question reaches the render; the first question that fails names itself and ends the
  walk.

  Three folds are documented:

    - `--auto` is modeled as the `mode` node, not folded away: unlike a restore/reverse
      path, it produces the reports -- it is an accepted input shape of the same
      "generate reports" request (exit 0), so the graph names it ALL_REPORTS_RENDERED
      rather than pretending every request must carry `--model`.
    - the `--type` dispatch (performance / capability / comparison / trend / all) only
      selects which report method runs and carries no verdict of its own; the only type
      that adds a requirement is `comparison` (which needs `--compare`). The graph folds
      the other four types into the `compare` node's `not_required` answer, so the
      dispatch never becomes a node.
    - a report over an index with no matching rows is still exit 0: the generator prints
      `没有找到数据` (or `没有找到最近 30 天的数据`) and returns the report text. The gate
      judges the *request's* shape, not whether the index holds data -- so the last node's
      label is REPORT_RENDERED whether the report carries numbers or the no-data notice.

  Everything the generator produced stays in the harness -- the markdown body, the
  no-data notices, the threshold legend it calls 判定标准, and the saved file paths. The
  graph reads none of it, only whether the request passed the two argument checks.

  Reference walk (report_gen.py, main()): if `--auto`, generate all reports and exit 0;
  else if not `--model`, print the error and exit 1; else if `--type comparison` and not
  `--compare`, print the error and exit 1; else render (and optionally save) and exit 0.

  Background:
    Given the workflow runs at most 6 iterations

  Scenario: mode
    Given a choice question "request" asks "Was `--auto` asked for? `main` checks it first: when set, the generator renders every model's reports and never looks at `--model` or `--compare`. A request that asked for auto ends here, rendered."
      | answer | meaning |
      | auto | `--auto` was set: every model's reports are rendered |
      | explicit | no `--auto`: the request is checked for `--model` next |
    And the answer comes from the field "ev_mode"
    When the answer "auto" goes to "STOP"
    And any other answer goes to "model"
    Then the primary question is "request"
    Then label "ALL_REPORTS_RENDERED" when the answer to "request" is "auto"
    And label "EXPLICIT_REQUEST" otherwise

  Scenario: model
    Given a choice question "request" asks "Was `--model` given? On the explicit path a request with no `--model` prints `错误: 需要指定 --model` and exits 1 -- and this check comes before the type dispatch, so it fires first even when the type's own argument is missing too. A missing model ends the walk."
      | answer | meaning |
      | missing | no `--model`: the generator exits 1 with the error |
      | given | `--model` was given: check the type's own argument next |
    And the answer comes from the field "ev_model"
    When the answer "missing" goes to "STOP"
    And any other answer goes to "compare"
    Then the primary question is "request"
    Then label "MISSING_MODEL" when the answer to "request" is "missing"
    And label "MODEL_GIVEN" otherwise

  Scenario: compare
    Given a choice question "request" asks "Is `--compare` present whenever it is needed? Only `--type comparison` requires it, and a comparison request with no `--compare` prints `错误: 对比报告需要指定 --compare` and exits 1. This is the last question: clearing it means the request is well-formed, and the generator renders the report (a no-data report is still exit 0)."
      | answer | meaning |
      | missing | `--type comparison` without `--compare`: the generator exits 1 with the error |
      | not_required | the type is not comparison, so `--compare` is not needed |
      | provided | `--compare` was given for a comparison request |
    And the answer comes from the field "ev_compare"
    When any other answer goes to "STOP"
    Then the primary question is "request"
    Then label "COMPARISON_NEEDS_COMPARE" when the answer to "request" is "missing"
    And label "REPORT_RENDERED" otherwise
