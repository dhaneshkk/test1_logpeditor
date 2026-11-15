//! # iogp-runtime — inference-time evaluation of compiled IOGP alert rules
//!
//! The editor (`iogp-editor`) parses controlled-English rules with APE and exports the approved
//! rule set as `config.json`. This crate is all a detection pipeline needs at inference time:
//! it loads that file and evaluates every camera frame. It has no parser, no Prolog and no
//! solver in it, and depends only on serde.
//!
//! * [`Engine`] — the fast evaluator (interned symbols, reused per-frame indexes, no per-binding
//!   allocation, proofs built only for raised alerts).
//! * [`reference::Engine`] — the straightforward evaluator that defines the semantics; the fast
//!   engine is tested to produce identical events.
//! * [`load_config`] — `config.json` → rules + timing.
//! * C ABI in [`ffi`] (header `include/iogp_runtime.h`).
//!
//! One engine per camera stream; an engine is `Send` but not shared between threads.

use serde::{Deserialize, Serialize};

pub mod fast;
pub mod ffi;
pub mod reference;

pub use fast::Engine;
pub use reference::{AlertEvent, AlertMachine, Frame, Object, Timer, EPS};

/// What a noun denotes in the detection world.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NounKind {
    Class { detector: String, human: bool },
    Zone,
    Signal { source: String },
    Alert { lsr: u8, rule: String },
    Unit { seconds: f64 },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Scope { Outer, Inner, Negated }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entity {
    pub var: String,
    pub noun: String,
    #[serde(flatten)]
    pub kind: NounKind,
    /// `at least N` (only for the subject of a condition)
    #[serde(default)]
    pub min_count: Option<u32>,
    pub scope: Scope,
    /// a named zone instance from the site configuration ("Rig-Floor-East"); the zone object's id
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// the zone must be one drawn on this camera ("a height-zone of CAM-03")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CondKind {
    Spatial { relation: String, detector: String, #[serde(default, skip_serializing_if = "Option::is_none")] metres: Option<f64> },
    Association { relation: String, detector: String },
    Signal { relation: String, source: String },
}

impl CondKind {
    pub fn relation(&self) -> &str {
        match self { CondKind::Spatial { relation, .. } | CondKind::Association { relation, .. } | CondKind::Signal { relation, .. } => relation }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cond {
    #[serde(flatten)]
    pub kind: CondKind,
    pub subject: String,
    pub object: String,
    pub negated: bool,
    /// the condition must have held continuously (gaps up to the grace period tolerated) this long
    #[serde(default)]
    pub min_seconds: Option<f64>,
    /// written as "... unless <subject> has a <signal>": an exception that suppresses the alert
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exception: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlertSpec { pub noun: String, pub lsr: u8, pub rule: String, pub severity: Option<String>, pub subject: Option<String> }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub text: String,
    #[serde(default)] pub paraphrase: String,
    pub readback: Vec<String>,
    pub alert: AlertSpec,
    pub entities: Vec<Entity>,
    pub conditions: Vec<Cond>,
    #[serde(default)] pub warnings: Vec<String>,
    #[serde(default)] pub fol: String,
    #[serde(default)] pub ape_tptp: String,
}

/// A loaded rule set: rules plus timing.
#[derive(Clone, Debug)]
pub struct RuleSet { pub rules: Vec<Rule>, pub grace_seconds: f64, pub cooldown_seconds: f64, pub revision: Option<String> }

#[derive(Debug)]
pub struct ConfigError(pub String);
impl std::fmt::Display for ConfigError { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) } }
impl std::error::Error for ConfigError {}

/// Load the `config.json` exported by iogp-editor (`/export/config.json`, schema `iogp-alert-rules/v1`).
pub fn load_config(json: &str) -> Result<RuleSet, ConfigError> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| ConfigError(format!("config.json: {e}")))?;
    if v["schema"].as_str() != Some("iogp-alert-rules/v1") {
        return Err(ConfigError(format!("config.json: expected schema iogp-alert-rules/v1, found {}", v["schema"])));
    }
    let grace_seconds = v["timing"]["grace_seconds"].as_f64().unwrap_or(0.7);
    let cooldown_seconds = v["timing"]["cooldown_seconds"].as_f64().unwrap_or(3.0);
    let mut rules = vec![];
    for (i, r) in v["rules"].as_array().ok_or(ConfigError("config.json: no rules array".into()))?.iter().enumerate() {
        let err = |f: &str| ConfigError(format!("config.json rule {}: missing or invalid '{f}'", r["id"].as_str().map(str::to_string).unwrap_or(i.to_string())));
        let s = |x: &serde_json::Value| x.as_str().map(str::to_string);
        rules.push(Rule {
            id: s(&r["id"]).ok_or_else(|| err("id"))?,
            text: s(&r["text"]).unwrap_or_default(),
            paraphrase: s(&r["paraphrase"]).unwrap_or_default(),
            readback: serde_json::from_value(r["readback"].clone()).unwrap_or_default(),
            alert: AlertSpec {
                noun: s(&r["alert"]["type"]).ok_or_else(|| err("alert.type"))?,
                lsr: r["lsr"]["id"].as_u64().ok_or_else(|| err("lsr.id"))? as u8,
                rule: s(&r["lsr"]["name"]).unwrap_or_default(),
                severity: s(&r["alert"]["severity"]),
                subject: s(&r["alert"]["subject"]),
            },
            entities: serde_json::from_value(r["entities"].clone()).map_err(|e| ConfigError(format!("{}: {e}", err("entities"))))?,
            conditions: serde_json::from_value(r["conditions"].clone()).map_err(|e| ConfigError(format!("{}: {e}", err("conditions"))))?,
            warnings: vec![], fol: s(&r["fol"]).unwrap_or_default(), ape_tptp: String::new(),
        });
    }
    validate(&rules)?;
    Ok(RuleSet { rules, grace_seconds, cooldown_seconds, revision: s_opt(&v["revision"]) })
}
fn s_opt(v: &serde_json::Value) -> Option<String> { v.as_str().map(str::to_string) }

/// Structural checks so a hand-edited or truncated config fails at load time, not mid-stream.
pub fn validate(rules: &[Rule]) -> Result<(), ConfigError> {
    for r in rules {
        let has = |v: &str| r.entities.iter().any(|e| e.var == v);
        for c in &r.conditions {
            if !has(&c.subject) || !has(&c.object) { return Err(ConfigError(format!("rule {}: condition refers to an undefined variable", r.id))); }
        }
        if let Some(s) = &r.alert.subject {
            if !r.entities.iter().any(|e| &e.var == s && e.scope == Scope::Outer) { return Err(ConfigError(format!("rule {}: alert subject {s} is not an outer variable", r.id))); }
        }
    }
    Ok(())
}
