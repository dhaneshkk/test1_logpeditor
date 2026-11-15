//! Native OWL 2 reasoners for AceWiki's queries (`AceWikiReasoner` contract):
//! consistency, types(ind) [all], instances(C) [all], super/sub classes [strict, all],
//! satisfiability, and question answering (instances of a class expression).
//!
//! * [`RlReasoner`] — OWL 2 RL: forward chaining over named individuals (OWL 2 RL/RDF rules
//!   restricted to the functional-syntax axioms APE produces), with equality (union-find) and
//!   AceWiki's unique-name assumption. Class subsumption / satisfiability are decided with one
//!   *prototype individual* per class, layered on the materialised ABox (prototypes never feed
//!   back into the named individuals' entailments).
//! * [`ElReasoner`] — OWL 2 EL: completion-rule classification (Baader/Brandt/Lutz EL++ rules
//!   CR1–CR6, CR10–CR11) where individuals are nominals {a}; ABox assertions become
//!   {a} ⊑ C and {a} ⊑ ∃r.{b}.
//!
//! Anything outside the active profile is reported by [`Reasoner::unsupported`] so the caller
//! can route the ontology to an OWLlink server instead.

use super::{Axiom, Prop, CE, NOTHING, THING};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

pub trait Reasoner: Send {
    fn name(&self) -> &'static str;
    /// Axioms the engine cannot handle soundly/completely (empty = fully supported).
    fn unsupported(&self) -> Vec<String>;
    fn is_consistent(&mut self) -> bool;
    fn types(&mut self, ind: &str) -> BTreeSet<String>;
    fn instances(&mut self, c: &CE) -> BTreeSet<String>;
    fn super_classes(&mut self, c: &str) -> BTreeSet<String>;
    fn sub_classes(&mut self, c: &str) -> BTreeSet<String>;
    fn is_satisfiable(&mut self, c: &str) -> bool;
}

/// Collects named classes / individuals / properties of an axiom set.
#[derive(Default, Debug, Clone)]
pub struct Signature { pub classes: BTreeSet<String>, pub individuals: BTreeSet<String>, pub props: BTreeSet<String> }
impl Signature {
    pub fn of(axioms: &[Axiom]) -> Signature {
        let mut s = Signature::default();
        for a in axioms { s.axiom(a); }
        s.classes.remove(THING); s.classes.remove(NOTHING);
        s
    }
    fn p(&mut self, p: &Prop) { self.props.insert(p.base().to_string()); }
    fn c(&mut self, c: &CE) {
        match c {
            CE::Class(i) => { self.classes.insert(i.clone()); }
            CE::And(xs) | CE::Or(xs) => for x in xs { self.c(x) },
            CE::Not(x) => self.c(x),
            CE::OneOf(is) => for i in is { self.individuals.insert(i.clone()); },
            CE::Some(p, x) | CE::All(p, x) | CE::Min(_, p, x) | CE::Max(_, p, x) | CE::Exact(_, p, x) => { self.p(p); self.c(x) }
            CE::HasValue(p, i) => { self.p(p); self.individuals.insert(i.clone()); }
            CE::HasSelf(p) => self.p(p),
            CE::Data(_) => {}
        }
    }
    fn axiom(&mut self, a: &Axiom) {
        match a {
            Axiom::Declaration(t, i) => match t.as_str() {
                "Class" => { self.classes.insert(i.clone()); }
                "NamedIndividual" => { self.individuals.insert(i.clone()); }
                "ObjectProperty" => { self.props.insert(i.clone()); }
                _ => {}
            },
            Axiom::SubClassOf(x, y) => { self.c(x); self.c(y) }
            Axiom::EquivalentClasses(xs) | Axiom::DisjointClasses(xs) => for x in xs { self.c(x) },
            Axiom::DisjointUnion(c, xs) => { self.classes.insert(c.clone()); for x in xs { self.c(x) } }
            Axiom::ClassAssertion(c, i) => { self.c(c); self.individuals.insert(i.clone()); }
            Axiom::PropertyAssertion(p, a, b) | Axiom::NegativePropertyAssertion(p, a, b) => { self.p(p); self.individuals.insert(a.clone()); self.individuals.insert(b.clone()); }
            Axiom::SameIndividual(is) | Axiom::DifferentIndividuals(is) => for i in is { self.individuals.insert(i.clone()); },
            Axiom::SubPropertyOf(p, q) | Axiom::InverseProperties(p, q) => { self.p(p); self.p(q) }
            Axiom::SubPropertyChain(ps, q) => { for p in ps { self.p(p) } self.p(q) }
            Axiom::EquivalentProperties(ps) | Axiom::DisjointProperties(ps) => for p in ps { self.p(p) },
            Axiom::Domain(p, c) | Axiom::Range(p, c) => { self.p(p); self.c(c) }
            Axiom::Functional(p) | Axiom::InverseFunctional(p) | Axiom::Reflexive(p) | Axiom::Irreflexive(p)
            | Axiom::Symmetric(p) | Axiom::Asymmetric(p) | Axiom::Transitive(p) => self.p(p),
            Axiom::Other(..) => {}
        }
    }
}

// =====================================================================================  RL

#[derive(Clone, Default)]
struct State {
    n: usize,
    parent: Vec<usize>,
    types: Vec<HashSet<String>>,
    /// out[prop][a] = {b}, inn[prop][b] = {a}   (only named properties; inverses via swap)
    out: HashMap<String, HashMap<usize, BTreeSet<usize>>>,
    inn: HashMap<String, HashMap<usize, BTreeSet<usize>>>,
    clash: Option<String>,
    dirty: BTreeSet<usize>,
    /// complex ABox assertions (¬C, ∀p.C, ≤n p.C, …), re-applied until fixpoint
    cas: Vec<(usize, CE)>,
    /// number of new facts so far (fixpoint detection)
    changes: u64,
}

impl State {
    fn add_ind(&mut self) -> usize { let i = self.n; self.n += 1; self.parent.push(i); self.types.push(HashSet::new()); self.dirty.insert(i); i }
    fn find(&self, mut x: usize) -> usize { while self.parent[x] != x { x = self.parent[x]; } x }
    fn succ(&self, p: &Prop, a: usize) -> Vec<usize> {
        let a = self.find(a);
        let (m, k) = match p { Prop::Named(n) => (self.out.get(n), n), Prop::Inv(n) => (self.inn.get(n), n) };
        let _ = k;
        let mut v: Vec<usize> = m.and_then(|m| m.get(&a)).map(|s| s.iter().map(|b| self.find(*b)).collect()).unwrap_or_default();
        v.sort(); v.dedup(); v
    }
    fn has_edge(&self, p: &Prop, a: usize, b: usize) -> bool { let b = self.find(b); self.succ(p, a).contains(&b) }
    fn touch(&mut self, a: usize) {
        let a = self.find(a);
        self.changes += 1;
        self.dirty.insert(a);
        let neigh: Vec<usize> = self.out.values().chain(self.inn.values()).filter_map(|m| m.get(&a)).flat_map(|s| s.iter().copied()).collect();
        for b in neigh { let b = self.find(b); self.dirty.insert(b); }
    }
    fn add_edge(&mut self, p: &Prop, a: usize, b: usize) -> bool {
        let (a, b) = (self.find(a), self.find(b));
        let (n, a, b) = match p { Prop::Named(n) => (n.clone(), a, b), Prop::Inv(n) => (n.clone(), b, a) };
        let new = self.out.entry(n.clone()).or_default().entry(a).or_default().insert(b);
        self.inn.entry(n).or_default().entry(b).or_default().insert(a);
        if new { self.touch(a); self.touch(b); }
        new
    }
    fn add_type(&mut self, a: usize, c: &str) -> bool {
        if c == THING { return false; }
        let a = self.find(a);
        if c == NOTHING { if self.clash.is_none() { self.clash = Some(format!("individual {a} is owl:Nothing")); } return false; }
        let new = self.types[a].insert(c.to_string());
        if new { self.touch(a); }
        new
    }
    fn merge(&mut self, x: usize, y: usize) -> bool {
        let (x, y) = (self.find(x), self.find(y));
        if x == y { return false; }
        let (keep, gone) = (x.min(y), x.max(y));
        self.parent[gone] = keep;
        let ty: Vec<String> = self.types[gone].drain().collect();
        self.types[keep].extend(ty);
        for m in [&mut self.out, &mut self.inn] {
            for pm in m.values_mut() {
                if let Some(s) = pm.remove(&gone) { pm.entry(keep).or_default().extend(s); }
                for s in pm.values_mut() { if s.remove(&gone) { s.insert(keep); } }
            }
        }
        self.touch(keep);
        true
    }
}

pub struct RlReasoner {
    axioms: Vec<Axiom>,
    sig: Signature,
    ind_ix: HashMap<String, usize>,
    names: Vec<String>,
    base: Option<State>,
    protos: HashMap<String, State>,
    unsupported: Vec<String>,
    // compiled rules
    subs: Vec<(CE, CE)>,
    disjoint: Vec<Vec<CE>>,
    prop_sub: Vec<(Prop, Prop)>,
    chains: Vec<(Vec<Prop>, Prop)>,
    domain: Vec<(Prop, CE)>,
    range: Vec<(Prop, CE)>,
    functional: Vec<Prop>,
    inv_functional: Vec<Prop>,
    irreflexive: Vec<Prop>,
    asymmetric: Vec<Prop>,
    transitive: Vec<Prop>,
    disjoint_props: Vec<Vec<Prop>>,
    neg_assert: Vec<(Prop, String, String)>,
    different: Vec<Vec<String>>,
}

impl RlReasoner {
    pub fn new(axioms: Vec<Axiom>) -> Self {
        let sig = Signature::of(&axioms);
        let mut r = RlReasoner {
            sig, axioms: vec![], ind_ix: HashMap::new(), names: vec![], base: None, protos: HashMap::new(), unsupported: vec![],
            subs: vec![], disjoint: vec![], prop_sub: vec![], chains: vec![], domain: vec![], range: vec![], functional: vec![],
            inv_functional: vec![], irreflexive: vec![], asymmetric: vec![], transitive: vec![], disjoint_props: vec![],
            neg_assert: vec![], different: vec![],
        };
        for i in r.sig.individuals.clone() { r.ind_ix.insert(i.clone(), r.names.len()); r.names.push(i); }
        for a in &axioms { r.compile(a); }
        r.axioms = axioms;
        r
    }
    fn compile(&mut self, a: &Axiom) {
        let check_super = |c: &CE| super::profile::check(super::profile::Profile::RL, &[Axiom::ClassAssertion(c.clone(), "x".into())]).is_empty();
        match a {
            Axiom::SubClassOf(x, y) => self.subs.push((x.clone(), y.clone())),
            Axiom::EquivalentClasses(xs) => { let v: Vec<_> = xs.iter().cloned().collect();
                for i in 0..v.len() { for j in 0..v.len() { if i != j { self.subs.push((v[i].clone(), v[j].clone())); } } } }
            Axiom::DisjointClasses(xs) => self.disjoint.push(xs.iter().cloned().collect()),
            Axiom::DisjointUnion(c, xs) => { self.unsupported.push(format!("{a:?}")); let _ = (c, xs); }
            Axiom::ClassAssertion(..) | Axiom::PropertyAssertion(..) | Axiom::SameIndividual(_) | Axiom::Declaration(..) => {}
            Axiom::NegativePropertyAssertion(p, x, y) => self.neg_assert.push((p.clone(), x.clone(), y.clone())),
            Axiom::DifferentIndividuals(is) => self.different.push(is.iter().cloned().collect()),
            Axiom::SubPropertyOf(p, q) => self.prop_sub.push((p.clone(), q.clone())),
            Axiom::SubPropertyChain(ps, q) => self.chains.push((ps.clone(), q.clone())),
            Axiom::EquivalentProperties(ps) => { let v: Vec<_> = ps.iter().cloned().collect();
                for i in 0..v.len() { for j in 0..v.len() { if i != j { self.prop_sub.push((v[i].clone(), v[j].clone())); } } } }
            Axiom::InverseProperties(p, q) => { self.prop_sub.push((p.clone(), q.inverse())); self.prop_sub.push((q.clone(), p.inverse())); }
            Axiom::Domain(p, c) => self.domain.push((p.clone(), c.clone())),
            Axiom::Range(p, c) => self.range.push((p.clone(), c.clone())),
            Axiom::Functional(p) => self.functional.push(p.clone()),
            Axiom::InverseFunctional(p) => self.inv_functional.push(p.clone()),
            Axiom::Irreflexive(p) => self.irreflexive.push(p.clone()),
            Axiom::Asymmetric(p) => self.asymmetric.push(p.clone()),
            Axiom::Symmetric(p) => self.prop_sub.push((p.clone(), p.inverse())),
            Axiom::Transitive(p) => self.transitive.push(p.clone()),
            Axiom::DisjointProperties(ps) => self.disjoint_props.push(ps.iter().cloned().collect()),
            Axiom::Reflexive(_) | Axiom::Other(..) => self.unsupported.push(format!("{a:?}")),
        }
        if let Axiom::SubClassOf(_, y) = a { if !check_super(y) { self.unsupported.push(format!("{a:?}")); } }
    }

    fn holds(&self, st: &State, a: usize, c: &CE) -> bool {
        match c {
            CE::Class(i) => i == THING || (i != NOTHING && st.types[st.find(a)].contains(i)),
            CE::And(xs) => xs.iter().all(|x| self.holds(st, a, x)),
            CE::Or(xs) => xs.iter().any(|x| self.holds(st, a, x)),
            CE::OneOf(is) => is.iter().any(|i| self.ind_ix.get(i).is_some_and(|k| st.find(*k) == st.find(a))),
            CE::Some(p, f) => st.succ(p, a).into_iter().any(|b| self.holds(st, b, f)),
            CE::HasValue(p, i) => self.ind_ix.get(i).is_some_and(|k| st.has_edge(p, a, *k)),
            CE::HasSelf(p) => st.has_edge(p, a, a),
            _ => false,
        }
    }
    fn assert(&self, st: &mut State, a: usize, c: &CE) {
        match c {
            CE::Class(i) => { st.add_type(a, i); }
            CE::And(xs) => for x in xs { self.assert(st, a, x) },
            CE::Not(x) => if self.holds(st, a, x) && st.clash.is_none() { st.clash = Some(format!("{} ∈ ¬{x:?}", self.label(a))); },
            CE::All(p, f) => for b in st.succ(p, a) { self.assert(st, b, f) },
            CE::HasValue(p, i) => if let Some(k) = self.ind_ix.get(i) { st.add_edge(p, a, *k); },
            CE::Max(n, p, f) => {
                let bs: Vec<usize> = st.succ(p, a).into_iter().filter(|b| self.holds(st, *b, f)).collect();
                if *n == 0 && !bs.is_empty() && st.clash.is_none() { st.clash = Some(format!("max 0 violated at {}", self.label(a))); }
                if *n == 1 && bs.len() > 1 { for b in &bs[1..] { st.merge(bs[0], *b); } }
            }
            _ => {} // non-RL superclass expressions are reported as unsupported at compile time
        }
    }
    /// Query-time membership: like `holds`, plus refutation for negation (a ∈ ¬X iff adding
    /// a ∈ X to the saturated ABox is inconsistent). Positive conjuncts are tested first so the
    /// (expensive) refutation only runs for candidates.
    fn holds_q(&self, st: &State, a: usize, c: &CE) -> bool {
        match c {
            CE::And(xs) => {
                let (neg, pos): (Vec<&CE>, Vec<&CE>) = xs.iter().partition(|x| matches!(x, CE::Not(_)));
                pos.iter().all(|x| self.holds_q(st, a, x)) && neg.iter().all(|x| self.holds_q(st, a, x))
            }
            CE::Not(x) => self.refutes(st, a, x),
            CE::Some(p, f) if !matches!(**f, CE::Class(_)) => st.succ(p, a).into_iter().any(|b| self.holds_q(st, b, f)),
            // ≥n p.F: n distinct named successors in F (named individuals are pairwise different
            // under AceWiki's unique-name assumption); 0 and 1 reduce to ⊤ and ∃
            CE::Min(n, p, f) => {
                if *n == 0 { return true; }
                let named: BTreeSet<usize> = (0..self.names.len()).map(|i| st.find(i)).collect();
                let bs = st.succ(p, a).into_iter().filter(|b| self.holds_q(st, *b, f));
                if *n == 1 { return bs.count() > 0; }
                bs.filter(|b| named.contains(b)).count() >= *n as usize
            }
            _ => self.holds(st, a, c),
        }
    }
    fn refutes(&self, st: &State, a: usize, x: &CE) -> bool {
        match x {
            CE::Or(xs) => xs.iter().all(|y| self.refutes(st, a, y)),
            _ => {
                let mut s2 = st.clone();
                s2.dirty.clear();
                self.assert_q(&mut s2, a, x);
                self.saturate(&mut s2);
                s2.clash.is_some()
            }
        }
    }
    /// Hypothetical assertion for refutation: also handles ∃ (fresh witness) and nominals.
    fn assert_q(&self, st: &mut State, a: usize, c: &CE) {
        match c {
            CE::And(xs) => for x in xs { self.assert_q(st, a, x) },
            CE::OneOf(is) if is.len() == 1 => if let Some(k) = self.ind_ix.get(is.iter().next().unwrap()) { st.merge(a, *k); },
            CE::Some(p, f) => match &**f {
                CE::OneOf(is) if is.len() == 1 => if let Some(k) = self.ind_ix.get(is.iter().next().unwrap()) { st.add_edge(p, a, *k); },
                f => { let b = st.add_ind(); st.add_edge(p, a, b); self.assert_q(st, b, f); }
            },
            CE::HasSelf(p) => { st.add_edge(p, a, a); }
            _ => self.assert(st, a, c),
        }
        st.cas.push((a, c.clone()));
    }
    fn label(&self, a: usize) -> String { self.names.get(a).cloned().unwrap_or_else(|| format!("prototype#{a}")) }

    /// Propagate to a fixpoint. The dirty-set loop handles local consequences; the outer loop
    /// re-applies complex ABox assertions and does full passes until no new fact is derived, so
    /// consequences that depend on facts derived later at other individuals are never missed.
    fn saturate(&self, st: &mut State) {
        loop {
            self.propagate(st);
            if st.clash.is_some() { return; }
            let before = st.changes;
            for (a, c) in st.cas.clone() { self.assert(st, a, &c); }
            for a in 0..st.n { if st.find(a) == a { st.dirty.insert(a); } }
            self.propagate(st);
            if st.clash.is_some() || st.changes == before { return; }
        }
    }
    fn propagate(&self, st: &mut State) {
        let mut rounds = 0;
        while !st.dirty.is_empty() && st.clash.is_none() {
            rounds += 1;
            if rounds > 100_000 { st.clash = Some("saturation did not terminate".into()); break; }
            let batch: Vec<usize> = std::mem::take(&mut st.dirty).into_iter().map(|a| st.find(a)).collect::<BTreeSet<_>>().into_iter().collect();
            for a in batch {
                let a = st.find(a);
                for (x, y) in &self.subs { if self.holds(st, a, x) { self.assert(st, a, y); } }
                for ds in &self.disjoint {
                    let n = ds.iter().filter(|d| self.holds(st, a, d)).count();
                    if n > 1 && st.clash.is_none() { st.clash = Some(format!("{} in disjoint classes", self.label(a))); }
                }
                for (p, q) in &self.prop_sub { for b in st.succ(p, a) { st.add_edge(q, a, b); } }
                for (ps, q) in &self.chains {
                    let mut frontier = vec![a];
                    for p in ps { let mut nx = vec![]; for x in frontier { nx.extend(st.succ(p, x)); } nx.sort(); nx.dedup(); frontier = nx; }
                    for b in frontier { st.add_edge(q, a, b); }
                }
                for p in &self.transitive {
                    for b in st.succ(p, a) { for c in st.succ(p, b) { st.add_edge(p, a, c); } }
                    for b in st.succ(&p.inverse(), a) { for c in st.succ(p, a) { st.add_edge(p, b, c); } }
                }
                for (p, c) in &self.domain { if !st.succ(p, a).is_empty() { self.assert(st, a, c); } }
                for (p, c) in &self.range { if !st.succ(&p.inverse(), a).is_empty() { self.assert(st, a, c); } }
                for p in &self.functional { let bs = st.succ(p, a); if bs.len() > 1 { for b in &bs[1..] { st.merge(bs[0], *b); } } }
                for p in &self.inv_functional { let bs = st.succ(&p.inverse(), a); if bs.len() > 1 { for b in &bs[1..] { st.merge(bs[0], *b); } } }
                for p in &self.irreflexive { if st.has_edge(p, a, a) && st.clash.is_none() { st.clash = Some(format!("irreflexive {p:?} at {}", self.label(a))); } }
                for p in &self.asymmetric { for b in st.succ(p, a) { if st.has_edge(p, b, a) && st.clash.is_none() { st.clash = Some(format!("asymmetric {p:?}")); } } }
                for ps in &self.disjoint_props {
                    let v: Vec<&Prop> = ps.iter().collect();
                    for i in 0..v.len() { for j in i + 1..v.len() {
                        for b in st.succ(v[i], a) { if st.has_edge(v[j], a, b) && st.clash.is_none() { st.clash = Some("disjoint properties".into()); } }
                    } }
                }
            }
            self.global_checks(st);
        }
        self.global_checks(st);
    }
    fn global_checks(&self, st: &mut State) {
        if st.clash.is_some() { return; }
        for (p, x, y) in &self.neg_assert {
            if let (Some(a), Some(b)) = (self.ind_ix.get(x), self.ind_ix.get(y)) {
                if st.has_edge(p, *a, *b) { st.clash = Some(format!("negative assertion violated: {p:?}({x},{y})")); return; }
            }
        }
        for ds in &self.different {
            let reps: Vec<usize> = ds.iter().filter_map(|i| self.ind_ix.get(i)).map(|k| st.find(*k)).collect();
            let set: HashSet<usize> = reps.iter().copied().collect();
            if set.len() < reps.len() { st.clash = Some("two different individuals are the same".into()); return; }
        }
    }

    fn base(&mut self) -> &State {
        if self.base.is_none() {
            let mut st = State::default();
            for _ in 0..self.names.len() { st.add_ind(); }
            for a in &self.axioms {
                match a {
                    Axiom::ClassAssertion(c, i) => {
                        let k = self.ind_ix[i];
                        if !matches!(c, CE::Class(_)) { st.cas.push((k, c.clone())); }
                        self.assert(&mut st, k, c);
                    }
                    Axiom::PropertyAssertion(p, x, y) => { let (a1, b1) = (self.ind_ix[x], self.ind_ix[y]); st.add_edge(p, a1, b1); }
                    Axiom::SameIndividual(is) => { let v: Vec<usize> = is.iter().map(|i| self.ind_ix[i]).collect(); for k in &v[1..] { st.merge(v[0], *k); } }
                    _ => {}
                }
            }
            self.saturate(&mut st);
            self.base = Some(st);
        }
        self.base.as_ref().unwrap()
    }
    /// Base ABox + one prototype individual of class `c` (index `n`).
    fn proto(&mut self, c: &str) -> (&State, usize) {
        let n = self.names.len();
        if !self.protos.contains_key(c) {
            let mut st = self.base().clone();
            st.dirty.clear();
            let k = st.add_ind();
            st.add_type(k, c);
            self.saturate(&mut st);
            self.protos.insert(c.to_string(), st);
        }
        (&self.protos[c], n)
    }
    fn named_types(&self, st: &State, a: usize) -> BTreeSet<String> {
        st.types[st.find(a)].iter().filter(|c| self.sig.classes.contains(*c)).cloned().collect()
    }
}

impl Reasoner for RlReasoner {
    fn name(&self) -> &'static str { "acewiki-rs native OWL 2 RL" }
    fn unsupported(&self) -> Vec<String> { self.unsupported.clone() }
    fn is_consistent(&mut self) -> bool { self.base().clash.is_none() }
    fn types(&mut self, ind: &str) -> BTreeSet<String> {
        let Some(&k) = self.ind_ix.get(ind) else { return BTreeSet::new() };
        if !self.is_consistent() { return self.sig.classes.clone(); }
        let st = self.base().clone();
        self.named_types(&st, k)
    }
    fn instances(&mut self, c: &CE) -> BTreeSet<String> {
        if !self.is_consistent() { return self.sig.individuals.clone(); }
        let st = self.base().clone();
        let mut out = BTreeSet::new();
        for (i, name) in self.names.iter().enumerate() {
            let ok = self.holds_q(&st, i, c);
            if ok { out.insert(name.clone()); }
        }
        out
    }
    fn super_classes(&mut self, c: &str) -> BTreeSet<String> {
        if !self.is_satisfiable(c) {
            let cands: Vec<String> = self.sig.classes.iter().filter(|x| x.as_str() != c).cloned().collect();
            return cands.into_iter().filter(|x| self.is_satisfiable(x)).collect();
        }
        let (st, k) = self.proto(c);
        let st = st.clone();
        let sups = self.named_types(&st, k);
        let mut out = BTreeSet::new();
        for s in sups {
            if s == c { continue; }
            // strict: exclude equivalents (s ⊑ c as well)
            let (st2, k2) = self.proto(&s);
            let back = st2.types[st2.find(k2)].contains(c);
            if !back { out.insert(s); }
        }
        out
    }
    fn sub_classes(&mut self, c: &str) -> BTreeSet<String> {
        let classes = self.sig.classes.clone();
        let mut out = BTreeSet::new();
        for d in classes {
            if d == c { continue; }
            if !self.is_satisfiable(&d) { out.insert(d); continue; }
            let (st, k) = self.proto(&d);
            if st.types[st.find(k)].contains(c) {
                let (st2, k2) = self.proto(c);
                if !st2.types[st2.find(k2)].contains(&d) || st2.clash.is_some() { out.insert(d); }
            }
        }
        out
    }
    fn is_satisfiable(&mut self, c: &str) -> bool {
        if !self.is_consistent() { return false; }
        if !self.sig.classes.contains(c) { return true; }
        self.proto(c).0.clash.is_none()
    }
}

// =====================================================================================  EL

/// Normalised EL concept used inside the completion engine.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum B { Top, Bot, A(String), Nom(String), Fresh(usize) }

pub struct ElReasoner {
    sig: Signature,
    unsupported: Vec<String>,
    // normal forms
    sub: HashMap<B, Vec<B>>,                       // A ⊑ B
    conj: Vec<(B, B, B)>,                          // A1 ⊓ A2 ⊑ B
    ex_r: HashMap<B, Vec<(String, B)>>,            // A ⊑ ∃r.B
    ex_l: HashMap<(String, B), Vec<B>>,            // ∃r.A ⊑ B
    role_sub: HashMap<String, Vec<String>>,        // r ⊑ s (reflexive-transitive closure computed)
    chains: Vec<(String, String, String)>,         // r ∘ s ⊑ t
    ranges: HashMap<String, Vec<B>>,
    neg_assert: Vec<(String, String, String)>,
    different: Vec<Vec<String>>,
    fresh: usize,
    query: Option<B>,
    done: Option<Saturation>,
    axioms: Vec<Axiom>,
    /// RL engine over the EL-completed ABox, for questions outside EL (inverse, ¬, ≥n, ∀)
    rl: Option<Box<RlReasoner>>,
}

#[derive(Clone, Default)]
struct Saturation {
    s: HashMap<B, BTreeSet<B>>,
    r: HashMap<String, BTreeSet<(B, B)>>,
    clash: bool,
}

impl ElReasoner {
    pub fn new(axioms: Vec<Axiom>) -> Self {
        let sig = Signature::of(&axioms);
        let mut e = ElReasoner { sig, unsupported: vec![], sub: HashMap::new(), conj: vec![], ex_r: HashMap::new(), ex_l: HashMap::new(),
            role_sub: HashMap::new(), chains: vec![], ranges: HashMap::new(), neg_assert: vec![], different: vec![], fresh: 0, query: None, done: None,
            axioms: axioms.clone(), rl: None };
        for a in &axioms { e.add(a); }
        e.close_roles();
        e
    }
    fn fresh(&mut self) -> B { self.fresh += 1; B::Fresh(self.fresh) }
    /// Name a (possibly complex) EL concept on the left (`side_left`) or right of ⊑.
    fn name(&mut self, c: &CE, left: bool) -> Option<B> {
        Some(match c {
            CE::Class(i) if i == THING => B::Top,
            CE::Class(i) if i == NOTHING => B::Bot,
            CE::Class(i) => B::A(i.clone()),
            CE::OneOf(is) if is.len() == 1 => B::Nom(is.iter().next().unwrap().clone()),
            CE::HasValue(Prop::Named(r), i) => return self.name(&CE::Some(Prop::Named(r.clone()), Box::new(CE::OneOf([i.clone()].into()))), left),
            CE::And(xs) => {
                let parts: Vec<B> = xs.iter().map(|x| self.name(x, left)).collect::<Option<_>>()?;
                let f = self.fresh();
                if left {
                    // A1 ⊓ ... ⊓ An ⊑ F  (binary chain)
                    let mut acc = parts[0].clone();
                    for p in &parts[1..] { let g = self.fresh(); self.conj.push((acc.clone(), p.clone(), g.clone())); acc = g; }
                    self.sub.entry(acc).or_default().push(f.clone());
                } else {
                    for p in parts { self.sub.entry(f.clone()).or_default().push(p); }
                }
                f
            }
            CE::Some(Prop::Named(r), x) => {
                let b = self.name(x, left)?;
                let f = self.fresh();
                if left { self.ex_l.entry((r.clone(), b)).or_default().push(f.clone()); } else { self.ex_r.entry(f.clone()).or_default().push((r.clone(), b)); }
                f
            }
            _ => return None,
        })
    }
    fn gci(&mut self, x: &CE, y: &CE, a: &Axiom) {
        match (self.name(x, true), self.name(y, false)) {
            (Some(l), Some(r)) => self.sub.entry(l).or_default().push(r),
            _ => self.unsupported.push(format!("{a:?}")),
        }
    }
    fn add(&mut self, a: &Axiom) {
        let nom = |i: &str| CE::OneOf([i.to_string()].into());
        match a {
            Axiom::Declaration(..) => {}
            Axiom::SubClassOf(x, y) => self.gci(x, y, a),
            Axiom::EquivalentClasses(xs) => { let v: Vec<_> = xs.iter().cloned().collect(); for i in 0..v.len() { for j in 0..v.len() { if i != j { self.gci(&v[i], &v[j], a); } } } }
            Axiom::DisjointClasses(xs) => { let v: Vec<_> = xs.iter().cloned().collect();
                for i in 0..v.len() { for j in i + 1..v.len() { self.gci(&CE::And([v[i].clone(), v[j].clone()].into()), &CE::Class(NOTHING.into()), a); } } }
            Axiom::ClassAssertion(c, i) => self.gci(&nom(i), c, a),
            Axiom::PropertyAssertion(Prop::Named(r), x, y) => self.gci(&nom(x), &CE::Some(Prop::Named(r.clone()), Box::new(nom(y))), a),
            Axiom::NegativePropertyAssertion(Prop::Named(r), x, y) => self.neg_assert.push((r.clone(), x.clone(), y.clone())),
            Axiom::SameIndividual(is) => { let v: Vec<_> = is.iter().collect(); for i in 1..v.len() { self.gci(&nom(v[0]), &nom(v[i]), a); self.gci(&nom(v[i]), &nom(v[0]), a); } }
            Axiom::DifferentIndividuals(is) => self.different.push(is.iter().cloned().collect()),
            Axiom::SubPropertyOf(Prop::Named(r), Prop::Named(s)) => self.role_sub.entry(r.clone()).or_default().push(s.clone()),
            Axiom::EquivalentProperties(ps) => {
                let v: Vec<_> = ps.iter().collect();
                for p in &v { for q in &v { if let (Prop::Named(r), Prop::Named(s)) = (p, q) { if r != s { self.role_sub.entry(r.clone()).or_default().push(s.clone()); } } } }
            }
            Axiom::SubPropertyChain(ps, Prop::Named(t)) if ps.iter().all(|p| matches!(p, Prop::Named(_))) => {
                let names: Vec<String> = ps.iter().map(|p| p.base().to_string()).collect();
                if names.len() == 1 { self.role_sub.entry(names[0].clone()).or_default().push(t.clone()); }
                else {
                    let mut acc = names[0].clone();
                    for (i, n) in names[1..].iter().enumerate() {
                        let target = if i == names.len() - 2 { t.clone() } else { format!("_chain{}_{}", self.chains.len(), i) };
                        self.chains.push((acc.clone(), n.clone(), target.clone()));
                        acc = target;
                    }
                }
            }
            Axiom::Transitive(Prop::Named(r)) => self.chains.push((r.clone(), r.clone(), r.clone())),
            Axiom::Domain(Prop::Named(r), c) => self.gci(&CE::Some(Prop::Named(r.clone()), Box::new(CE::Class(THING.into()))), c, a),
            Axiom::Range(Prop::Named(r), c) => match self.name(c, false) { Some(b) => self.ranges.entry(r.clone()).or_default().push(b), None => self.unsupported.push(format!("{a:?}")) },
            _ => self.unsupported.push(format!("{a:?}")),
        }
    }
    fn close_roles(&mut self) {
        let mut changed = true;
        while changed {
            changed = false;
            let snap = self.role_sub.clone();
            for (r, ss) in &snap { for s in ss { if let Some(ts) = snap.get(s) { for t in ts { let e = self.role_sub.entry(r.clone()).or_default(); if !e.contains(t) && t != r { e.push(t.clone()); changed = true; } } } } }
        }
    }
    fn supers_of_role(&self, r: &str) -> Vec<String> { let mut v = vec![r.to_string()]; if let Some(x) = self.role_sub.get(r) { v.extend(x.iter().cloned()); } v }

    fn saturate(&mut self) -> &Saturation {
        if self.done.is_some() { return self.done.as_ref().unwrap(); }
        let mut sat = Saturation::default();
        let mut queue: VecDeque<(B, B)> = VecDeque::new();          // (context, concept) for S(context)
        let mut rq: VecDeque<(String, B, B)> = VecDeque::new();      // (r, C, D) for R(r)
        let mut contexts: Vec<B> = vec![B::Top];
        for c in &self.sig.classes { contexts.push(B::A(c.clone())); }
        for i in &self.sig.individuals { contexts.push(B::Nom(i.clone())); }
        if let Some(q) = &self.query { contexts.push(q.clone()); }
        let mut known: HashSet<B> = HashSet::new();
        for c in &contexts { if known.insert(c.clone()) { queue.push_back((c.clone(), c.clone())); queue.push_back((c.clone(), B::Top)); } }
        let mut preds: HashMap<B, Vec<(String, B)>> = HashMap::new(); // D -> [(r, C)] with (C,D) in R(r)
        // contexts known to be non-empty: individuals and everything R-reachable from them
        let mut reach: HashSet<B> = self.sig.individuals.iter().map(|i| B::Nom(i.clone())).collect();
        // successor contexts D ⊓ ranges(r), memoised per (D, r)
        let mut range_ctx: HashMap<(B, String), B> = HashMap::new();
        let mut extra_sub: HashMap<B, Vec<B>> = HashMap::new();
        let mut fresh = self.fresh + 20_000_000;
        let mut steps = 0usize;
        loop {
            steps += 1;
            if steps > 20_000_000 { break; }
            if let Some((c, d)) = queue.pop_front() {
                if !sat.s.entry(c.clone()).or_default().insert(d.clone()) { continue; }
                for ys in [self.sub.get(&d), extra_sub.get(&d)].into_iter().flatten() { for y in ys { queue.push_back((c.clone(), y.clone())); } }  // CR1
                let sc = sat.s[&c].clone();
                for (a1, a2, b) in &self.conj {                                                                                         // CR2
                    if (a1 == &d && sc.contains(a2)) || (a2 == &d && sc.contains(a1)) { queue.push_back((c.clone(), b.clone())); }
                }
                if let Some(es) = self.ex_r.get(&d) {                                                                                     // CR3
                    for (r, b) in es {
                        let rs: Vec<B> = self.supers_of_role(r).iter().flat_map(|x| self.ranges.get(x).cloned().unwrap_or_default()).collect();
                        let target = if rs.is_empty() { b.clone() } else {
                            range_ctx.entry((b.clone(), r.clone())).or_insert_with(|| {
                                fresh += 1; let t = B::Fresh(fresh);
                                extra_sub.insert(t.clone(), std::iter::once(b.clone()).chain(rs.into_iter()).collect());
                                t
                            }).clone()
                        };
                        if known.insert(target.clone()) { queue.push_back((target.clone(), target.clone())); queue.push_back((target.clone(), B::Top)); }
                        rq.push_back((r.clone(), c.clone(), target));
                    }
                }
                if let Some(ps) = preds.get(&c).cloned() {                                                                                // CR4/CR5 backward
                    for (r, x) in ps {
                        if let Some(es) = self.ex_l.get(&(r.clone(), d.clone())) { for e in es { queue.push_back((x.clone(), e.clone())); } }
                        if d == B::Bot { queue.push_back((x.clone(), B::Bot)); }
                    }
                }
                // CR6 (nominals). (a) C ⊑ {i} ⇒ S({i}) ⊆ S(C)  — always sound.
                if let B::Nom(i) = &d {
                    let own = B::Nom(i.clone());
                    if c != own { if let Some(so) = sat.s.get(&own).cloned() { for x in so { queue.push_back((c.clone(), x)); } } }
                }
                if matches!(c, B::Nom(_)) {
                    let holders: Vec<B> = sat.s.iter().filter(|(k, v)| *k != &c && v.contains(&c)).map(|(k, _)| k.clone()).collect();
                    for h in holders { queue.push_back((h, d.clone())); }
                }
                // (b) C non-empty (reachable from an individual) and C ⊑ {i} ⇒ C ≡ {i} ⇒ S(C) ⊆ S({i})
                if reach.contains(&c) {
                    let noms: Vec<B> = sat.s[&c].iter().filter(|x| matches!(x, B::Nom(_)) && **x != c).cloned().collect();
                    for n in &noms { queue.push_back((n.clone(), d.clone())); }
                    if matches!(d, B::Nom(_)) && d != c { for x in sat.s[&c].clone() { queue.push_back((d.clone(), x)); } }
                }
                if d == B::Bot && matches!(c, B::Nom(_) | B::Top) { sat.clash = true; }
            } else if let Some((r, c, d)) = rq.pop_front() {
                for rr in self.supers_of_role(&r) {
                    if !sat.r.entry(rr.clone()).or_default().insert((c.clone(), d.clone())) { continue; }
                    preds.entry(d.clone()).or_default().push((rr.clone(), c.clone()));
                    if reach.contains(&c) && !reach.contains(&d) {
                        // d (and everything reachable from it) becomes non-empty: replay its S-set through rule (b)
                        let mut stack = vec![d.clone()];
                        while let Some(x) = stack.pop() {
                            if !reach.insert(x.clone()) { continue; }
                            if let Some(sx) = sat.s.get(&x).cloned() {
                                for n in sx.iter().filter(|y| matches!(y, B::Nom(_)) && **y != x) { for y in &sx { queue.push_back((n.clone(), y.clone())); } }
                            }
                            for (_, ps) in sat.r.iter() { for (a, b) in ps { if a == &x && !reach.contains(b) { stack.push(b.clone()); } } }
                        }
                    }
                    if let Some(sd) = sat.s.get(&d).cloned() {                                                                            // CR4/CR5 forward
                        for x in sd {
                            if let Some(es) = self.ex_l.get(&(rr.clone(), x.clone())) { for e in es { queue.push_back((c.clone(), e.clone())); } }
                            if x == B::Bot { queue.push_back((c.clone(), B::Bot)); }
                        }
                    }
                    for (r1, r2, t) in &self.chains {                                                                                     // CR11
                        if r1 == &rr { if let Some(ps) = sat.r.get(r2) { for (x, y) in ps.clone() { if &x == &d { rq.push_back((t.clone(), c.clone(), y)); } } } }
                        if r2 == &rr { if let Some(ps) = sat.r.get(r1) { for (x, y) in ps.clone() { if &y == &c { rq.push_back((t.clone(), x, d.clone())); } } } }
                    }
                }
            } else { break; }
        }
        for ds in &self.different {
            for i in 0..ds.len() { for j in i + 1..ds.len() {
                if sat.s.get(&B::Nom(ds[i].clone())).is_some_and(|s| s.contains(&B::Nom(ds[j].clone()))) { sat.clash = true; }
            } }
        }
        for (r, x, y) in &self.neg_assert {
            if sat.r.get(r).is_some_and(|p| p.iter().any(|(c, d)| c == &B::Nom(x.clone()) && sat.s.get(d).is_some_and(|s| s.contains(&B::Nom(y.clone()))))) { sat.clash = true; }
        }
        self.done = Some(sat);
        self.done.as_ref().unwrap()
    }
    /// RL engine over the original axioms plus everything EL derived about named individuals
    /// (types and edges between individuals), used for question classes outside EL.
    fn rl_view(&mut self) -> &mut RlReasoner {
        if self.rl.is_none() {
            let sat = self.saturate().clone();
            let mut ax = self.axioms.clone();
            for i in &self.sig.individuals {
                if let Some(si) = sat.s.get(&B::Nom(i.clone())) {
                    for b in si { if let B::A(c) = b { ax.push(Axiom::ClassAssertion(CE::Class(c.clone()), i.clone())); } }
                }
            }
            for (r, ps) in &sat.r {
                if r.starts_with("_chain") { continue; }
                for (x, y) in ps {
                    let B::Nom(a) = x else { continue };
                    for b in sat.s.get(y).into_iter().flatten() { if let B::Nom(bn) = b { ax.push(Axiom::PropertyAssertion(Prop::Named(r.clone()), a.clone(), bn.clone())); } }
                }
            }
            self.rl = Some(Box::new(RlReasoner::new(ax)));
        }
        self.rl.as_mut().unwrap()
    }
    fn s_of(&mut self, b: &B) -> BTreeSet<B> { self.saturate(); self.done.as_ref().unwrap().s.get(b).cloned().unwrap_or_default() }
    fn named(&self, s: &BTreeSet<B>) -> BTreeSet<String> {
        s.iter().filter_map(|b| if let B::A(c) = b { self.sig.classes.contains(c).then(|| c.clone()) } else { None }).collect()
    }
}

impl Reasoner for ElReasoner {
    fn name(&self) -> &'static str { "acewiki-rs native OWL 2 EL" }
    fn unsupported(&self) -> Vec<String> { self.unsupported.clone() }
    fn is_consistent(&mut self) -> bool { !self.saturate().clash }
    fn types(&mut self, ind: &str) -> BTreeSet<String> {
        if !self.is_consistent() { return self.sig.classes.clone(); }
        let s = self.s_of(&B::Nom(ind.to_string()));
        self.named(&s)
    }
    fn instances(&mut self, c: &CE) -> BTreeSet<String> {
        if !self.is_consistent() { return self.sig.individuals.clone(); }
        // name the query concept on the left: Q ⊑ Fq, re-saturate, read individuals with Fq
        let saved = (self.sub.clone(), self.conj.clone(), self.ex_l.clone(), self.fresh, self.done.take());
        let out = match self.name(c, true) {
            Some(q) => {
                let target = B::Fresh(10_000_000 + self.fresh);
                self.sub.entry(q).or_default().push(target.clone());
                self.done = None;
                let inds = self.sig.individuals.clone();
                inds.into_iter().filter(|i| self.s_of(&B::Nom(i.clone())).contains(&target)).collect()
            }
            None => {
                self.sub = saved.0.clone(); self.conj = saved.1.clone(); self.ex_l = saved.2.clone(); self.fresh = saved.3; self.done = saved.4.clone();
                return self.rl_view().instances(c);
            }
        };
        self.sub = saved.0; self.conj = saved.1; self.ex_l = saved.2; self.fresh = saved.3; self.done = saved.4;
        out
    }
    fn super_classes(&mut self, c: &str) -> BTreeSet<String> {
        if !self.is_satisfiable(c) { return self.sig.classes.iter().filter(|x| x.as_str() != c).cloned().collect::<Vec<_>>().into_iter().filter(|x| self.is_satisfiable(x)).collect(); }
        let s = self.s_of(&B::A(c.to_string()));
        let sup = self.named(&s);
        sup.into_iter().filter(|d| d != c && !self.s_of(&B::A(d.clone())).contains(&B::A(c.to_string()))).collect()
    }
    fn sub_classes(&mut self, c: &str) -> BTreeSet<String> {
        let classes = self.sig.classes.clone();
        let mine = self.s_of(&B::A(c.to_string()));
        classes.into_iter().filter(|d| d != c).filter(|d| {
            let sd = self.s_of(&B::A(d.clone()));
            sd.contains(&B::Bot) || (sd.contains(&B::A(c.to_string())) && !mine.contains(&B::A(d.clone())))
        }).collect()
    }
    fn is_satisfiable(&mut self, c: &str) -> bool {
        if !self.is_consistent() { return false; }
        !self.s_of(&B::A(c.to_string())).contains(&B::Bot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c(n: &str) -> CE { CE::Class(n.into()) }
    fn p(n: &str) -> Prop { Prop::Named(n.into()) }

    /// "Germany is not a country." after "Germany is an alpine country." / "Every alpine country is a country."
    #[test]
    fn negative_class_assertion_checked_after_derivation() {
        for order in [false, true] {
            let mut ax = vec![
                Axiom::ClassAssertion(CE::Not(Box::new(c("country"))), "G".into()),
                Axiom::SubClassOf(c("alpine"), c("country")),
                Axiom::ClassAssertion(c("alpine"), "G".into()),
            ];
            if order { ax.reverse(); }
            assert!(!RlReasoner::new(ax).is_consistent(), "order {order}");
        }
    }

    /// two-hop existential on the left-hand side: ∃borders.∃borders.Rich ⊑ Lucky
    #[test]
    fn nested_existential_found_by_full_pass() {
        let ax = vec![
            Axiom::SubClassOf(CE::Some(p("borders"), Box::new(CE::Some(p("borders"), Box::new(c("Rich"))))), c("Lucky")),
            Axiom::PropertyAssertion(p("borders"), "a".into(), "b".into()),
            Axiom::PropertyAssertion(p("borders"), "b".into(), "c".into()),
            Axiom::SubClassOf(c("Big"), c("Rich")),
            Axiom::ClassAssertion(c("Big"), "c".into()),
        ];
        let mut r = RlReasoner::new(ax);
        assert!(r.types("a").contains("Lucky"));
    }

    /// ∀ in an ABox assertion applies to successors added later (via a sub-property)
    #[test]
    fn universal_assertion_applies_to_later_edges() {
        let ax = vec![
            Axiom::ClassAssertion(CE::All(p("hasPart"), Box::new(c("Part"))), "x".into()),
            Axiom::SubPropertyOf(p("hasDirectPart"), p("hasPart")),
            Axiom::PropertyAssertion(p("hasDirectPart"), "x".into(), "y".into()),
        ];
        let mut r = RlReasoner::new(ax);
        assert!(r.types("y").contains("Part"));
    }

    /// "Which country is not a member of the EU?" — ¬∃member.{EU} inside a conjunction,
    /// answered by refutation under unique names; disjointness gives "… and is not a capital".
    #[test]
    fn negation_inside_conjunctive_query() {
        let ax = vec![
            Axiom::ClassAssertion(c("country"), "CH".into()),
            Axiom::ClassAssertion(c("country"), "DE".into()),
            Axiom::PropertyAssertion(p("member"), "DE".into(), "EU".into()),
            Axiom::ClassAssertion(CE::Not(Box::new(CE::Some(p("member"), Box::new(CE::OneOf(["EU".to_string()].into()))))), "CH".into()),
            Axiom::DisjointClasses(vec![c("country"), c("capital")].into_iter().collect()),
            Axiom::DifferentIndividuals(["CH", "DE", "EU"].iter().map(|x| x.to_string()).collect()),
        ];
        let mut r = RlReasoner::new(ax);
        let q = CE::And(BTreeSet::from([c("country"), CE::Not(Box::new(CE::Some(p("member"), Box::new(CE::OneOf(["EU".to_string()].into())))))]));
        assert_eq!(r.instances(&q).into_iter().collect::<Vec<_>>(), vec!["CH".to_string()]);
        let q2 = CE::And(BTreeSet::from([c("country"), CE::Not(Box::new(c("capital")))]));
        assert_eq!(r.instances(&q2).len(), 2);
    }

    /// "Which book is written by at least 2 authors?" under unique names
    #[test]
    fn min_cardinality_query_counts_named_successors() {
        let ax = vec![
            Axiom::ClassAssertion(c("book"), "GO".into()), Axiom::ClassAssertion(c("book"), "Solo".into()),
            Axiom::ClassAssertion(c("author"), "TP".into()), Axiom::ClassAssertion(c("author"), "NG".into()),
            Axiom::PropertyAssertion(p("write"), "TP".into(), "GO".into()), Axiom::PropertyAssertion(p("write"), "NG".into(), "GO".into()),
            Axiom::PropertyAssertion(p("write"), "TP".into(), "Solo".into()),
            Axiom::DifferentIndividuals(["GO", "Solo", "TP", "NG"].iter().map(|x| x.to_string()).collect()),
        ];
        let mut r = RlReasoner::new(ax);
        let q = CE::And(BTreeSet::from([c("book"), CE::Min(2, Prop::Inv("write".into()), Box::new(c("author")))]));
        assert_eq!(r.instances(&q).into_iter().collect::<Vec<_>>(), vec!["GO".to_string()]);
    }

    /// EL: Range(capital-of, capital) + capital-of(Germany, Berlin) ⇒ Berlin ∈ capital ⊑ city
    #[test]
    fn el_range_reaches_abox_individual() {
        let ax = vec![
            Axiom::Range(p("capital"), c("capital")),
            Axiom::SubClassOf(c("capital"), c("city")),
            Axiom::PropertyAssertion(p("capital"), "DE".into(), "Berlin".into()),
            Axiom::DifferentIndividuals(["DE", "Berlin"].iter().map(|x| x.to_string()).collect()),
        ];
        let mut r = ElReasoner::new(ax);
        let t = r.types("Berlin");
        assert!(t.contains("capital") && t.contains("city"), "{t:?}");
        assert!(r.is_consistent());
    }

    /// EL ontology, question with an inverse property ("Which countries are contained by Europe?")
    #[test]
    fn el_inverse_question_uses_rl_view() {
        let ax = vec![
            Axiom::ClassAssertion(c("country"), "DE".into()),
            Axiom::PropertyAssertion(p("contain"), "EU".into(), "DE".into()),
            Axiom::DifferentIndividuals(["DE", "EU"].iter().map(|x| x.to_string()).collect()),
        ];
        let mut r = ElReasoner::new(ax);
        let q = CE::And(BTreeSet::from([c("country"), CE::Some(Prop::Inv("contain".into()), Box::new(CE::OneOf(["EU".to_string()].into())))]));
        assert_eq!(r.instances(&q).into_iter().collect::<Vec<_>>(), vec!["DE".to_string()]);
    }

    /// EL: a class context that merely *could* be {a} must not leak into a (it may be empty)
    #[test]
    fn el_unreachable_nominal_context_does_not_leak() {
        let ax = vec![
            Axiom::SubClassOf(c("Ghost"), CE::OneOf(["a".to_string()].into())),
            Axiom::SubClassOf(c("Ghost"), c("Spooky")),
            Axiom::ClassAssertion(c("Thingy"), "a".into()),
        ];
        let mut r = ElReasoner::new(ax);
        assert!(!r.types("a").contains("Spooky"));
    }
}
