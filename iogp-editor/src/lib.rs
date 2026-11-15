//! iogp-editor: controlled-English (Attempto Controlled English) rules for IOGP Life-Saving Rule
//! alerts. Sentences are parsed by APE (ape-rs, in-process on Scryer Prolog), checked against
//! the detection vocabulary, and compiled to time-indexed first-order logic plus a detector
//! configuration that a detection framework (e.g. VisionGuard360) applies.
//!
//! Design follows yappu-fsm's vision rules: sentences as rules, duration timers that tolerate
//! short detection gaps, positive evidence, an alert state machine and explainable alerts.

pub mod asp;
pub mod catalog;
pub mod compile;
pub mod config;
pub mod drs;
pub mod grammar;
pub mod ingest;
pub mod runtime;
pub mod store;
pub mod suggest;
pub mod web;
pub mod yappu_ref;

use ape_rs::Ape;
use catalog::Catalog;
use compile::{CompileError, Rule};

/// APE + catalog: compiles sentences to rules.
pub struct Compiler {
    pub cat: Catalog,
    lexicon: String,
    ape: Ape,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct ApeOut { pub drs: String, pub paraphrase: String, pub tptp: String, pub fol: String, pub messages: Vec<(String, String, String)> }

impl Compiler {
    pub fn new(cat: Catalog, ape: Ape) -> Self {
        let lexicon = cat.lexicon_text();
        Compiler { cat, lexicon, ape }
    }
    pub fn set_catalog(&mut self, cat: Catalog) { self.lexicon = cat.lexicon_text(); self.cat = cat; }
    pub fn ape(&mut self) -> &mut Ape { &mut self.ape }

    pub fn analyze(&mut self, text: &str) -> anyhow::Result<ApeOut> {
        let r = self.ape.get_results(&[("text", text), ("ulextext", &self.lexicon), ("noclex", "on"), ("guess", "off"),
            ("cdrs", "on"), ("cparaphrase1", "on"), ("ctptp", "on"), ("cfol", "on")])?;
        let x = &r.content;
        let get = |t: &str| -> String {
            let (o, c) = (format!("<{t}>"), format!("</{t}>"));
            match (x.find(&o), x.find(&c)) { (Some(i), Some(j)) if j > i => acewiki_rs::engine::unescape(&x[i + o.len()..j]).trim().to_string(), _ => String::new() }
        };
        let re = regex::Regex::new(r#"(?s)<message\s+importance="([^"]*)"\s+type="([^"]*)"\s+sentence="[^"]*"\s+token="[^"]*"\s+value="([^"]*)"\s+repair="([^"]*)""#).unwrap();
        let messages = re.captures_iter(x).map(|c| (c[1].to_string(), acewiki_rs::engine::unescape(&c[3]), acewiki_rs::engine::unescape(&c[4]))).collect();
        Ok(ApeOut { drs: get("drs"), paraphrase: get("paraphrase1"), tptp: get("tptp"), fol: get("fol"), messages })
    }

    /// Turn an APE error message into end-user language.
    fn friendly(&self, m: &(String, String, String)) -> Option<CompileError> {
        let (_, value, repair) = m;
        if repair.starts_with("Use the prefix") {
            let w = value.trim().to_string();
            let mut words: Vec<String> = self.cat.lexicon().iter().filter_map(|l| {
                let a = l.split(['(', ',']).nth(1)?.trim().trim_matches('\'').to_string(); Some(a) }).collect();
            words.extend(["is", "are", "a", "an", "the", "if", "then", "and", "does", "not", "at", "least", "more", "than"].map(String::from));
            words.sort(); words.dedup();
            let lw = w.to_lowercase();
            // words containing what was typed first (e.g. 'alert' -> the alert nouns), then near misses
            let mut sub: Vec<&str> = words.iter().filter(|x| lw.len() >= 4 && x.contains(lw.as_str()) && x.as_str() != lw).map(|x| x.as_str()).collect();
            sub.sort_by_key(|x| x.len());
            let mut close: Vec<(usize, &String)> = words.iter().map(|x| (edit(&lw, &x.to_lowercase()), x)).filter(|(d, x)| *d <= (w.len() / 3).max(1) && x.len() > 2).collect();
            close.sort();
            let mut sug: Vec<&str> = sub.into_iter().take(6).collect();
            for (_, x) in close { if sug.len() < 6 && !sug.contains(&x.as_str()) { sug.push(x); } }
            return Some(CompileError { message: format!("'{w}' is not a word {} knows", self.cat.agent),
                hint: if sug.is_empty() { "use a word offered by the editor (the catalog lists everything the detectors can see)".into() } else { format!("did you mean: {}?", sug.join(", ")) } });
        }
        if !value.contains(' ') && !repair.is_empty() && !repair.contains(' ') {
            return Some(CompileError { message: format!("'{}' is not a word {} knows", value.trim(), self.cat.agent), hint: format!("did you mean: {}?", repair.trim()) });
        }
        if value.contains("<>") {
            let before = value.split("<>").next().unwrap_or("").trim();
            return Some(CompileError { message: format!("the sentence stops being a valid rule after \"{before}\""), hint: "pick one of the words the editor offers at that point".into() });
        }
        Some(CompileError { message: value.clone(), hint: repair.clone() })
    }

    /// "If C then VisionGuard raises ... unless X has a Y [or Z has a W]." → plain ACE
    /// "If C and X does not have a Y [and Z does not have a W] then VisionGuard raises ...".
    /// APE stays the authority on meaning: it parses the rewritten sentence. Only records
    /// (catalog signals) can be exceptions; a visual exception would need the positive-evidence
    /// policy turned inside out, so it is refused with a suggestion.
    pub fn prepare(&self, text: &str) -> (String, Vec<compile::Exception>, Vec<CompileError>) {
        let t = text.trim();
        let re = regex::Regex::new(r"(?i)^(.*?)\s+unless\s+(.+?)\s*\.?\s*$").unwrap();
        let Some(c) = re.captures(t) else { return (t.to_string(), vec![], vec![]) };
        let (main, exc) = (c[1].trim().to_string(), c[2].to_string());
        let mut errs = vec![];
        let then = regex::Regex::new(r"(?i)\s+then\s+").unwrap();
        let Some(m) = then.find(&main) else {
            return (t.to_string(), vec![], vec![CompileError { message: "'unless' goes at the end of a complete rule".into(), hint: "If ... then VisionGuard raises a ... alert unless the restricted-area has a permit.".into() }]);
        };
        let clause = regex::Regex::new(r"^(?:the\s+([a-z][a-z0-9-]*)|([A-Z][A-Za-z0-9-]*))\s+([a-z]+)\s+(?:a|an)\s+([a-z][a-z0-9-]*)$").unwrap();
        let (mut add, mut exs) = (String::new(), vec![]);
        for part in regex::Regex::new(r"\s+or\s+").unwrap().split(exc.trim()) {
            let Some(k) = clause.captures(part.trim()) else {
                errs.push(CompileError { message: format!("cannot read the exception 'unless {}'", part.trim()), hint: "write 'unless the <zone or object> has a <record>', e.g. 'unless the restricted-area has a permit'".into() });
                continue;
            };
            let subj = k.get(1).or(k.get(2)).unwrap().as_str().to_string();
            let subj_txt = if k.get(1).is_some() { format!("the {subj}") } else { subj.clone() };
            let (third, obj) = (&k[3], k[4].to_string());
            match self.cat.relation_by_third(third) {
                Some(r) if r.detector == "signal" => {
                    let art = if acewiki_rs::model::use_an(&obj) { "an" } else { "a" };
                    add += &format!(" and {subj_txt} does not {} {art} {obj}", r.verb);
                    exs.push((subj, r.verb.clone(), obj));
                }
                Some(r) => errs.push(CompileError {
                    message: format!("'unless ... {} ...' cannot be an exception: cameras can miss objects, so absence must be timed", r.third),
                    hint: format!("put it in the if-part with a duration, e.g. '... and does not {} a {obj} for at least 3 seconds'", r.verb) }),
                None => errs.push(CompileError { message: format!("'{third}' is not a relation VisionGuard knows"), hint: "exceptions are records: 'unless the restricted-area has a permit'".into() }),
            }
        }
        let ace = format!("{}{add} {}.", &main[..m.start()], &main[m.start()..].trim_start());
        let ace = ace.replace(" then", " then").replace("  ", " ");
        (ace, exs, errs)
    }

    /// Text → rule, or the reasons it is not a valid alert rule (APE's and the checker's).
    pub fn compile(&mut self, id: &str, text: &str) -> Result<Rule, Vec<CompileError>> {
        let text = text.trim();
        if text.is_empty() { return Err(vec![CompileError { message: "empty rule".into(), hint: "type a sentence starting with 'If'".into() }]); }
        let (ace, exceptions, perrs) = self.prepare(text);
        if !perrs.is_empty() { return Err(perrs); }
        let out = self.analyze(&ace).map_err(|e| vec![CompileError { message: format!("APE failed: {e:#}"), hint: "internal error".into() }])?;
        let mut errs: Vec<CompileError> = out.messages.iter().filter(|m| m.0 == "error").filter_map(|m| self.friendly(m)).collect();
        // an unknown word also makes APE report where parsing stopped; the word error says it all
        if errs.iter().any(|e| e.message.contains("is not a word")) { errs.retain(|e| !e.message.starts_with("the sentence stops")); }
        let mut seen = std::collections::HashSet::new();
        errs.retain(|e| seen.insert((e.message.clone(), e.hint.clone())));
        if !errs.is_empty() || out.drs.is_empty() || out.drs == "drs([],[])" {
            return Err(if errs.is_empty() { vec![CompileError { message: "APE could not parse the sentence".into(), hint: "use the words offered by the editor".into() }] } else { errs });
        }
        let ap: String = out.messages.iter().filter(|m| m.0 == "warning").map(|m| m.1.clone()).collect::<Vec<_>>().join("; ");
        let mut r = compile::compile(&self.cat, id, text, &out.drs, &out.paraphrase, &out.tptp, &exceptions)?;
        if !ap.is_empty() { r.warnings.push(format!("APE: {ap}")); }
        Ok(r)
    }
}

/// `ID: sentence` lines (or bare sentences) → (id, text); several sentences on one line are split.
pub fn parse_lines(text: &str) -> Vec<(String, String)> {
    let re = regex::Regex::new(r"^([A-Za-z][\w.-]*):\s+(.*)$").unwrap();
    let mut out = vec![];
    for raw in text.lines() {
        let l = raw.split('#').next().unwrap().trim();
        if l.is_empty() { continue; }
        let (id, body) = match re.captures(l) { Some(c) => (Some(c[1].to_string()), c[2].to_string()), None => (None, l.to_string()) };
        let parts = split_sentences(&body);
        let many = parts.len() > 1;
        for (k, s) in parts.into_iter().enumerate() {
            let id = match &id { Some(i) if many => format!("{i}.{}", k + 1), Some(i) => i.clone(), None => format!("r{}", out.len() + 1) };
            out.push((id, s));
        }
    }
    out
}

pub fn split_sentences(t: &str) -> Vec<String> {
    let mut v = vec![]; let mut cur = String::new();
    for ch in t.chars() { cur.push(ch); if ch == '.' { let s = cur.trim().to_string(); if !s.is_empty() { v.push(s); } cur.clear(); } }
    if !cur.trim().is_empty() { v.push(cur.trim().to_string()); }
    v
}

fn edit(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() { cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1])); }
        prev = cur;
    }
    prev[b.len()]
}
