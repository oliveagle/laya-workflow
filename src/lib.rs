//! Shared library for the `laya-workflow` crate.
//!
//! * `workflow`   — resilient decision-graph runner (Laya scheduling nodes)
//! * `backend`    — `Decide` backends: HTTP client and scripted mocks
//! * `apps`       — the four Laya apps (agent gate, email triage, moderation, drafts)
//! * `capability` — capability registry (data / local / net / proto / web / ...)

pub mod accuracy;
pub mod apps;
pub mod backend;
pub mod capability;
pub mod optimizer;
pub mod orchestrate;
pub mod persist;
pub mod spec;
pub mod workflow;
