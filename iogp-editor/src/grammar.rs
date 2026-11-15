//! Predictive-editor grammar for IOGP alert rules.
//!
//! A small Codeco-style grammar (the format `acewiki_rs::chartparser` reads), generated from the
//! catalog. It describes exactly the sentence shapes the compiler accepts, so the editor only ever
//! offers continuations that can still become a valid rule: after "is" it offers the spatial
//! prepositions, after "wears" only detector classes, after "has" only signals, and so on.
//! APE remains the authority on meaning; this grammar only drives word-by-word suggestions.
//!
//! Features: `pl` (number agreement), `an` (a/an), `sort` (class | zone | signal),
//! `subj` (may be a rule subject), `osort` (object sort required by a verb).

use crate::catalog::{Catalog, NounKind};
use acewiki_rs::chartparser::{grammar::{Grammar, LexicalRule}, parser::{ChartParser, DynamicLexicon}};
use serde_json::{json, Value};
use std::sync::Arc;

/// Start category of a rule sentence.
pub const START: &str = "rule";

fn feats(fs: &[(&str, &str)]) -> Value {
    let mut m = serde_json::Map::new();
    for (k, v) in fs {
        let val = match v.strip_prefix('$') { Some(n) => json!({"var": n.parse::<u64>().unwrap()}), None => json!({"c": v}) };
        m.insert(k.to_string(), val);
    }
    Value::Object(m)
}
fn n(name: &str, fs: &[(&str, &str)]) -> Value { json!({"t": "non", "name": name, "f": feats(fs)}) }
fn p(name: &str, fs: &[(&str, &str)]) -> Value { json!({"t": "pre", "name": name, "f": feats(fs)}) }
fn t(word: &str) -> Value { json!({"t": "term", "name": word}) }
fn rule(cats: Vec<Value>) -> Value { json!({"kind": "rule", "scope_closing": false, "ann": {}, "cats": cats}) }
fn lex(cat: &str, word: &str, fs: &[(&str, &str)]) -> Value { json!({"kind": "lex", "ann": {}, "word": word, "cat": p(cat, fs)}) }

fn an(word: &str) -> &'static str { if word.starts_with(['a', 'e', 'i', 'o', 'u']) { "plus" } else { "minus" } }

/// The grammar as JSON (also served at `/grammar.json` for inspection).
pub fn grammar_json(cat: &Catalog) -> Value {
    let mut r = vec![];
    // rule → if conds then consequent .
    r.push(rule(vec![n("rule", &[]), t("if"), n("conds", &[]), t("then"), n("cons", &[]), t(".")]));
    r.push(rule(vec![n("conds", &[]), n("cond", &[])]));
    r.push(rule(vec![n("conds", &[]), n("cond", &[]), t("and"), n("conds", &[])]));
    r.push(rule(vec![n("cond", &[]), n("subj", &[("pl", "$0")]), n("vps", &[("pl", "$0")])]));
    // subjects
    r.push(rule(vec![n("subj", &[("pl", "minus")]), p("art", &[("an", "$0")]), p("noun_sg", &[("an", "$0"), ("subj", "plus")])]));
    r.push(rule(vec![n("subj", &[("pl", "minus")]), p("art", &[("an", "$0")]), p("noun_sg", &[("an", "$0"), ("subj", "plus")]), t("that"), n("vp_core", &[("pl", "minus")])]));
    r.push(rule(vec![n("subj", &[("pl", "minus")]), t("the"), p("noun_sg", &[("subj", "plus")])]));
    r.push(rule(vec![n("subj", &[("pl", "plus")]), t("at least"), p("num", &[]), p("noun_pl", &[("subj", "plus")])]));
    r.push(rule(vec![n("subj", &[("pl", "minus")]), p("pn_zone", &[])]));
    // verb phrases
    r.push(rule(vec![n("vps", &[("pl", "$0")]), n("vp", &[("pl", "$0")])]));
    r.push(rule(vec![n("vps", &[("pl", "$0")]), n("vp", &[("pl", "$0")]), t("and"), n("vps", &[("pl", "$0")])]));
    r.push(rule(vec![n("vp", &[("pl", "$0")]), n("vp_core", &[("pl", "$0")]), n("opt_dur", &[])]));
    for (pl, cop, aux, tv) in [("minus", "is", "does not", "tv_sg"), ("plus", "are", "do not", "tv_pl")] {
        r.push(rule(vec![n("vp_core", &[("pl", pl)]), t(cop), n("sp", &[])]));
        r.push(rule(vec![n("vp_core", &[("pl", pl)]), t(&format!("{cop} not")), n("sp", &[])]));
        r.push(rule(vec![n("vp_core", &[("pl", pl)]), p(tv, &[("osort", "$0")]), n("obj", &[("sort", "$0")])]));
        r.push(rule(vec![n("vp_core", &[("pl", pl)]), t(aux), p("tv_inf", &[("osort", "$0")]), n("obj", &[("sort", "$0")])]));
    }
    // spatial phrase: preposition + a class or zone
    r.push(rule(vec![n("sp", &[]), p("prep", &[]), n("obj", &[("subj", "plus")])]));
    r.push(rule(vec![n("obj", &[("sort", "$1"), ("subj", "$2")]), p("art", &[("an", "$0")]), p("noun_sg", &[("an", "$0"), ("sort", "$1"), ("subj", "$2")])]));
    r.push(rule(vec![n("obj", &[("sort", "$1"), ("subj", "$2")]), t("the"), p("noun_sg", &[("sort", "$1"), ("subj", "$2")])]));
    // named zones ("Rig-Floor-East") and camera scoping ("a height-zone of CAM-03")
    r.push(rule(vec![n("obj", &[("sort", "zone"), ("subj", "plus")]), p("pn_zone", &[])]));
    r.push(rule(vec![n("obj", &[("sort", "zone"), ("subj", "plus")]), p("art", &[("an", "$0")]), p("noun_sg", &[("an", "$0"), ("sort", "zone")]), t("of"), p("camera", &[])]));
    r.push(rule(vec![n("obj", &[("sort", "zone"), ("subj", "plus")]), t("the"), p("noun_sg", &[("sort", "zone")]), t("of"), p("camera", &[])]));
    // durations
    r.push(rule(vec![n("opt_dur", &[])]));
    for q in ["at least", "more than"] {
        r.push(rule(vec![n("opt_dur", &[]), t("for"), t(q), p("num", &[]), p("unit_pl", &[])]));
        r.push(rule(vec![n("opt_dur", &[]), t("for"), t(q), t("1"), p("unit_sg", &[])]));
    }
    // consequent
    r.push(rule(vec![n("cons", &[]), t(&cat.agent), t("raises"), t("a"), p("severity", &[]), p("alert", &[]), n("opt_for", &[]), n("opt_unless", &[])]));
    // exceptions: "unless the restricted-area has a permit [or the worker has a permit]"
    r.push(rule(vec![n("opt_unless", &[])]));
    r.push(rule(vec![n("opt_unless", &[]), t("unless"), n("excs", &[])]));
    r.push(rule(vec![n("excs", &[]), n("exc", &[])]));
    r.push(rule(vec![n("excs", &[]), n("exc", &[]), t("or"), n("excs", &[])]));
    r.push(rule(vec![n("exc", &[]), t("the"), p("noun_sg", &[("subj", "plus")]), p("tv_sg", &[("osort", "signal")]), p("art", &[("an", "$0")]), p("noun_sg", &[("an", "$0"), ("sort", "signal")])]));
    r.push(rule(vec![n("exc", &[]), p("pn_zone", &[]), p("tv_sg", &[("osort", "signal")]), p("art", &[("an", "$0")]), p("noun_sg", &[("an", "$0"), ("sort", "signal")])]));
    r.push(rule(vec![n("opt_for", &[])]));
    r.push(rule(vec![n("opt_for", &[]), t("for"), t("the"), p("noun_sg", &[("subj", "plus")])]));

    let mut l = vec![lex("art", "a", &[("an", "minus")]), lex("art", "an", &[("an", "plus")])];
    let noun = |sg: &str, pl: &str, sort: &str, subj: &str, l: &mut Vec<Value>| {
        l.push(lex("noun_sg", sg, &[("an", an(sg)), ("sort", sort), ("subj", subj)]));
        l.push(lex("noun_pl", pl, &[("sort", sort), ("subj", subj)]));
    };
    for c in &cat.classes { noun(&c.noun, &c.plural, "class", "plus", &mut l); }
    for z in &cat.zones { noun(&z.noun, &z.plural, "zone", "plus", &mut l); }
    for s in &cat.signals { noun(&s.noun, &s.plural, "signal", "minus", &mut l); }
    for rel in &cat.relations {
        for osort in &rel.object_kinds {
            l.push(lex("tv_sg", &rel.third, &[("osort", osort)]));
            l.push(lex("tv_pl", &rel.verb, &[("osort", osort)]));
            l.push(lex("tv_inf", &rel.verb, &[("osort", osort)]));
        }
    }
    for s in &cat.spatial { l.push(lex("prep", &s.prep, &[])); }
    for z in &cat.zone_instances { l.push(lex("pn_zone", &z.name, &[])); }
    for c in &cat.cameras { l.push(lex("camera", &c.name, &[])); }
    for s in &cat.severities { l.push(lex("severity", s, &[])); }
    for a in &cat.alerts { l.push(lex("alert", &a.noun, &[])); }
    for u in &cat.units { l.push(lex("unit_sg", &u.noun, &[])); l.push(lex("unit_pl", &u.plural, &[])); }
    json!({"rules": r, "lexrules": l})
}

/// Numbers: any integer ≥ 2 is accepted when typed; the menu lists common values.
#[derive(Clone)]
struct Numbers;
const MENU_NUMBERS: &[u32] = &[2, 3, 4, 5, 10, 15, 20, 30, 45, 60, 90, 120];
impl DynamicLexicon for Numbers {
    fn lex_rules_for_category(&self, c: &str) -> Vec<LexicalRule> {
        if c == "num" { MENU_NUMBERS.iter().map(|i| LexicalRule::new("num", &i.to_string(), &[])).collect() } else { vec![] }
    }
    fn lex_rules_for_word(&self, w: &str) -> Vec<LexicalRule> {
        match w.parse::<u32>() { Ok(n) if n >= 2 && !w.starts_with('0') => vec![LexicalRule::new("num", w, &[])], _ => vec![] }
    }
}

/// A prepared predictive parser for one catalog; `parser()` forks it cheaply.
pub struct RuleGrammar { template: ChartParser }

impl RuleGrammar {
    pub fn new(cat: &Catalog) -> anyhow::Result<Self> {
        let g = Arc::new(Grammar::from_json(&grammar_json(cat).to_string())?);
        let mut p = ChartParser::new(g, START);
        p.set_dynamic_lexicon(Box::new(Numbers));
        p.set_rule_cache(Default::default());
        Ok(RuleGrammar { template: p })
    }
    pub fn parser(&self) -> ChartParser { self.template.clone() }
}

/// Menu heading for a suggestion (by preterminal category, or for literal words).
pub fn menu_group(cat: Option<&str>, word: &str, catalog: &Catalog) -> &'static str {
    match cat {
        Some("noun_sg" | "noun_pl") => match catalog.noun(word.trim_start_matches("the ")) {
            Some(NounKind::Class { .. }) => "detected object",
            Some(NounKind::Zone) => "zone",
            Some(NounKind::Signal { .. }) => "site signal",
            _ => "noun",
        },
        Some("tv_sg" | "tv_pl" | "tv_inf") => "observed relation",
        Some("prep") => "position",
        Some("pn_zone") => "named zone",
        Some("camera") => "camera",
        Some("severity") => "severity",
        Some("alert") => "alert (IOGP Life-Saving Rule)",
        Some("num") => "number",
        Some("unit_sg" | "unit_pl") => "time unit",
        _ => "function word",
    }
}
