//! acewiki-rs — AceWiki (semantic wiki on Attempto Controlled English) in Rust.
//!
//! * [`chartparser`] — port of AceWiki's Codeco chart parser (predictive editing)
//! * [`model`], [`storage`] — AceWiki data model and the unchanged on-disk format
//! * [`engine`] — ACE→OWL via ape-rs (in-process Scryer), profile checks, reasoning
//! * [`owl`] — OWL/XML model, OWL 2 EL/RL profiles, native EL/RL reasoners, OWLlink client
//! * [`web`] — axum + HTMX wiki UI, ACE Editor, OpenAI-compatible `/v1/chat/completions`
pub mod aceeditor;
pub mod chartparser;
pub mod engine;
pub mod model;
pub mod owl;
pub mod storage;
pub mod web;

/// mimalloc as the global allocator: the chart parser and the reasoners allocate many small,
/// short-lived objects (feature maps, edges), where it is markedly faster than the system malloc.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
