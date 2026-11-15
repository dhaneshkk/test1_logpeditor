//! `ape-rs` — the Attempto Parsing Engine hosted in Rust on Scryer Prolog.
//!
//! Architecture (see MIGRATION.md):
//!
//! * The *linguistic core* (tokenizer, ACE grammar, anaphora resolution, DRS → FOL/PNF/TPTP/
//!   OWL/SWRL/RuleML/paraphrase) stays Prolog: it is ~20k lines of DCG/unification code that
//!   Prolog expresses natively. It is mechanically translated from SWI-Prolog to ISO-ish Prolog
//!   by `tools/swi2scryer.pl` and runs on the embedded Scryer engine.
//! * Everything that was SWI-specific *infrastructure* — CLI, socket server, HTTP server,
//!   memory files, timeouts, process/IO — is Rust.
//!
//! The Scryer `Machine` is not thread-safe, so an [`Ape`] instance owns one machine and is used
//! from a single thread (the HTTP server runs a dedicated worker, exactly like the original
//! APE web service which ran with `workers(1)` because APE is not thread-safe).

use scryer_prolog::{LeafAnswer, Machine, MachineBuilder, Term};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// All output types APE can produce (`solo=<type>`), as listed in `prolog/ape.pl:output_type/1`.
pub const OUTPUT_TYPES: &[&str] = &[
    "input", "tokens", "sentences", "drs", "drsrt", "syntax", "syntaxpp", "syntaxd", "syntaxdpp",
    "drspp", "drsxml", "drshtml", "paraphrase", "paraphrase1", "paraphrase2", "owlrdf", "owlfss",
    "owlfsspp", "owlxml", "ruleml", "fol", "pnf", "tptp",
];

/// Parameters accepted by APE (mirrors the SWI web service / `get_ape_results/3`).
pub const BOOLEAN_FLAGS: &[&str] = &[
    "guess", "noclex", "cinput", "ctokens", "csentences", "cdrs", "cdrsxml", "cdrspp", "cdrshtml",
    "cparaphrase", "cparaphrase1", "cparaphrase2", "ctptp", "cowlfss", "cowlfsspp", "cowlrdf",
    "cowlxml", "cruleml", "cfol", "cpnf", "csyntax", "csyntaxpp", "csyntaxd", "csyntaxdpp",
];
pub const VALUE_PARAMS: &[&str] = &["text", "solo", "ulextext", "uri"];

#[derive(Debug)]
pub enum ApeError {
    Load(String),
    Prolog(String),
    BadParam(String),
}
impl fmt::Display for ApeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApeError::Load(s) => write!(f, "failed to load APE: {s}"),
            ApeError::Prolog(s) => write!(f, "prolog error: {s}"),
            ApeError::BadParam(s) => write!(f, "bad parameter: {s}"),
        }
    }
}
impl std::error::Error for ApeError {}

/// One APE result: MIME content type plus body (exactly what SWI-APE returns).
#[derive(Debug, Clone, PartialEq)]
pub struct ApeResult {
    pub content_type: String,
    pub content: String,
    pub elapsed: Duration,
}

/// An APE engine instance (one Scryer machine with the converted APE loaded).
pub struct Ape {
    machine: Machine,
    time_limit_secs: u32,
    dir: PathBuf,
    queries: u32,
    /// Rebuild the Scryer machine after this many requests (0 = never). Scryer does not reclaim
    /// retracted clauses, and APE retracts its dynamic user lexicon on every request with
    /// `ulextext`, so a long-lived engine grows by ~2 MB/request without recycling.
    pub recycle_after: u32,
    pub recycles: u32,
}

impl Ape {
    /// Load APE from the directory holding the converted Prolog tree (contains `ape.pl`).
    pub fn new(prolog_dir: impl AsRef<Path>) -> Result<Self, ApeError> {
        let dir: PathBuf = prolog_dir.as_ref().canonicalize().map_err(|e| ApeError::Load(e.to_string()))?;
        let ape_pl = dir.join("ape.pl");
        if !ape_pl.exists() {
            return Err(ApeError::Load(format!("{} not found", ape_pl.display())));
        }
        let machine = Self::boot(&dir)?;
        let recycle_after = std::env::var("APE_RECYCLE").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
        Ok(Ape { machine, time_limit_secs: 10, dir, queries: 0, recycle_after, recycles: 0 })
    }

    fn boot(dir: &Path) -> Result<Machine, ApeError> {
        let mut machine = MachineBuilder::default().build();
        let q = format!(
            "use_module({}), use_module(library(lists)), use_module(library(iso_ext)).",
            quote_atom(&dir.join("ape").to_string_lossy())
        );
        first_answer(&mut machine, &q).map_err(ApeError::Load)?;
        // Unique temp-file prefix per engine (see swi_compat:tmp_path/2): pid + process-wide counter.
        static ENGINES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let k = ENGINES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let prefix = std::env::temp_dir().join(format!("ape-{}-{k}-", std::process::id()));
        first_answer(&mut machine, &format!("bb_put(swi_tmp_prefix, {}).", quote_atom(&prefix.to_string_lossy())))
            .map_err(ApeError::Load)?;
        Ok(machine)
    }

    /// Rebuild the engine now (drops all accumulated Prolog state).
    pub fn recycle(&mut self) -> Result<(), ApeError> {
        self.machine = Self::boot(&self.dir)?;
        self.queries = 0;
        self.recycles += 1;
        Ok(())
    }

    /// Wall-clock-ish limit per request. Scryer has no asynchronous interrupt, so the
    /// converted APE enforces it as an inference budget (see `swi_compat:call_with_time_limit/2`).
    pub fn with_time_limit(mut self, secs: u32) -> Self {
        self.time_limit_secs = secs.max(1);
        self
    }

    /// Run APE with an arbitrary parameter list, e.g. `[("text","John waits."),("solo","drspp")]`.
    pub fn get_results(&mut self, params: &[(&str, &str)]) -> Result<ApeResult, ApeError> {
        let mut items = Vec::with_capacity(params.len());
        for (k, v) in params {
            if !(VALUE_PARAMS.contains(k) || BOOLEAN_FLAGS.contains(k)) {
                return Err(ApeError::BadParam(format!("unknown parameter `{k}`")));
            }
            if *k == "solo" && !OUTPUT_TYPES.contains(v) {
                return Err(ApeError::BadParam(format!("unknown output type `{v}`")));
            }
            if BOOLEAN_FLAGS.contains(k) && !matches!(*v, "on" | "off") {
                return Err(ApeError::BadParam(format!("`{k}` must be on|off")));
            }
            items.push(format!("{}={}", k, quote_atom(v)));
        }
        let q = format!(
            "ape:get_ape_results_timelimit([{}], ApeCT, ApeC, {}).",
            items.join(","),
            self.time_limit_secs
        );
        let t0 = Instant::now();
        let ans = first_answer(&mut self.machine, &q).map_err(ApeError::Prolog);
        self.queries += 1;
        if self.recycle_after > 0 && self.queries >= self.recycle_after { self.recycle()?; }
        let ans = ans?;
        let elapsed = t0.elapsed();
        match ans {
            LeafAnswer::LeafAnswer { bindings, .. } => {
                let ct = bindings.get("ApeCT").map(term_text).unwrap_or_default();
                let c = bindings.get("ApeC").map(term_text).unwrap_or_default();
                Ok(ApeResult { content_type: ct, content: c, elapsed })
            }
            other => Err(ApeError::Prolog(format!("unexpected answer {other:?}"))),
        }
    }

    /// Raw access for tooling (`ape-parity`): run a goal on the engine and return the bindings of
    /// its first solution as text. `Ok(None)` means the goal failed.
    pub fn query(&mut self, goal: &str) -> Result<Option<std::collections::BTreeMap<String, Term>>, ApeError> {
        let mut it = self.machine.run_query(goal.to_string());
        match it.next() {
            Some(Ok(LeafAnswer::LeafAnswer { bindings, .. })) => Ok(Some(bindings.into_iter().collect())),
            Some(Ok(LeafAnswer::True)) => Ok(Some(Default::default())),
            Some(Ok(LeafAnswer::False)) | None => Ok(None),
            Some(Ok(LeafAnswer::Exception(t))) | Some(Err(t)) => Err(ApeError::Prolog(term_text(&t))),
        }
    }

    /// Convenience: parse one ACE text and return a single output type.
    pub fn solo(&mut self, text: &str, output: &str) -> Result<ApeResult, ApeError> {
        self.get_results(&[("text", text), ("solo", output)])
    }
}

fn first_answer(m: &mut Machine, q: &str) -> Result<LeafAnswer, String> {
    let mut it = m.run_query(q.to_string());
    match it.next() {
        Some(Ok(LeafAnswer::Exception(t))) => Err(term_text(&t)),
        Some(Ok(LeafAnswer::False)) | None => Err(format!("query failed: {q}")),
        Some(Ok(a)) => Ok(a),
        Some(Err(t)) => Err(term_text(&t)),
    }
}

/// Render a returned term as text (atoms/strings verbatim, other terms in canonical-ish form).
pub fn term_text(t: &Term) -> String {
    match t {
        Term::Atom(s) | Term::String(s) => s.clone(),
        Term::Integer(i) => i.to_string(),
        Term::Float(f) => f.to_string(),
        Term::Rational(r) => r.to_string(),
        Term::Var(v) => format!("_{v}"),
        Term::List(xs) => format!("[{}]", xs.iter().map(term_text).collect::<Vec<_>>().join(",")),
        Term::Compound(f, args) => {
            format!("{}({})", f, args.iter().map(term_text).collect::<Vec<_>>().join(","))
        }
        other => format!("{other:?}"),
    }
}

/// Quote any string as a Prolog atom literal (ISO escapes), safe to splice into a query.
pub fn quote_atom(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        match ch {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:x}\\", c as u32)),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Locate the converted Prolog tree: `$APE_PROLOG_DIR`, else `./prolog-scryer`.
pub fn default_prolog_dir() -> PathBuf {
    std::env::var_os("APE_PROLOG_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("prolog-scryer"))
}

pub mod openai;
pub mod server;
