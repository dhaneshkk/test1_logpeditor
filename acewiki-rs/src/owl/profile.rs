//! OWL 2 EL / RL profile checks for a single sentence's axioms, as AceWiki applies
//! OWL API's `OWL2ELProfile` / `OWL2RLProfile` (violations "Use of undeclared …" ignored).
//! Both profiles also impose the OWL 2 DL global restriction on simple properties.

use super::{Axiom, Prop, CE, THING};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile { EL, RL, Full }

impl Profile {
    pub fn parse(s: &str) -> Profile {
        match s.to_ascii_lowercase().as_str() { "owl2el" => Profile::EL, "owl2rl" => Profile::RL, _ => Profile::Full }
    }
    pub fn name(&self) -> &'static str { match self { Profile::EL => "OWL 2 EL", Profile::RL => "OWL 2 RL", Profile::Full => "OWL 2 Full" } }
}

/// Returns the list of violations (empty = in profile).
pub fn check(profile: Profile, axioms: &[Axiom]) -> Vec<String> {
    let mut v = Vec::new();
    if profile == Profile::Full { return v; }
    dl_simple_properties(axioms, &mut v);
    for a in axioms {
        match profile {
            Profile::RL => rl_axiom(a, &mut v),
            Profile::EL => el_axiom(a, &mut v),
            Profile::Full => {}
        }
    }
    v
}

// ------------------------------------------------------------------------------ DL restriction

fn dl_simple_properties(axioms: &[Axiom], v: &mut Vec<String>) {
    let mut non_simple: HashSet<String> = HashSet::new();
    for a in axioms {
        match a {
            Axiom::Transitive(p) => { non_simple.insert(p.base().to_string()); }
            Axiom::SubPropertyChain(ps, q) if ps.len() > 1 => { non_simple.insert(q.base().to_string()); }
            _ => {}
        }
    }
    // propagate upwards through the sub-property hierarchy
    loop {
        let before = non_simple.len();
        for a in axioms {
            if let Axiom::SubPropertyOf(p, q) = a { if non_simple.contains(p.base()) { non_simple.insert(q.base().to_string()); } }
            if let Axiom::EquivalentProperties(ps) = a { if ps.iter().any(|p| non_simple.contains(p.base())) { for p in ps { non_simple.insert(p.base().to_string()); } } }
            if let Axiom::InverseProperties(p, q) = a { if non_simple.contains(p.base()) || non_simple.contains(q.base()) { non_simple.insert(p.base().to_string()); non_simple.insert(q.base().to_string()); } }
        }
        if non_simple.len() == before { break; }
    }
    if non_simple.is_empty() { return; }
    let ns = |p: &Prop| non_simple.contains(p.base());
    fn ce_ns(c: &CE, ns: &dyn Fn(&Prop) -> bool) -> bool {
        match c {
            CE::Min(_, p, f) | CE::Max(_, p, f) | CE::Exact(_, p, f) => ns(p) || ce_ns(f, ns),
            CE::HasSelf(p) => ns(p),
            CE::And(xs) | CE::Or(xs) => xs.iter().any(|x| ce_ns(x, ns)),
            CE::Not(x) | CE::Some(_, x) | CE::All(_, x) => ce_ns(x, ns),
            _ => false,
        }
    }
    for a in axioms {
        let bad = match a {
            Axiom::Functional(p) | Axiom::InverseFunctional(p) | Axiom::Irreflexive(p) | Axiom::Asymmetric(p) => ns(p),
            Axiom::DisjointProperties(ps) => ps.iter().any(|p| ns(p)),
            Axiom::SubClassOf(x, y) => ce_ns(x, &ns) || ce_ns(y, &ns),
            Axiom::EquivalentClasses(xs) | Axiom::DisjointClasses(xs) => xs.iter().any(|x| ce_ns(x, &ns)),
            Axiom::ClassAssertion(c, _) | Axiom::Domain(_, c) | Axiom::Range(_, c) => ce_ns(c, &ns),
            _ => false,
        };
        if bad { v.push(format!("Use of non-simple property in restriction: {a:?}")); }
    }
}

// -------------------------------------------------------------------------------------- RL

fn rl_sub(c: &CE) -> bool {
    match c {
        CE::Class(i) => i != THING,
        CE::And(xs) | CE::Or(xs) => xs.iter().all(rl_sub),
        CE::OneOf(_) => true,
        CE::Some(_, f) => f.is_thing() || rl_sub(f),
        CE::HasValue(..) => true,
        _ => false,
    }
}
fn rl_super(c: &CE) -> bool {
    match c {
        CE::Class(i) => i != THING,
        CE::And(xs) => xs.iter().all(rl_super),
        CE::Not(x) => rl_sub(x),
        CE::All(_, f) => rl_super(f),
        CE::HasValue(..) => true,
        CE::Max(n, _, f) => *n <= 1 && (f.is_thing() || rl_sub(f)),
        _ => false,
    }
}
fn rl_equiv(c: &CE) -> bool {
    match c {
        CE::Class(i) => i != THING,
        CE::And(xs) => xs.iter().all(rl_equiv),
        CE::HasValue(..) => true,
        _ => false,
    }
}
fn rl_axiom(a: &Axiom, v: &mut Vec<String>) {
    let ok = match a {
        Axiom::SubClassOf(x, y) => rl_sub(x) && rl_super(y),
        Axiom::EquivalentClasses(xs) => xs.iter().all(rl_equiv),
        Axiom::DisjointClasses(xs) => xs.iter().all(rl_sub),
        Axiom::ClassAssertion(c, _) => rl_super(c),
        Axiom::Domain(_, c) | Axiom::Range(_, c) => rl_super(c),
        Axiom::DisjointUnion(..) | Axiom::Reflexive(_) => false,
        Axiom::Other(n, _) => !n.starts_with("DLSafeRule") && n != "HasKey",
        _ => true,
    };
    if !ok { v.push(format!("Not in OWL 2 RL: {a:?}")); }
}

// -------------------------------------------------------------------------------------- EL

fn el_prop(p: &Prop) -> bool { matches!(p, Prop::Named(_)) }
fn el_ce(c: &CE) -> bool {
    match c {
        CE::Class(_) => true,
        CE::And(xs) => xs.iter().all(el_ce),
        CE::Some(p, f) => el_prop(p) && el_ce(f),
        CE::HasValue(p, _) | CE::HasSelf(p) => el_prop(p),
        CE::OneOf(is) => is.len() == 1,
        _ => false,
    }
}
fn el_axiom(a: &Axiom, v: &mut Vec<String>) {
    let ok = match a {
        Axiom::SubClassOf(x, y) => el_ce(x) && el_ce(y),
        Axiom::EquivalentClasses(xs) | Axiom::DisjointClasses(xs) => xs.iter().all(el_ce),
        Axiom::ClassAssertion(c, _) => el_ce(c),
        Axiom::PropertyAssertion(p, ..) | Axiom::NegativePropertyAssertion(p, ..) => el_prop(p),
        Axiom::SubPropertyOf(p, q) => el_prop(p) && el_prop(q),
        Axiom::SubPropertyChain(ps, q) => ps.iter().all(el_prop) && el_prop(q),
        Axiom::EquivalentProperties(ps) => ps.iter().all(el_prop),
        Axiom::Domain(p, c) | Axiom::Range(p, c) => el_prop(p) && el_ce(c),
        Axiom::Transitive(p) | Axiom::Reflexive(p) => el_prop(p),
        Axiom::SameIndividual(_) | Axiom::DifferentIndividuals(_) | Axiom::Declaration(..) => true,
        _ => false,
    };
    if !ok { v.push(format!("Not in OWL 2 EL: {a:?}")); }
}
