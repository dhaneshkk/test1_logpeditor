//! APE DRS → alert rule → (time-indexed first-order logic, detector configuration, readback).
//!
//! A rule is one ACE sentence `If <conditions> then <Agent> raises a [severity] <x>-alert [for the <subject>].`
//! The conditions are what APE produced, checked against the catalog:
//!
//! * `X is in|under|near|on Y [for at least N seconds]`: a spatial relation (Y is a zone or an object);
//! * `X wears|carries|holds|uses|drives a Y [for at least N seconds]`: an association detected between objects;
//! * `X has a Y`: a signal from an external system (permit-to-work, isolation register, ...);
//! * any of these negated (`does not wear ...`);
//! * `at least N Xs are in Y`: a count.
//!
//! Positive-evidence policy, as in yappu-fsm's vision rules: a camera cannot tell that a harness is
//! absent from one frame, since occlusion and dim frames also hide it. So a negated *visual*
//! condition must carry a duration (`does not wear a harness for at least 3 seconds`). Negated
//! signals come from systems of record and need none.

use crate::catalog::{Catalog, NounKind};
use crate::drs::Term;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

// The rule IR lives in the dependency-free runtime crate (shared with inference).
pub use iogp_runtime::{AlertSpec, Cond, CondKind, Entity, Rule, Scope};

#[derive(Clone, Debug, Serialize)]
pub struct CompileError { pub message: String, pub hint: String }
impl CompileError {
    fn new(m: impl Into<String>, h: impl Into<String>) -> Self { CompileError { message: m.into(), hint: h.into() } }
}
impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{} ({})", self.message, self.hint) }
}

fn list(t: &Term) -> &[Term] { match t { Term::List(l) => l, _ => &[] } }

#[derive(Default)]
struct Box_ {
    objects: BTreeMap<String, (String, String, f64)>, // var -> (lemma, op, n)
    events: Vec<(String, String, Term, Option<Term>)>, // (event var, verb, subj, obj)
    mods: BTreeMap<String, Vec<(String, Term)>>,       // event var -> [(prep, arg)]
    props: Vec<(String, String)>,
    rels: Vec<(String, String, Term)>, // relation(X, of, Y)
    negs: Vec<Term>,
    other: Vec<String>,
}

fn collect(drs: &Term) -> Box_ {
    let mut b = Box_::default();
    for c in list(&drs.args()[1]) {
        let c = c.strip_index();
        match c.functor() {
            Some(("object", 6)) => {
                let a = c.args();
                if let (Some(v), Some(l), Some(op), Some(n)) = (a[0].var(), a[1].atom(), a[4].atom(), a[5].num()) {
                    b.objects.insert(v.into(), (l.into(), op.into(), n));
                } else { b.other.push(format!("{c}")); }
            }
            Some(("predicate", 3)) => {
                let a = c.args();
                b.events.push((a[0].var().unwrap_or("?").into(), a[1].atom().unwrap_or("?").into(), a[2].clone(), None));
            }
            Some(("predicate", 4)) => {
                let a = c.args();
                b.events.push((a[0].var().unwrap_or("?").into(), a[1].atom().unwrap_or("?").into(), a[2].clone(), Some(a[3].clone())));
            }
            Some(("modifier_pp", 3)) => {
                let a = c.args();
                b.mods.entry(a[0].var().unwrap_or("?").into()).or_default().push((a[1].atom().unwrap_or("?").into(), a[2].clone()));
            }
            Some(("property", 3)) => {
                let a = c.args();
                b.props.push((a[0].var().unwrap_or("?").into(), a[1].atom().unwrap_or("?").into()));
            }
            Some(("relation", 3)) => {
                let a = c.args();
                b.rels.push((a[0].var().unwrap_or("?").into(), a[1].atom().unwrap_or("?").into(), a[2].clone()));
            }
            Some(("-", 1)) => b.negs.push(c.args()[0].clone()),
            _ => b.other.push(format!("{c}")),
        }
    }
    b
}

struct Ctx<'a> {
    cat: &'a Catalog,
    entities: BTreeMap<String, Entity>, // keyed by DRS var
    order: Vec<String>,
    errors: Vec<CompileError>,
    warnings: Vec<String>,
}

impl<'a> Ctx<'a> {
    fn declare(&mut self, drs_var: &str, lemma: &str, op: &str, n: f64, scope: Scope) -> Option<()> {
        let Some(kind) = self.cat.noun(lemma) else {
            self.errors.push(CompileError::new(format!("'{lemma}' is not in the detection vocabulary"), "use a word from the catalog"));
            return None;
        };
        if matches!(kind, NounKind::Unit { .. } | NounKind::Alert { .. }) { return Some(()); }
        let min_count = match (op, n as u32) {
            ("eq", 1) | ("na", _) => None,
            ("geq", k) if k > 1 => Some(k),
            ("greater", k) => Some(k + 1),
            ("geq", _) => None,
            _ => { self.errors.push(CompileError::new(format!("the number of {lemma}s must be given as 'at least N'"), "e.g. 'at least 3 workers are in a lifting-zone'")); None }
        };
        self.entities.insert(drs_var.into(), Entity { var: String::new(), noun: lemma.into(), kind, min_count, scope, instance: None, camera: None });
        self.order.push(drs_var.into());
        Some(())
    }

    /// A DRS argument as an entity key: a discourse referent, or a proper name of a zone instance.
    fn ent_of(&mut self, t: &Term) -> Option<String> {
        if let Some(v) = t.var() { return self.entities.contains_key(v).then(|| v.to_string()); }
        let ("named", 1) = t.functor()? else { return None };
        let name = t.args()[0].atom()?.to_string();
        let key = format!("named:{name}");
        if self.entities.contains_key(&key) { return Some(key); }
        if let Some(zi) = self.cat.zone_instance(&name).cloned() {
            self.entities.insert(key.clone(), Entity { var: String::new(), noun: zi.zone.clone(), kind: NounKind::Zone, min_count: None, scope: Scope::Outer,
                instance: Some(zi.name.clone()), camera: zi.camera.clone() });
            self.order.push(key.clone());
            return Some(key);
        }
        if self.cat.camera(&name).is_some() {
            self.errors.push(CompileError::new(format!("{name} is a camera, not a place or an object"), format!("scope a zone to it: 'a height-zone of {name}'")));
        } else if name == self.cat.agent {
            self.errors.push(CompileError::new(format!("{name} can only raise alerts"), "conditions are about detected objects, zones and records"));
        } else {
            self.errors.push(CompileError::new(format!("'{name}' is not a named zone of this site"), "named zones are listed under Vocabulary"));
        }
        None
    }

    /// "a height-zone of CAM-03": scope zone referents to a camera.
    fn apply_rels(&mut self, b: &Box_) {
        for (v, rel, y) in &b.rels {
            let cam = match y.functor() { Some(("named", 1)) => y.args()[0].atom().map(str::to_string), _ => None };
            let Some(e) = self.entities.get(v) else { continue };
            match (rel.as_str(), cam) {
                ("of", Some(c)) if self.cat.camera(&c).is_some() && matches!(e.kind, NounKind::Zone) => { self.entities.get_mut(v).unwrap().camera = Some(c); }
                ("of", Some(c)) if self.cat.camera(&c).is_some() => {
                    let n = e.noun.clone();
                    self.errors.push(CompileError::new(format!("only zones can be scoped to a camera, not a {n}"), format!("e.g. 'a worker is in a height-zone of {c}'")));
                }
                ("of", Some(c)) => {
                    let cams = self.cat.cameras.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ");
                    self.errors.push(CompileError::new(format!("'{c}' is not a camera of this site"), if cams.is_empty() { "no cameras are configured in the catalog".to_string() } else { format!("cameras: {cams}") }));
                }
                _ => self.errors.push(CompileError::new(format!("'{} of ...' is not supported", e.noun), "use 'a <zone> of <camera>', e.g. 'a height-zone of CAM-03'")),
            }
        }
    }

    fn duration(&mut self, b: &Box_, ev: &str) -> Option<f64> {
        let mut d = None;
        for (p, a) in b.mods.get(ev).into_iter().flatten() {
            if p != "for" { continue; }
            let Some(v) = a.var() else { continue };
            let Some((lemma, op, n)) = b.objects.get(v) else { continue };
            let Some(NounKind::Unit { seconds }) = self.cat.noun(lemma) else {
                self.errors.push(CompileError::new(format!("'for a {lemma}' is not a duration"), "write 'for at least N seconds'"));
                continue;
            };
            match op.as_str() {
                "geq" | "greater" => {}
                "eq" => self.warnings.push(format!("'for {n} {lemma}s' is read as 'for at least {n} {lemma}s'")),
                _ => { self.errors.push(CompileError::new("a duration must be a minimum", "write 'for at least N seconds' or 'for more than N seconds'")); continue; }
            }
            d = Some(n * seconds);
        }
        d
    }

    fn cond_of(&mut self, b: &Box_, ev: &(String, String, Term, Option<Term>), negated: bool) -> Option<Cond> {
        let (e, verb, subj, obj) = ev;
        let Some(sv) = self.ent_of(subj) else {
            self.errors.push(CompileError::new(format!("the subject of '{verb}' must be an object or zone VisionGuard detects"), "start the condition with e.g. 'a worker'"));
            return None;
        };
        let min_seconds = self.duration(b, e);
        let mods: Vec<(String, Term)> = b.mods.get(e).into_iter().flatten().filter(|(p, _)| p != "for").cloned().collect();
        if verb == "be" {
            if obj.is_some() {
                self.errors.push(CompileError::new("'is a ...' (classification) is not a condition VisionGuard can check", "say where it is: 'is in a ZONE', 'is under a suspended-load'"));
                return None;
            }
            if mods.len() != 1 {
                self.errors.push(CompileError::new("say where the object is, with exactly one position word", "'is in', 'is under', 'is near' or 'is on'"));
                return None;
            }
            let (prep, ov) = &mods[0];
            let Some(sp) = self.cat.spatial(prep).cloned() else {
                self.errors.push(CompileError::new(format!("'{prep}' is not a position VisionGuard can measure"), "use in, under, near or on"));
                return None;
            };
            let before = self.errors.len();
            let Some(ov) = self.ent_of(ov) else {
                if self.errors.len() == before { self.errors.push(CompileError::new(format!("'{prep}' must be followed by a zone or an object"), "e.g. 'in a height-zone'")); }
                return None;
            };
            if !matches!(self.entities[&sv].kind, NounKind::Class { .. }) {
                self.errors.push(CompileError::new(format!("a {} cannot move; only detected objects can be {prep} something", self.entities[&sv].noun), "start with an object such as 'a worker'"));
                return None;
            }
            if matches!(self.entities[&ov].kind, NounKind::Signal { .. }) {
                self.errors.push(CompileError::new(format!("a {} is a record, not a place", self.entities[&ov].noun), "use 'has a ...' for records"));
                return None;
            }
            return Some(Cond { kind: CondKind::Spatial { relation: prep.clone(), detector: sp.detector, metres: sp.default_metres }, subject: sv, object: ov, negated, min_seconds, exception: false });
        }
        let Some(rel) = self.cat.relation(verb).cloned() else {
            self.errors.push(CompileError::new(format!("VisionGuard cannot observe '{verb}'"), "use a relation from the catalog"));
            return None;
        };
        if !mods.is_empty() {
            self.errors.push(CompileError::new(format!("'{verb} ... {}' combines two conditions in one", mods[0].0), "write them as two conditions joined by 'and'"));
            return None;
        }
        let Some(ov) = obj.as_ref().and_then(|o| self.ent_of(o)) else {
            self.errors.push(CompileError::new(format!("'{verb}' needs an object VisionGuard detects"), "e.g. 'wears a harness'"));
            return None;
        };
        let okind = self.entities[&ov].kind.clone();
        if rel.detector == "signal" {
            let NounKind::Signal { source } = okind else {
                self.errors.push(CompileError::new(format!("'{verb}' links a zone or object to a record such as a permit"), "e.g. 'the hot-work-area has a permit'"));
                return None;
            };
            return Some(Cond { kind: CondKind::Signal { relation: verb.clone(), source }, subject: sv, object: ov, negated, min_seconds, exception: false });
        }
        if !matches!(okind, NounKind::Class { .. }) || !matches!(self.entities[&sv].kind, NounKind::Class { .. }) {
            self.errors.push(CompileError::new(format!("'{verb}' relates two detected objects"), "e.g. 'a worker wears a harness'"));
            return None;
        }
        if negated && min_seconds.is_none() {
            self.errors.push(CompileError::new(
                format!("'does not {verb} a {}' needs a duration", self.entities[&ov].noun),
                "a camera can miss an object for a frame (occlusion, glare); write e.g. 'for at least 3 seconds'"));
            return None;
        }
        Some(Cond { kind: CondKind::Association { relation: verb.clone(), detector: rel.detector }, subject: sv, object: ov, negated, min_seconds, exception: false })
    }
}

/// Compile one APE result (DRS text, paraphrase, TPTP) into an alert rule.
/// An "unless" clause, as (subject: zone noun / zone name / class noun, relation verb, signal noun).
pub type Exception = (String, String, String);

pub fn compile(cat: &Catalog, id: &str, text: &str, drs_text: &str, paraphrase: &str, ape_tptp: &str, exceptions: &[Exception]) -> Result<Rule, Vec<CompileError>> {
    let drs = crate::drs::parse(drs_text).map_err(|e| vec![CompileError::new(format!("cannot read APE's DRS: {e}"), "internal error")])?;
    let top = list(&drs.args().get(1).cloned().unwrap_or(Term::List(vec![]))).to_vec();
    let impls: Vec<&Term> = top.iter().map(|t| t.strip_index()).filter(|t| matches!(t.functor(), Some(("=>", 2)))).collect();
    if top.len() != 1 || impls.len() != 1 {
        return Err(vec![CompileError::new("a rule is one sentence 'If ... then VisionGuard raises a ... alert.'", "start with 'If'")]);
    }
    let (ante, cons) = (&impls[0].args()[0], &impls[0].args()[1]);
    let a = collect(ante);
    let c = collect(cons);
    let mut cx = Ctx { cat, entities: BTreeMap::new(), order: vec![], errors: vec![], warnings: vec![] };
    if let Some(o) = a.other.first() {
        let raw = o.to_string();
        if raw.contains(",leq,") || raw.contains(",less,") {
            cx.errors.push(CompileError::new("'at most' / 'less than' cannot trigger an alert", "use a minimum: 'for at least N seconds' or 'at least N workers'"));
        } else {
            cx.errors.push(CompileError::new("this sentence uses a construction the alert rules do not support", "use the rule patterns offered by the editor"));
        }
    }
    if let Some((v, p)) = a.props.first() { cx.errors.push(CompileError::new(format!("adjectives such as '{p}' are not conditions VisionGuard checks (on {v})"), "remove the adjective")); }
    for (v, (l, op, n)) in &a.objects { cx.declare(v, l, op, *n, Scope::Outer); }
    cx.apply_rels(&a);
    let mut conds = Vec::new();
    for ev in &a.events { if let Some(c) = cx.cond_of(&a, ev, false) { conds.push(c); } }
    for n in &a.negs {
        let nb = collect(n);
        if let Some(o) = nb.other.first() { cx.errors.push(CompileError::new(format!("unsupported construct inside a negation: {o}"), "negate one simple condition")); continue; }
        if !nb.rels.is_empty() { cx.errors.push(CompileError::new("scope zones to a camera outside the negation", "e.g. 'a worker is in a height-zone of CAM-03 and does not wear ...'")); continue; }
        if nb.events.len() != 1 { cx.errors.push(CompileError::new("negate exactly one condition", "e.g. 'does not wear a harness for at least 3 seconds'")); continue; }
        for (v, (l, op, k)) in &nb.objects { cx.declare(v, l, op, *k, Scope::Negated); }
        let mut merged = Box_ { objects: a.objects.clone(), ..Default::default() };
        merged.objects.extend(nb.objects.clone());
        merged.mods = nb.mods.clone();
        if let Some(c) = cx.cond_of(&merged, &nb.events[0], true) { conds.push(c); }
    }
    // consequent
    let alert = c.objects.values().find_map(|(l, _, _)| match cat.noun(l) { Some(NounKind::Alert { lsr, rule }) => Some((l.clone(), lsr, rule)), _ => None });
    let raise = c.events.iter().find(|(_, v, s, o)| v == "raise" && matches!(s.functor(), Some(("named", 1))) && o.is_some());
    let mut subject = None;
    match (&alert, raise) {
        (Some(_), Some((e, _, s, _))) => {
            let agent = s.args()[0].atom().unwrap_or("");
            if agent != cat.agent { cx.errors.push(CompileError::new(format!("only {} raises alerts", cat.agent), format!("write 'then {} raises ...'", cat.agent))); }
            for (p, x) in c.mods.get(e).into_iter().flatten() {
                if p != "for" { cx.errors.push(CompileError::new(format!("'raises ... {p} ...' is not supported"), "use 'for the worker' to name who the alert is about")); continue; }
                match cx.ent_of(x) {
                    Some(v) if cx.entities[&v].scope == Scope::Outer && cx.entities[&v].min_count.is_none() => subject = Some(v),
                    _ => cx.errors.push(CompileError::new("the alert can only be raised for an object or zone named in a positive condition", "e.g. '... for the worker'")),
                }
            }
        }
        _ => cx.errors.push(CompileError::new(format!("the then-part must be '{} raises a [severity] <rule>-alert'", cat.agent),
            format!("alerts: {}", cat.alerts.iter().map(|a| a.noun.as_str()).collect::<Vec<_>>().join(", ")))),
    }
    if c.events.len() > 1 || !c.negs.is_empty() || c.other.iter().any(|_| true) {
        cx.errors.push(CompileError::new("the then-part raises exactly one alert", "move further conditions into the if-part"));
    }
    let severity = c.props.iter().map(|(_, p)| p.clone()).find(|p| cat.severities.contains(p));
    if let Some((_, p)) = c.props.iter().find(|(_, p)| !cat.severities.contains(p)) {
        cx.errors.push(CompileError::new(format!("'{p}' is not a severity"), format!("use {}", cat.severities.join(", "))));
    }
    let primary_errors = !cx.errors.is_empty();
    if !primary_errors && !conds.iter().any(|c| !c.negated) {
        cx.errors.push(CompileError::new("a rule needs at least one positive condition", "say what VisionGuard must see, e.g. 'a worker is in a height-zone'"));
    }
    // every declared object must be used
    let used: BTreeSet<&String> = conds.iter().flat_map(|c| [&c.subject, &c.object]).collect();
    for v in cx.order.iter().filter(|_| !primary_errors) {
        if !used.contains(v) { cx.errors.push(CompileError::new(format!("the {} is not used by any condition", cx.entities[v].noun), "remove it or add a condition about it")); }
    }
    // a negated condition must be about something the if-part has already established
    // ("... unless the hot-work-area has a permit" with no hot-work-area in the if-part)
    let anchored: BTreeSet<String> = conds.iter().filter(|c| !c.negated).flat_map(|c| [c.subject.clone(), c.object.clone()]).collect();
    for c in conds.iter().filter(|c| c.negated) {
        let e = &cx.entities[&c.subject];
        if !primary_errors && !anchored.contains(&c.subject) {
            cx.errors.push(CompileError::new(format!("'the {}' is not mentioned in the if-part", e.noun),
                format!("first say what VisionGuard sees about it, e.g. 'a worker is in a {}', or refer to a zone already named", e.noun)));
        }
    }
    // "unless" clauses: they arrive as negated signal conditions; mark them
    for (es, ev, eo) in exceptions {
        let hit = conds.iter_mut().find(|c| c.negated && !c.exception && matches!(&c.kind, CondKind::Signal { relation, .. } if relation == ev)
            && cx.entities[&c.object].noun == *eo
            && (cx.entities[&c.subject].noun == *es || cx.entities[&c.subject].instance.as_deref() == Some(es.as_str())));
        match hit {
            Some(c) => c.exception = true,
            None => cx.errors.push(CompileError::new(format!("the exception 'unless {es} {} a {eo}' does not refer to anything in the if-part", cat.relation(ev).map(|r| r.third.as_str()).unwrap_or(ev)),
                "name a zone or object of the if-part, e.g. '... unless the restricted-area has a permit'")),
        }
    }
    if !cx.errors.is_empty() { return Err(cx.errors); }

    // scopes: class objects used in exactly one duration condition are existential inside it
    let mut uses: BTreeMap<String, usize> = BTreeMap::new();
    for c in &conds { *uses.entry(c.subject.clone()).or_default() += 1; *uses.entry(c.object.clone()).or_default() += 1; }
    for c in &conds {
        if c.negated || c.min_seconds.is_none() { continue; }
        let o = &c.object;
        if let Some(e) = cx.entities.get_mut(o) {
            if matches!(e.kind, NounKind::Class { .. }) && uses[o] == 1 && Some(o) != subject.as_ref() && e.scope == Scope::Outer { e.scope = Scope::Inner; }
        }
    }
    for e in cx.entities.values_mut() { if e.min_count.is_some() { e.scope = Scope::Inner; } }
    // readable variable names: W, W2, Z, ...
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut taken: BTreeSet<String> = BTreeSet::new();
    for v in &cx.order {
        let e = &cx.entities[v];
        let base: String = match &e.kind {
            NounKind::Zone => "Z".into(), NounKind::Signal { .. } => "R".into(),
            NounKind::Class { human: true, .. } => "W".into(),
            _ => e.noun.split('-').next().unwrap_or("X").chars().next().unwrap().to_ascii_uppercase().to_string(),
        };
        let mut n = base.clone(); let mut k = 2;
        while taken.contains(&n) || n == "T" || n == "S" { n = format!("{base}{k}"); k += 1; }
        taken.insert(n.clone());
        names.insert(v.clone(), n);
    }
    let mut entities: Vec<Entity> = cx.order.iter().map(|v| { let mut e = cx.entities[v].clone(); e.var = names[v].clone(); e }).collect();
    entities.sort_by_key(|e| match e.scope { Scope::Outer => 0, Scope::Inner => 1, Scope::Negated => 2 });
    let conditions: Vec<Cond> = conds.into_iter().map(|mut c| { c.subject = names[&c.subject].clone(); c.object = names[&c.object].clone(); c }).collect();
    let (anoun, lsr, rname) = alert.unwrap();
    let alert = AlertSpec { noun: anoun, lsr, rule: rname, severity, subject: subject.map(|s| names[&s].clone()) };
    let mut rule = Rule { id: id.into(), text: text.into(), paraphrase: paraphrase.trim().into(), readback: vec![], alert, entities, conditions,
        warnings: cx.warnings, fol: String::new(), ape_tptp: ape_tptp.trim().into() };
    rule.readback = readback(cat, &rule);
    rule.fol = fol(&rule);
    Ok(rule)
}

// ------------------------------------------------------------------------------------------- readback

fn gerund(v: &str) -> String {
    match v { "wear" => "wearing".into(), "carry" => "carrying".into(), "hold" => "holding".into(), "use" => "using".into(), "drive" => "driving".into(),
        "have" => "having".into(), v if v.ends_with('e') => format!("{}ing", &v[..v.len() - 1]), v => format!("{v}ing") }
}
fn secs(s: f64) -> String {
    if s >= 60.0 && (s / 60.0).fract() == 0.0 { let m = s / 60.0; format!("{} minute{}", m, if m == 1.0 { "" } else { "s" }) }
    else { format!("{} second{}", s, if s == 1.0 { "" } else { "s" }) }
}
fn an(w: &str) -> &'static str { if acewiki_rs::model::use_an(w) { "an" } else { "a" } }

pub fn readback(cat: &Catalog, r: &Rule) -> Vec<String> {
    let ent: BTreeMap<&str, &Entity> = r.entities.iter().map(|e| (e.var.as_str(), e)).collect();
    let dup = |e: &Entity| r.entities.iter().filter(|x| x.noun == e.noun).count() > 1;
    let the = |v: &str| {
        let e = ent[v];
        if let Some(n) = &e.instance { return n.clone(); }
        let base = if dup(e) { format!("the {} {}", e.noun, e.var) } else { format!("the {}", e.noun) };
        match &e.camera { Some(c) => format!("{base} of {c}"), None => base }
    };
    let mut out = vec![];
    let sev = r.alert.severity.as_deref().map(|s| format!("{s} ")).unwrap_or_default();
    let who = r.alert.subject.as_deref().map(|s| format!(" about {}", the(s))).unwrap_or_default();
    out.push(format!("{} raises a {sev}{} alert (IOGP Life-Saving Rule {}){who} when:", cat.agent, r.alert.rule, r.alert.lsr));
    for c in &r.conditions {
        let s = ent[c.subject.as_str()];
        let subj = match s.min_count { Some(k) => format!("at least {k} {}", cat.classes.iter().find(|x| x.noun == s.noun).map(|x| x.plural.clone()).unwrap_or(format!("{}s", s.noun))), None => the(&c.subject) };
        let o = ent[c.object.as_str()];
        let obj_ind = format!("{} {}", an(&o.noun), o.noun);
        let obj = if o.scope == Scope::Outer && !matches!(o.kind, NounKind::Zone) || dup(o) { the(&c.object) } else if matches!(o.kind, NounKind::Zone) { the(&c.object) } else { obj_ind.clone() };
        let plural = s.min_count.is_some();
        let dur = c.min_seconds.map(|d| format!(" for at least {}", secs(d))).unwrap_or_default();
        let line = match (&c.kind, c.negated) {
            (CondKind::Spatial { relation, metres, .. }, false) => {
                let m = if relation == "near" { metres.map(|m| format!(" (within {m} m)")).unwrap_or_default() } else { String::new() };
                format!("{subj} {} {relation} {obj}{m}{dur}", if plural { "are" } else { "is" })
            }
            (CondKind::Spatial { relation, .. }, true) => format!("{subj} {} not {relation} {obj}{dur}", if plural { "are" } else { "is" }),
            (CondKind::Association { relation, .. }, false) => format!("{subj} {} seen {} {obj_ind}{dur}", if plural { "are" } else { "is" }, gerund(relation)),
            (CondKind::Association { relation, .. }, true) => format!("{subj} {} not seen {} {obj_ind}{dur}", if plural { "are" } else { "is" }, gerund(relation)),
            (CondKind::Signal { source, .. }, false) => format!("{subj} has {obj_ind} (from the {source}){dur}"),
            (CondKind::Signal { source, .. }, true) if c.exception => format!("unless {subj} has {obj_ind} (from the {source})"),
            (CondKind::Signal { source, .. }, true) => format!("{subj} has no {} (according to the {source}){dur}", o.noun),
        };
        out.push(line);
    }
    out
}

// ------------------------------------------------------------------------------------------- FOL

fn pred(w: &str) -> String { w.replace('-', "_") }

/// Time-indexed first-order logic (TPTP fof). `within(S, T, D)` means T - D <= S <= T, so
/// `! [S] : (within(S, T, D) => phi(S))` says phi held throughout the D seconds up to T.
pub fn fol(r: &Rule) -> String {
    let ent: BTreeMap<&str, &Entity> = r.entities.iter().map(|e| (e.var.as_str(), e)).collect();
    let type_atom = |e: &Entity, v: &str| -> String {
        match &e.kind {
            NounKind::Class { detector, .. } => format!("{}({v})", pred(detector)),
            NounKind::Zone => {
                let mut a = vec![format!("{}({v})", pred(&e.noun))];
                if let Some(n) = &e.instance { a.push(format!("zone_id({v}, '{n}')")); }
                if let Some(c) = &e.camera { a.push(format!("camera_of({v}, '{c}')")); }
                a.join(" & ")
            }
            NounKind::Signal { .. } => format!("{}({v})", pred(&e.noun)),
            _ => format!("thing({v})"),
        }
    };
    let atom = |c: &Cond, subj: &str, obj: &str, t: &str| -> String {
        match &c.kind {
            CondKind::Spatial { relation, .. } => format!("{}({subj}, {obj}, {t})", pred(relation)),
            CondKind::Association { relation, .. } => format!("{}({subj}, {obj}, {t})", pred(relation)),
            CondKind::Signal { relation, .. } => format!("{}({subj}, {obj}, {t})", pred(relation)),
        }
    };
    let mut body: Vec<String> = Vec::new();
    for e in r.entities.iter().filter(|e| e.scope == Scope::Outer) { body.push(type_atom(e, &e.var)); }
    for c in &r.conditions {
        let s = ent[c.subject.as_str()];
        let o = ent[c.object.as_str()];
        let t = if c.min_seconds.is_some() { "S" } else { "T" };
        let mut phi = if let Some(k) = s.min_count {
            let vs: Vec<String> = (1..=k).map(|i| format!("{}{i}", s.var)).collect();
            let mut parts: Vec<String> = Vec::new();
            for i in 0..vs.len() { for j in i + 1..vs.len() { parts.push(format!("{} != {}", vs[i], vs[j])); } }
            for v in &vs { parts.push(type_atom(s, v)); parts.push(atom(c, v, &c.object, t)); }
            format!("? [{}] : ({})", vs.join(", "), parts.join(" & "))
        } else { atom(c, &c.subject, &c.object, t) };
        if o.scope != Scope::Outer && s.min_count.is_none() {
            phi = format!("? [{}] : ({} & {phi})", o.var, type_atom(o, &o.var));
        }
        if c.negated { phi = format!("~ ({phi})"); }
        if let Some(d) = c.min_seconds {
            phi = format!("! [S] : (within(S, T, {}) => {phi})", fmt_num(d));
        }
        body.push(phi);
    }
    let mut outer: Vec<String> = vec!["T".into()];
    outer.extend(r.entities.iter().filter(|e| e.scope == Scope::Outer).map(|e| e.var.clone()));
    let about = r.alert.subject.clone()
        .or_else(|| r.entities.iter().find(|e| e.scope == Scope::Outer && matches!(e.kind, NounKind::Zone)).map(|e| e.var.clone()))
        .unwrap_or_else(|| "scene".into());
    let lsr = pred(r.alert.noun.trim_end_matches("-alert"));
    let head = format!("alert({lsr}, {}, {about}, T)", r.alert.severity.as_deref().unwrap_or("unspecified"));
    let name = format!("lsr{}_{}", r.alert.lsr, r.id.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect::<String>());
    format!("fof({name}, axiom,\n  ! [{}] :\n    ( ( {} )\n      => {head} )).", outer.join(", "), body.join("\n        & "))
}

fn fmt_num(d: f64) -> String { if d.fract() == 0.0 { format!("{}", d as i64) } else { format!("{d}") } }
