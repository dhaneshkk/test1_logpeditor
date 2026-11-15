//! Alert evaluation at inference time lives in the dependency-free `iogp-runtime` crate (Rust API
//! and C ABI, no parser or Prolog). Re-exported here so the editor, the eval tools and the tests
//! use exactly what a detection pipeline links.
//!
//! `Engine` is the fast evaluator; `RefEngine` the reference semantics it is tested against.

pub use iogp_runtime::fast::{Engine, Kind};
pub use iogp_runtime::reference::Engine as RefEngine;
pub use iogp_runtime::{load_config, AlertEvent, AlertMachine, Frame, Object, RuleSet, Timer, EPS};
