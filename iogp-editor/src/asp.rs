//! Answer set programming (clingo, via the `clingox` crate) for **offline** rule work only.
//! Nothing here is linked into `iogp-runtime`; inference never runs a solver.
//!
//! * [`ruleset_lp`] — the approved rules as a clingo program (`/export/rules.lp`). Needs no clingo
//!   to produce; the detection team can run it with plain `clingo`.
//! * [`review`] for approvers — the smallest example scene that
//!   fires each rule, rules that can never fire, rules implied by other rules, and IOGP
//!   Life-Saving Rules with no live rule. [`derive`] evaluates frames with clingo, and the tests
//!   use it to check that the encoding derives exactly what the reference engine derives.
//!
//! Encoding (times in integer milliseconds):
//! ```text
//! frame(T).  obj(Id, Type, T).  cam(Id, Camera, T).  fact(Rel, A, B, T).     % believed input
//! prev(P, S): P is the frame before S
//! r<i>_b(V.., T)     outer bindings (candidates by type, named instance, camera)
//! r<i>_h<j>(V.., T)  condition j holds (plain / inner existential / count; negation as failure)
//! r<i>_r<j>(V.., T, S)  condition j has held since S without a break (run start, carried frame to frame)
//! r<i>_k<j>(V.., T)  condition j satisfied, including "for at least D" (T - S >= D)
//! derived(Rule, Subject, T)
//! ```
//! Input facts are *believed* facts: the runtime's evidence smoothing (grace period) is applied
//! before evaluation ([`believed`]), exactly as the runtime does, so timers inside are exact.

use crate::catalog::Catalog;
use crate::compile::{Entity, Rule, Scope};
use iogp_runtime::{Frame, NounKind, EPS};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write;

pub fn q(s: &str) -> String { format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")) }

fn type_key(e: &Entity) -> String {
    match &e.kind {
        NounKind::Class { detector, .. } => format!("c:{detector}"),
        NounKind::Zone => format!("z:{}", e.noun),
        NounKind::Signal { .. } => format!("s:{}", e.noun),
        _ => "none".into(),
    }
}

pub const PRELUDE: &str = "% iogp alert rules as an answer set program (clingo 5).
% Input, per evaluated frame T (milliseconds): frame(T). obj(Id, Type, T). cam(Id, Camera, T). fact(Rel, A, B, T).
% Types: \"c:<detector label>\" for detected classes, \"z:<zone type>\" for zones, \"s:<record>\" for signals.
% Facts are believed facts: apply the grace period first (a fact seen at S is believed until S + grace).
prev(P, S) :- frame(S), P = #max { Q : frame(Q), Q < S }, P != #inf.
#show derived/3.
";

/// One rule as clingo rules with predicate prefix `p`.
pub fn rule_lp(r: &Rule, p: &str) -> String {
    let mut o = String::new();
    let outer: Vec<usize> = r.entities.iter().enumerate().filter(|(_, e)| e.scope == Scope::Outer).map(|(i, _)| i).collect();
    let var = |ei: usize| outer.iter().position(|&x| x == ei).map(|k| format!("V{k}"));
    let bvars = outer.iter().enumerate().map(|(k, _)| format!("V{k}")).collect::<Vec<_>>().join(",");
    let a = |t: &str| if bvars.is_empty() { t.to_string() } else { format!("{bvars},{t}") };
    let _ = writeln!(o, "% {} — {}", r.id, r.text);
    for (k, e) in r.entities.iter().enumerate() {
        let mut body = format!("obj(X,{},T)", q(&type_key(e)));
        if let Some(i) = &e.instance { let _ = write!(body, ", X = {}", q(i)); }
        if let Some(c) = &e.camera { let _ = write!(body, ", cam(X,{},T)", q(c)); }
        let _ = writeln!(o, "{p}_c{k}(X,T) :- {body}.");
    }
    let cands = outer.iter().enumerate().map(|(k, &ei)| format!("{p}_c{ei}(V{k},T)")).collect::<Vec<_>>();
    let _ = writeln!(o, "{p}_b({}) :- {}frame(T).", a("T"), cands.iter().map(|c| format!("{c}, ")).collect::<String>());
    let mut oks = vec![];
    for (j, c) in r.conditions.iter().enumerate() {
        let ix = |v: &str| r.entities.iter().position(|e| e.var == v).unwrap();
        let (s, ob) = (ix(&c.subject), ix(&c.object));
        let rel = q(c.kind.relation());
        let pos = format!("{p}_p{j}({})", a("T"));
        if let Some(n) = r.entities[s].min_count {
            let vo = var(ob).unwrap_or("_".into());
            let _ = writeln!(o, "{pos} :- {p}_b({}), #count {{ X : {p}_c{s}(X,T), fact({rel},X,{vo},T) }} >= {n}.", a("T"));
        } else if r.entities[ob].scope != Scope::Outer {
            let vs = var(s).unwrap_or("_".into());
            let _ = writeln!(o, "{p}_x{j}(S,T) :- {p}_c{ob}(Y,T), fact({rel},S,Y,T).");
            let _ = writeln!(o, "{pos} :- {p}_b({}), {p}_x{j}({vs},T).", a("T"));
        } else {
            let (vs, vo) = (var(s).unwrap(), var(ob).unwrap());
            let _ = writeln!(o, "{pos} :- {p}_b({}), fact({rel},{vs},{vo},T).", a("T"));
        }
        if c.negated { let _ = writeln!(o, "{p}_h{j}({}) :- {p}_b({}), not {pos}.", a("T"), a("T")); }
        else { let _ = writeln!(o, "{p}_h{j}({}) :- {pos}.", a("T")); }
        match c.min_seconds {
            Some(d) => {
                let dms = (d * 1000.0).round() as i64;
                let _ = writeln!(o, "{p}_q{j}({}) :- {p}_h{j}({}), prev(P,T), {p}_h{j}({}).", a("T"), a("T"), a("P"));
                let _ = writeln!(o, "{p}_r{j}({},T) :- {p}_h{j}({}), not {p}_q{j}({}).", a("T"), a("T"), a("T"));
                let _ = writeln!(o, "{p}_r{j}({},S) :- {p}_q{j}({}), prev(P,T), {p}_r{j}({},S).", a("T"), a("T"), a("P"));
                let _ = writeln!(o, "{p}_k{j}({}) :- {p}_r{j}({},S), T - S >= {dms}.", a("T"), a("T"));
            }
            None => { let _ = writeln!(o, "{p}_k{j}({}) :- {p}_h{j}({}).", a("T"), a("T")); }
        }
        oks.push(format!("{p}_k{j}({})", a("T")));
    }
    let subj = match &r.alert.subject {
        Some(v) => r.entities.iter().position(|e| &e.var == v).and_then(var).unwrap_or_else(|| q("scene")),
        None => outer.iter().position(|&ei| matches!(r.entities[ei].kind, NounKind::Zone)).map(|k| format!("V{k}")).unwrap_or_else(|| q("scene")),
    };
    let _ = writeln!(o, "derived({},{subj},T) :- {p}_b({}){}.", q(&r.id), a("T"), oks.iter().map(|k| format!(", {k}")).collect::<String>());
    o
}

/// The whole rule set as one clingo program (prelude + one block per rule).
pub fn ruleset_lp(rules: &[Rule]) -> String {
    let mut s = PRELUDE.to_string();
    for (i, r) in rules.iter().enumerate() { s.push('\n'); s += &rule_lp(r, &format!("r{i}")); }
    s
}

/// Frames → believed input facts (ms times), applying evidence smoothing exactly as the runtime
/// does: objects seen within `grace` stay candidates (under their primary type, without a camera
/// unless seen in this frame), facts seen within `grace` stay believed.
pub fn believed(frames: &[Frame], grace: f64) -> String {
    let mut out = String::new();
    let mut recent: HashMap<(String, String), f64> = HashMap::new();
    let mut seen: HashMap<(String, String, String), f64> = HashMap::new();
    for f in frames {
        let t = f.t;
        let tm = (t * 1000.0).round() as i64;
        let _ = writeln!(out, "frame({tm}).");
        let mut objs: BTreeSet<(String, String)> = BTreeSet::new();
        let mut cams: BTreeSet<(String, String)> = BTreeSet::new();
        for o in &f.objects {
            for (k, ty) in [("c", &o.class), ("z", &o.zone), ("s", &o.signal)] { if let Some(ty) = ty { objs.insert((o.id.clone(), format!("{k}:{ty}"))); } }
            if let Some(c) = o.camera.as_ref().or(f.camera.as_ref()) { cams.insert((o.id.clone(), c.clone())); }
        }
        for ((ty, id), last) in &recent { if t - last <= grace + EPS { objs.insert((id.clone(), ty.clone())); } }
        let mut facts: BTreeSet<(String, String, String)> = f.facts.iter().cloned().collect();
        for (fa, last) in &seen { if t - last <= grace + EPS { facts.insert(fa.clone()); } }
        for (id, ty) in &objs { let _ = writeln!(out, "obj({},{},{tm}).", q(id), q(ty)); }
        for (id, c) in &cams { let _ = writeln!(out, "cam({},{},{tm}).", q(id), q(c)); }
        for (r, a, b) in &facts { let _ = writeln!(out, "fact({},{},{},{tm}).", q(r), q(a), q(b)); }
        for o in &f.objects {
            let ty = o.class.as_ref().map(|c| format!("c:{c}")).or_else(|| o.zone.as_ref().map(|z| format!("z:{z}"))).or_else(|| o.signal.as_ref().map(|s| format!("s:{s}")));
            if let Some(ty) = ty { recent.insert((ty, o.id.clone()), t); }
        }
        recent.retain(|_, last| t - *last <= grace);
        for fa in &f.facts { seen.insert(fa.clone(), t); }
        seen.retain(|_, last| t - *last <= grace);
    }
    out
}

/// Human-readable form of a relation between two objects.
fn rel_phrase(cat: &Catalog, rel: &str) -> String {
    if cat.spatial.iter().any(|s| s.prep == rel) { return format!("is {rel}"); }
    cat.relations.iter().find(|r| r.verb == rel).map(|r| r.third.clone()).unwrap_or_else(|| rel.to_string())
}

/// The objects a scene may contain: enough of each type for every rule involved.
#[derive(Debug, Default)]
pub struct Universe { pub objs: Vec<(String, String, Option<String>, String)>, pub rels: BTreeSet<String>, pub step_ms: i64, pub horizon_ms: i64 }

impl Universe {
    pub fn for_rules(rules: &[&Rule]) -> Universe {
        let mut u = Universe::default();
        let mut durs = vec![];
        for r in rules {
            let mut count: BTreeMap<String, usize> = BTreeMap::new();
            for e in &r.entities {
                let ty = type_key(e);
                if let Some(i) = &e.instance {
                    if !u.objs.iter().any(|o| &o.0 == i) { u.objs.push((i.clone(), ty, e.camera.clone(), e.noun.clone())); }
                    continue;
                }
                for _ in 0..e.min_count.unwrap_or(1) {
                    let k = count.entry(format!("{ty}@{:?}", e.camera)).or_default();
                    *k += 1;
                    let base = match &e.camera { Some(c) => format!("{}@{c}", e.noun), None => e.noun.clone() };
                    let id = if *k == 1 { base } else { format!("{base}-{k}") };
                    if !u.objs.iter().any(|o| o.0 == id) { u.objs.push((id, ty.clone(), e.camera.clone(), e.noun.clone())); }
                }
            }
            for c in &r.conditions { u.rels.insert(c.kind.relation().to_string()); if let Some(d) = c.min_seconds { durs.push((d * 1000.0).round() as i64); } }
        }
        u.step_ms = if durs.iter().all(|d| d % 1000 == 0) { 1000 } else { 100 };
        u.horizon_ms = durs.iter().copied().max().unwrap_or(0);
        u.horizon_ms = (u.horizon_ms + u.step_ms - 1) / u.step_ms * u.step_ms;
        u
    }
    fn lp(&self) -> String {
        let mut s = String::new();
        let mut t = 0;
        while t <= self.horizon_ms { let _ = writeln!(s, "frame({t})."); t += self.step_ms; }
        for (id, ty, cam, _) in &self.objs {
            let _ = writeln!(s, "u({},{}).", q(id), q(ty));
            if let Some(c) = cam { let _ = writeln!(s, "ucam({},{}).", q(id), q(c)); }
        }
        for r in &self.rels { let _ = writeln!(s, "urel({}).", q(r)); }
        s += "{ obj(X,Ty,T) : u(X,Ty), frame(T) }.\ncam(X,C,T) :- obj(X,_,T), ucam(X,C).\n";
        s += "{ fact(R,A,B,T) : urel(R), u(A,_), u(B,_), A != B, frame(T) }.\n";
        s += "#minimize { 1,X,T : obj(X,_,T) ; 1,R,A,B,T : fact(R,A,B,T) }.\n#show obj/3. #show fact/4.\n";
        let _ = writeln!(s, "horizon({}).", self.horizon_ms);
        s
    }
}

/// A smallest scene (objects and facts over time) in which a rule fires at the last frame.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Witness { pub rule: String, pub seconds: f64, pub lines: Vec<String> }

#[derive(Debug, Clone, serde::Serialize)]
pub struct Implication { pub a: String, pub b: String }

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Review {
    pub witnesses: Vec<Witness>,
    /// rules that no scene can fire
    pub dead: Vec<String>,
    /// whenever `a` fires, `b` fires too (within scenes of the bounded universe)
    pub implications: Vec<Implication>,
    /// IOGP Life-Saving Rules (number, name) with no rule
    pub uncovered_lsr: Vec<(u8, String)>,
    /// named zones of the site that no rule mentions
    pub unused_zones: Vec<String>,
    pub bound: String,
}

fn describe(cat: &Catalog, u: &Universe, atoms: &[(String, Vec<String>, i64)]) -> Vec<String> {
    let nframes = (u.horizon_ms / u.step_ms + 1) as usize;
    let span = |ts: &BTreeSet<i64>| -> String {
        if ts.len() == nframes { return if nframes == 1 { "now".into() } else { "throughout".into() }; }
        let v: Vec<i64> = ts.iter().copied().collect();
        let contiguous = v.windows(2).all(|w| w[1] - w[0] == u.step_ms);
        let f = |ms: i64| format!("{}", ms as f64 / 1000.0);
        if contiguous { if v.len() == 1 { format!("at {} s", f(v[0])) } else { format!("{}–{} s", f(v[0]), f(*v.last().unwrap())) } }
        else { format!("at {} s", v.iter().map(|x| f(*x)).collect::<Vec<_>>().join(", ")) }
    };
    let mut objs: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    let mut facts: BTreeMap<(String, String, String), BTreeSet<i64>> = BTreeMap::new();
    for (name, args, t) in atoms {
        match name.as_str() {
            "obj" => { objs.entry(args[0].clone()).or_default().insert(*t); }
            "fact" => { facts.entry((args[1].clone(), args[0].clone(), args[2].clone())).or_default().insert(*t); }
            _ => {}
        }
    }
    let mut lines = vec![];
    for (id, ts) in &objs { lines.push(format!("{id} is seen ({})", span(ts))); }
    for ((a, r, b), ts) in &facts { lines.push(format!("{a} {} {b} ({})", rel_phrase(cat, r), span(ts))); }
    let absent: Vec<&str> = u.objs.iter().filter(|o| !objs.contains_key(&o.0)).map(|o| o.0.as_str()).collect();
    if !absent.is_empty() { lines.push(format!("never seen: {}", absent.join(", "))); }
    lines
}

mod solve {
    use clingox::prelude::*;

    pub(super) type Atoms = Vec<(String, Vec<String>, i64)>;

    fn atom_of(s: &Symbol) -> Option<(String, Vec<String>, i64)> {
        let name = s.name()?.to_string();
        let args = s.arguments()?;
        let (last, rest) = args.split_last()?;
        let t = last.as_number()? as i64;
        Some((name, rest.iter().map(|a| a.as_string().map(str::to_string).unwrap_or_else(|| a.to_string())).collect(), t))
    }

    /// Ground and solve; with `optimize`, the optimal model. `None` = unsatisfiable.
    pub(super) fn run(program: &str, optimize: bool) -> Result<Option<Atoms>, String> {
        let e = |x: clingox::Error| x.to_string();
        let mut ctl = if optimize { Control::with_args(["--opt-mode=opt"]).map_err(e)? } else { Control::new().map_err(e)? };
        ctl.add_base(program).map_err(e)?;
        ctl.ground(&[Part::base()]).map_err(e)?;
        if optimize {
            let (_, models) = ctl.solve_all().map_err(e)?;
            Ok(models.last().map(|m| m.symbols().iter().filter_map(atom_of).collect()))
        } else {
            match ctl.solve_first().map_err(e)? {
                Outcome::Sat(m, _) => Ok(Some(m.symbols().iter().filter_map(atom_of).collect())),
                _ => Ok(None),
            }
        }
    }
}

/// Evaluate frames with clingo: the (rule, subject) pairs derived at each frame time (ms).
pub fn derive(rules: &[Rule], frames: &[Frame], grace: f64) -> Result<BTreeMap<i64, BTreeSet<(String, String)>>, String> {
    let prog = format!("{}\n{}", ruleset_lp(rules), believed(frames, grace));
    let atoms = solve::run(&prog, false)?.ok_or("the frame program has no answer set")?;
    let mut out: BTreeMap<i64, BTreeSet<(String, String)>> = BTreeMap::new();
    for (n, a, t) in atoms { if n == "derived" { out.entry(t).or_default().insert((a[0].clone(), a[1].clone())); } }
    Ok(out)
}

/// A smallest scene in which `r` fires; `None` if no scene can fire it.
pub fn witness(cat: &Catalog, r: &Rule) -> Result<Option<Witness>, String> {
    let u = Universe::for_rules(&[r]);
    let prog = format!("{}\n{}\n{}\n:- horizon(H), not derived({},_,H).\n", PRELUDE.replace("#show derived/3.", ""), rule_lp(r, "r0"), u.lp(), q(&r.id));
    Ok(solve::run(&prog, true)?.map(|atoms| Witness { rule: r.id.clone(), seconds: u.horizon_ms as f64 / 1000.0, lines: describe(cat, &u, &atoms) }))
}

/// Is every scene that fires `a` also one that fires `b`? (bounded: the universe of both rules)
pub fn implies(a: &Rule, b: &Rule) -> Result<bool, String> {
    let u = Universe::for_rules(&[a, b]);
    let prog = format!("{}\n{}\n{}\n{}\n:- horizon(H), not derived({},_,H).\n:- horizon(H), derived({},_,H).\n",
        PRELUDE.replace("#show derived/3.", ""), rule_lp(a, "ra"), rule_lp(b, "rb"), u.lp().replace("#minimize { 1,X,T : obj(X,_,T) ; 1,R,A,B,T : fact(R,A,B,T) }.\n", ""), q(&a.id), q(&b.id));
    Ok(solve::run(&prog, false)?.is_none())
}

/// Full review of a rule set for approvers.
pub fn review(cat: &Catalog, rules: &[Rule]) -> Result<Review, String> {
    let mut rv = Review { bound: "scenes with at most the objects the rules mention (counts included), over the longest duration involved".into(), ..Default::default() };
    for r in rules {
        match witness(cat, r)? { Some(w) => rv.witnesses.push(w), None => rv.dead.push(r.id.clone()) }
    }
    for a in rules {
        for b in rules {
            if a.id == b.id || rv.dead.contains(&a.id) || rv.dead.contains(&b.id) { continue; }
            if implies(a, b)? { rv.implications.push(Implication { a: a.id.clone(), b: b.id.clone() }); }
        }
    }
    for al in &cat.alerts { if !rules.iter().any(|r| r.alert.lsr == al.lsr) { rv.uncovered_lsr.push((al.lsr, al.rule.clone())); } }
    for z in &cat.zone_instances {
        let used = rules.iter().any(|r| r.entities.iter().any(|e| e.instance.as_deref() == Some(z.name.as_str()) || (e.instance.is_none() && matches!(e.kind, NounKind::Zone) && e.noun == z.zone && e.camera.as_ref().is_none_or(|c| Some(c) == z.camera.as_ref()))));
        if !used { rv.unused_zones.push(z.name.clone()); }
    }
    Ok(rv)
}
