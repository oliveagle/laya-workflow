//! `laya-workflow-tests` — offline unit tests for the Laya workflow engine and
//! apps, mirroring the coverage of `code/laya/tests/*.py` but with a scripted
//! backend so they run without a model or server.
//!
//! Run: `cargo run --release --bin laya-workflow-tests`
//!
//! The sections live in sibling modules (`engine`, `apps`, `spec`,
//! `capabilities*`, `secrets`, `timeouts`, `heuristics`); this file owns the
//! harness, the shared helpers and the section registry.

mod rules;
mod capabilities;
mod computer_use;
mod capabilities2;
mod capabilities3;
mod codegraph;
mod engine;
mod evaluate;
mod heuristics;
mod persist;
mod secrets;
mod t_apps;
mod t_spec;
mod t_validate;
mod t_state;
mod t_bdd;
mod t_plugin;
mod timeouts;
mod update;

pub use laya_workflow::apps;
pub use laya_workflow::backend::{choice, noul, score, verdict, ScriptedBackend};
pub use laya_workflow::capability;
pub use laya_workflow::optimizer;
pub use laya_workflow::spec;
pub use laya_workflow::workflow::{
    consecutive_same, Checkpoint, Decide, Edge, FanOut, NodeAction, ResilientWorkflow, SubWorkflow,
    WorkflowNode,
};
pub use serde_json::{json, Map, Value};

pub struct Harness {
    pub passed: usize,
    pub failed: usize,
}

impl Harness {
    pub fn check(&mut self, name: &str, cond: bool) {
        if cond {
            self.passed += 1;
            println!("  ok   {name}");
        } else {
            self.failed += 1;
            println!("  FAIL {name}");
        }
    }
    pub fn eq<T: std::fmt::Debug + PartialEq>(&mut self, name: &str, got: T, want: T) {
        let ok = got == want;
        if !ok {
            println!("       got={got:?} want={want:?}");
        }
        self.check(name, ok);
    }
}

pub fn q(v: &[(&str, &str)]) -> Value {
    let mut m = Map::new();
    for (k, s) in v {
        m.insert(k.to_string(), json!(s));
    }
    Value::Object(m)
}

fn sections() -> &'static [(&'static str, fn(&mut Harness))] {
    &[
        ("rules-schema", rules::test_rules_schema),
        ("rules-probability", rules::test_rules_probability_and_band),
        ("rules-glob", rules::test_rules_glob),
        ("rules-runner", rules::test_rules_runner),
        ("rules-check-offline", rules::test_rules_check_offline),
        ("rules-hook-output", rules::test_rules_hook_output),
        ("rules-events", rules::test_rules_events),
        ("rules-compile-prompt", rules::test_rules_compile_prompt),
        ("edge", engine::test_edge),
        ("node", engine::test_node),
        ("workflow", engine::test_workflow),
        ("composition", engine::test_composition),
        (
            "apps: rule logic with scripted verdicts",
            t_apps::test_apps_rule_logic_with_scripted_verdicts,
        ),
        ("app workflows", t_apps::test_app_workflows),
        ("optimizer", t_apps::test_optimizer),
        ("spec", t_spec::test_spec),
        ("spec-closest-match", t_spec::test_spec_closest_match),
        ("spec-native-action-nodes", t_spec::test_spec_native_action_nodes),
        ("spec-nesting", t_spec::test_spec_nesting),
        ("spec-folders", t_spec::test_spec_folders),
        ("spec-version", t_spec::test_spec_version),
        ("spec-layers", t_spec::test_spec_layers),
        ("spec-state-projection-runtime", t_spec::test_spec_state_projection_runtime),
        ("validate-unknown-action-kind", t_validate::test_validate_unknown_action_kind),
        ("validate-dangling-edge", t_validate::test_validate_dangling_edge),
        ("validate-unknown-question-ref", t_validate::test_validate_unknown_question_ref),
        ("validate-control-ok", t_validate::test_validate_control_ok),
        ("validate-dead-node", t_validate::test_validate_dead_node),
        ("validate-duplicate-names", t_validate::test_validate_duplicate_names),
        ("validate-primary-q-missing", t_validate::test_validate_primary_q_missing),
        ("validate-action-q-missing", t_validate::test_validate_action_q_missing),
        ("validate-primary-required", t_validate::test_validate_primary_required),
        ("validate-gate-option", t_validate::test_validate_gate_option),
        ("validate-rubric-dim", t_validate::test_validate_rubric_dim),
        ("validate-combine-q", t_validate::test_validate_combine_q),
        ("validate-action-answer-contract", t_validate::test_validate_action_answer_contract),
        ("validate-call-and-ui-contract", t_validate::test_validate_call_and_ui_contract),
        ("validate-action-question-type", t_validate::test_validate_action_question_type),
        ("validate-state-contract", t_validate::test_validate_state_contract),
        ("validate-node-keys", t_validate::test_validate_node_keys),
        ("validate-question-keys", t_validate::test_validate_question_keys),
        ("validate-question-type", t_validate::test_validate_question_type),
        ("validate-question-criteria", t_validate::test_validate_question_criteria_shape),
        ("validate-heuristic-values", t_validate::test_validate_heuristic_values),
        ("validate-heuristic-ranges", t_validate::test_validate_heuristic_ranges),
        ("validate-heuristic-needles", t_validate::test_validate_heuristic_needles),
        ("validate-heuristic-blanks", t_validate::test_validate_heuristic_blanks),
        ("validate-threshold-values", t_validate::test_validate_threshold_when_values),
        ("validate-action-keys", t_validate::test_validate_action_keys),
        ("validate-spec-keys", t_validate::test_validate_spec_keys),
        ("validate-capability-runnable", t_validate::test_validate_capability_runnable),
        ("validate-capability-plugin", t_validate::test_validate_capability_plugin),
        ("validate-string-options", t_validate::test_validate_option_string),
        ("validate-option-domains", t_validate::test_validate_option_domains),
        ("validate-option-clamps", t_validate::test_validate_option_clamps),
        ("validate-option-attach", t_validate::test_validate_option_attach),
        ("validate-call-with", t_validate::test_validate_call_with),
        ("validate-call-project", t_validate::test_validate_call_project),
        ("validate-spec-value-types", t_validate::test_validate_spec_value_types),
        ("validate-spec-value-ranges", t_validate::test_validate_spec_value_ranges),
        ("validate-option-values", t_validate::test_validate_option_values),
        ("validate-option-values-2", t_validate::test_validate_option_values_2),
        ("validate-option-values-3", t_validate::test_validate_option_values_3),
        ("validate-option-values-4", t_validate::test_validate_option_values_4),
        ("validate-when-operator", t_validate::test_validate_when_operator),
        ("validate-edge-key", t_validate::test_validate_edge_key),
        ("validate-sub-continue", t_validate::test_validate_sub_continue),
        ("validate-copy-keys", t_validate::test_validate_copy_keys),
        ("validate-combine-fallback", t_validate::test_validate_combine_fallback),
        ("validate-ref-cycle", t_validate::test_validate_ref_cycle),
        ("validate-call-capability", t_validate::test_validate_call_capability),
        ("validate-call-chain-capability", t_validate::test_validate_call_chain_capability),
        ("validate-unread-option", t_validate::test_validate_unread_option),
        ("validate-min-conf-range", t_validate::test_validate_min_conf_range),
        ("validate-max-iterations", t_validate::test_validate_max_iterations),
        ("validate-threshold-not-exhaustive", t_validate::test_validate_threshold_not_exhaustive),
        ("validate-threshold-dead-rules", t_validate::test_validate_threshold_dead_rules),
        ("validate-rule-scope-coherence", t_validate::test_validate_rule_scope_coherence),
        ("validate-repo-specs-strict", t_validate::test_validate_repo_specs_strict),
        ("state", t_state::test_state_home_override),
        ("bdd-decision", t_bdd::test_bdd_decision),
        ("bdd-decision-primary-q", t_bdd::test_bdd_decision_primary_q),
        ("bdd-decision-rule-shapes", t_bdd::test_bdd_decision_rule_shapes),
        ("bdd-decision-extended", t_bdd::test_bdd_decision_extended),
        ("bdd-decision-call", t_bdd::test_bdd_decision_call),
        ("bdd-decision-regex", t_bdd::test_bdd_decision_regex),
        ("bdd-decision-actions", t_bdd::test_bdd_decision_actions),
        ("bdd-decision-heuristics2", t_bdd::test_bdd_decision_heuristics2),
        ("bdd-sub-workflow-continue", t_bdd::test_bdd_sub_workflow_continue),
        ("bdd-decision-policy", t_bdd::test_bdd_decision_policy),
        ("bdd-policy-ceilings", t_bdd::test_bdd_policy_ceilings),
        ("bdd-decision-ui-proposal", t_bdd::test_bdd_decision_ui_proposal),
        ("bdd-decision-roundtrip-vocab", t_bdd::test_bdd_decision_roundtrip_vocab),
        ("bdd-transpile-decision", t_bdd::test_bdd_transpile_decision),
        ("bdd-validate-suggestion", t_bdd::test_bdd_validate_suggestion),
        ("bdd-scaffold", t_bdd::test_bdd_scaffold),
        ("bdd-score-corpus-floor", t_bdd::test_bdd_score_corpus_floor),
        ("bdd-score-e2e-decision", t_bdd::test_bdd_score_e2e_decision),
        ("bdd-smoke-expect-coverage", t_bdd::test_bdd_smoke_expect_coverage),
        ("bdd-smoke-trace-path-floor", t_bdd::test_bdd_smoke_trace_path_floor),
        ("bdd-smoke-offline-external-skip", t_bdd::test_bdd_smoke_offline_external_skip),
        ("bdd-smoke-branch-coverage", t_bdd::test_bdd_smoke_branch_coverage),
        ("dsl-smoke-unasserted-state", t_bdd::test_dsl_smoke_unasserted_state),
        ("dsl-smoke-cwd-independent", t_bdd::test_dsl_smoke_cwd_independent),
        ("dsl-smoke-tally", t_bdd::test_dsl_smoke_tally),
        ("bdd-live-decisions", t_bdd::test_bdd_live_decisions),
        ("bdd-coverage-ledger", t_bdd::test_bdd_dsl_coverage_ledger),
        ("bdd-list-routes", t_bdd::test_bdd_list_routes),
        ("bdd-label-accounted", t_bdd::test_bdd_label_accounted),
        ("bdd-output-coverage", t_bdd::test_bdd_output_coverage),
        ("bdd-platform-bound-skip", t_bdd::test_bdd_platform_bound_skip),
        ("bdd-cdp-attach", t_bdd::test_bdd_cdp_attach),
        ("bdd-json-isolation", t_bdd::test_bdd_json_isolation),
        ("bdd-doc-check", t_bdd::test_bdd_doc_check),
        ("bdd-args-probe-verdicts", t_bdd::test_bdd_args_probe_verdicts),
        ("bdd-features-check", t_bdd::test_bdd_features_check),
        ("bdd-scope-census", t_bdd::test_bdd_scope_census),
        ("bdd-table-pipe-escape", t_bdd::test_bdd_table_pipe_escape),
        ("bdd-mcp-validate-ok", t_bdd::test_bdd_mcp_validate_ok),
        ("bdd-mcp-validate-err", t_bdd::test_bdd_mcp_validate_err),
        ("bdd-mcp-validate-no-input", t_bdd::test_bdd_mcp_validate_no_input),
        ("bdd-mcp-catalog", t_bdd::test_bdd_mcp_catalog),
        ("bdd-mcp-new", t_bdd::test_bdd_mcp_new),
        ("bdd-mcp-transpile", t_bdd::test_bdd_mcp_transpile),
        ("bdd-mcp-import", t_bdd::test_bdd_mcp_import),
        ("bdd-mcp-decide", t_bdd::test_bdd_mcp_decide),
        ("bdd-mcp-answers", t_bdd::test_bdd_mcp_answers),
        ("bdd-answers-from-inventory", t_bdd::test_bdd_answers_from_inventory),
        ("bdd-decide-stepwise", t_bdd::test_bdd_decide_stepwise_walk),
        ("bdd-engine-contract", t_bdd::test_bdd_engine_contract),
        ("bdd-mcp-list", t_bdd::test_bdd_mcp_list),
        ("bdd-cli-diagnostics", t_bdd::test_bdd_cli_diagnostics),
        ("bdd-vocabulary", t_bdd::test_bdd_vocabulary),
        ("bdd-single-action", t_bdd::test_bdd_single_action),
        ("bdd-table-key-once", t_bdd::test_bdd_table_key_once),
        ("bdd-step-once", t_bdd::test_bdd_step_once),
        ("bdd-call-with-keys", t_bdd::test_bdd_call_with_keys),
        ("bdd-call-project", t_bdd::test_bdd_call_project),
        ("bdd-call-with-types", t_bdd::test_bdd_call_with_types),
        ("bdd-call-with-ranges", t_bdd::test_bdd_call_with_ranges),
        ("bdd-app-port-parity", t_bdd::test_bdd_app_port_parity),
        ("bdd-app-parity", t_bdd::test_bdd_app_parity),
        ("bdd-decision-suggest", t_bdd::test_bdd_decision_suggest),
        ("bdd-decision-vocab-check", t_bdd::test_bdd_decision_vocab_check),
        ("bdd-decision-e2e-output-assert", t_bdd::test_bdd_decision_e2e_output_assert),
        ("bdd-outputs-tag-shape", t_bdd::test_bdd_outputs_tag_shape),
        ("bdd-payload-tags", t_bdd::test_bdd_payload_tags),
        ("bdd-selector-tags", t_bdd::test_bdd_selector_tags),
        ("bdd-selector-placement", t_bdd::test_bdd_selector_placement),
        ("plugin-chrome-discovery", t_plugin::test_plugin_chrome_discovery),
        ("bdd-browser-binding", t_bdd::test_bdd_browser_binding),
        ("bdd-browser-gate", t_bdd::test_bdd_browser_gate),
        ("bdd-port-check", t_bdd::test_bdd_port_check),
        ("bdd-import", t_bdd::test_bdd_import),
        ("bdd-import-dir", t_bdd::test_bdd_import_dir),
        ("install", t_state::test_install),
        ("capabilities", capabilities::test_capabilities),
        ("span-selection", capabilities::test_span_selection),
        ("quality-rubric", capabilities::test_quality_rubric),
        ("support-routing", capabilities::test_support_routing),
        ("evaluate", evaluate::test_evaluate),
        ("computer-use", computer_use::test_computer_use),
        ("capabilities-extra", capabilities::test_capabilities_extra),
        // On-device Needle 3 integration (extract / embed / complete); skipped
        // when libneedle.so or needle3.cact is not installed.
        ("needle", capabilities::test_needle),
        (
            "capabilities-batch2",
            capabilities2::test_capabilities_batch2,
        ),
        ("codegraph", codegraph::test_codegraph),
        (
            "capabilities-batch3",
            capabilities3::test_capabilities_batch3,
        ),
        ("web-research", capabilities3::test_web_research),
        ("secrets", secrets::test_secrets),
        ("capability-timeouts", timeouts::test_capability_timeouts),
        ("heuristic-fixes", heuristics::test_heuristic_fixes),
        ("persist", persist::test_persistence),
        ("accuracy", heuristics::test_accuracy),
        ("heuristic-answer-field", heuristics::test_answer_field_authority),
        ("update-version", update::test_update),
        ("update-install", update::test_extract_and_install),
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!("laya-workflow-tests [--list] [SECTION ...]");
        eprintln!("  no args       run every section (full suite)");
        eprintln!("  --list        print section names, run nothing");
        eprintln!("  SECTION ...   run only the named sections (prefix match)");
        eprintln!("  env LAYA_TEST_SECTIONS=comma,list  same selection");
        eprintln!();
        eprintln!("sections:");
        for (name, _) in sections() {
            eprintln!("  {name}");
        }
        std::process::exit(if args.len() > 1 { 0 } else { 2 });
    }
    if args.iter().any(|a| a == "--list" || a == "-l") {
        for (name, _) in sections() {
            println!("{name}");
        }
        return;
    }
    let mut h = Harness {
        passed: 0,
        failed: 0,
    };
    let mut filters: Vec<String> = args;
    if let Ok(env) = std::env::var("LAYA_TEST_SECTIONS") {
        for s in env.split(',') {
            let s = s.trim();
            if !s.is_empty() {
                filters.push(s.to_string());
            }
        }
    }
    for (name, run) in sections() {
        let selected = filters.is_empty()
            || filters
                .iter()
                .any(|f| f == "all" || name.starts_with(f.as_str()));
        if !selected {
            continue;
        }
        println!("[{name}]");
        run(&mut h);
    }
    println!("\n{} passed, {} failed", h.passed, h.failed);
    if h.failed > 0 {
        std::process::exit(1);
    }
}

fn _assert_trait_object() {
    let _: Option<&dyn Decide> = None;
}
