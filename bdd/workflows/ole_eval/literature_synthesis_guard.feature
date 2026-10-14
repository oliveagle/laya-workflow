@decision
Feature: literature_synthesis_guard
  Guard for LLM-proposed adaptive literature syntheses as a single triage node.
  The model proposes only a theme and claim-to-evidence mappings; this workflow
  owns the fixed citation graph and abstract-like corpus, verifies paper IDs and
  quotes, computes citation-graph coverage and claim strength, and returns the
  ACCEPT / REVISE / REJECT decision itself together with the engine's own first
  blocking cause. It never searches, downloads, or opens a paper.
  Port of code/qwen_vm/app_scenarios/literature_synthesis_guard.

  The engine's own verbs: ACCEPT (the synthesis is fully supported), REVISE
  (only repairable issues: duplicate evidence, thin support, or a strength that
  disagrees with the computed strength), or REJECT (one or more hard-blocking
  reasons), each carrying the engine's own first-seen cause.

  Folding boundaries (documented, not computed in the DSL):
  - The proposal envelope (the balanced-object extraction size cap, the
    duplicate-key rejection, the dangerous-token scan, exactly the keys
    request_id/review_id/theme/claims, the identifier/shape checks, the 1..8
    claim count, the 4-key claim shape, the 1..16 evidence count, the 3-key
    evidence shape, the paper_id format, the whitelisted relation and the
    quote length) folds into the `proposal` choice.
  - The fixed paper corpus, the theme scopes and their core-paper sets are the
    authority; the harness runs the deterministic review and hands the DSL one
    answer per check. The engine appends every reason in stage order; the port
    carries only the first-appended cause, so the ladder mirrors the engine's
    own stage order (ledger, theme, claims, evidence, graph, coverage, strength).
  - The engine's own stage machine (START -> PROPOSAL_PARSED -> THEME_CHECKED ->
    CLAIMS_CHECKED -> EVIDENCE_CHECKED -> GRAPH_CHECKED -> COVERAGE_CHECKED ->
    STRENGTH_CHECKED -> DECIDED) is a linear pipeline with no alternative
    branches, so a single evaluation is one node visit.
  - The in-memory request ledger (the DUPLICATE_REQUEST_ID replay guard) is not
    replayed inside the DSL; the harness owns it.

  Emitted labels: reject_proposal, reject_duplicate_request_id, reject_unknown_theme,
  reject_theme_scope_mismatch, reject_duplicate_claim_id, reject_duplicate_claim_text,
  reject_paper_id_not_found, reject_paper_outside_theme, reject_evidence_relation_mismatch,
  reject_evidence_not_in_abstract_corpus, reject_fabricated_citation,
  reject_contradictory_evidence_for_paper, reject_contradictory_evidence,
  reject_citation_graph_disconnected, reject_core_coverage_below_threshold,
  reject_no_supporting_evidence, revise, accept.

  Background:
    Given the workflow runs at most 5 iterations

  Scenario: guard
    Given a choice question "proposal" asks "Does the model response parse as exactly one synthesis proposal {request_id, review_id, theme, claims} whose claims and evidence satisfy the envelope (identifiers, 1..8 claims, 1..16 evidence each, whitelisted relations, paper_id format and quote lengths)?"
      | answer    | meaning                                                                                     |
      | accepted  | exactly one structurally valid synthesis proposal                                            |
      | malformed | no JSON object, an oversized output, a duplicate key, a dangerous token, a wrong key set, or a claim/evidence outside the envelope |
    And the answer comes from the field "ev_proposal"
    And a choice question "gate" asks "Against the fixed citation graph and abstract corpus, which first review cause does the engine report for this proposal?"
      | answer                              | meaning                                                                                |
      | none                                | every stage passed: the synthesis is accepted                                           |
      | duplicate_request_id                | the request_id was already reviewed in this session                                     |
      | unknown_theme                       | the theme name is not a known theme                                                      |
      | theme_scope_mismatch                | the theme scope does not match the known theme's scope                                   |
      | duplicate_claim_id                  | two claims reuse a claim_id                                                              |
      | duplicate_claim_text                | two claims carry the same normalized claim text                                          |
      | paper_id_not_found                  | a cited paper_id is absent from the fixed corpus                                         |
      | paper_outside_theme                 | a cited paper does not belong to the proposal's theme                                    |
      | evidence_relation_mismatch          | a quote is in the corpus but its declared relation differs from the corpus relation     |
      | evidence_not_in_abstract_corpus     | a quote is not a verbatim corpus evidence span for the paper                             |
      | fabricated_citation                 | a quote appears neither in the paper's evidence spans nor in its abstract               |
      | contradictory_evidence_for_paper    | one paper carries two different relations within a claim                                 |
      | contradictory_evidence              | a claim carries both supporting and refuting evidence                                    |
      | duplicate_evidence_collapsed        | a claim lists the same paper relation twice                                              |
      | citation_graph_disconnected         | the claim's valid papers are not connected in the citation graph                          |
      | core_coverage_below_threshold       | the theme's core papers are covered below the minimum core coverage                       |
      | no_supporting_evidence              | a claim has no supporting evidence                                                        |
      | insufficient_distinct_support       | a claim has only one distinct supporting paper                                            |
      | strength_downgraded                 | the proposed strength is stronger than the computed strength                              |
      | strength_upgraded                   | the proposed strength is weaker than the computed strength                                |
    And the answer comes from the field "ev_gate"
    When any other answer goes to "STOP"
    Then the primary question is "proposal"
    And the action reads the question "proposal"
    And label "reject_proposal" when the answer to "proposal" is "malformed"
    And label "reject_duplicate_request_id" when the answer to "gate" is "duplicate_request_id"
    And label "reject_unknown_theme" when the answer to "gate" is "unknown_theme"
    And label "reject_theme_scope_mismatch" when the answer to "gate" is "theme_scope_mismatch"
    And label "reject_duplicate_claim_id" when the answer to "gate" is "duplicate_claim_id"
    And label "reject_duplicate_claim_text" when the answer to "gate" is "duplicate_claim_text"
    And label "reject_paper_id_not_found" when the answer to "gate" is "paper_id_not_found"
    And label "reject_paper_outside_theme" when the answer to "gate" is "paper_outside_theme"
    And label "reject_evidence_relation_mismatch" when the answer to "gate" is "evidence_relation_mismatch"
    And label "reject_evidence_not_in_abstract_corpus" when the answer to "gate" is "evidence_not_in_abstract_corpus"
    And label "reject_fabricated_citation" when the answer to "gate" is "fabricated_citation"
    And label "reject_contradictory_evidence_for_paper" when the answer to "gate" is "contradictory_evidence_for_paper"
    And label "reject_contradictory_evidence" when the answer to "gate" is "contradictory_evidence"
    And label "reject_citation_graph_disconnected" when the answer to "gate" is "citation_graph_disconnected"
    And label "reject_core_coverage_below_threshold" when the answer to "gate" is "core_coverage_below_threshold"
    And label "reject_no_supporting_evidence" when the answer to "gate" is "no_supporting_evidence"
    And label "revise" when the answer to "gate" is "duplicate_evidence_collapsed"
    And label "revise" when the answer to "gate" is "insufficient_distinct_support"
    And label "revise" when the answer to "gate" is "strength_downgraded"
    And label "revise" when the answer to "gate" is "strength_upgraded"
    And label "accept" otherwise
    And keep the state key "request_id"
