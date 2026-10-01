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
pub mod db;
pub mod laya_mem;
pub mod laya_mem_util;
pub mod laya_mem_vec;
#[cfg(test)]
mod laya_mem_auto_tests;
pub mod mcp;
pub mod optimizer;
pub mod orchestrate;
pub mod persist;
pub mod spec;
pub mod state;
pub mod workflow;
