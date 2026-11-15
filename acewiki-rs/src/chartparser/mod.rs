//! Port of `ch.uzh.ifi.attempto.chartparser` (AceWiki's Codeco chart parser).
//!
//! The Java parser represents feature values as `StringRef`s pointing to shared, mutable
//! `StringObject`s; unification merges objects. Here every parser owns an arena of union-find
//! variables ([`Vars`]); a feature value is a variable id, `deep_copy` allocates fresh variables
//! (preserving sharing within the copied structure, exactly like the Java `stringObjs` map), and
//! unification is only ever applied to fresh copies — the same invariant the Java code relies on.
//!
//! Semantics mirrored 1:1 from the Java sources: Earley-style predict / resolve / complete loop
//! with the same idle-step termination, antecedent lists (`>`, `>>`, `//`, `#`, `<`, `/<`),
//! scope-closing rules (`~>`), edge identifiers used for chart de-duplication, abstract options
//! with exceptions, and concrete options from grammar + dynamic lexicon.

pub mod grammar;
pub mod parser;

pub use grammar::{Grammar, LexicalRule, GrammarRule};
pub use parser::{ChartParser, AbstractOption, ConcreteOption, DynamicLexicon, ParseTree};

use std::rc::Rc;

/// Interned symbol (category or feature name).
pub type Sym = Rc<str>;

/// Union-find arena of feature variables.
#[derive(Default, Debug, Clone)]
pub struct Vars {
    parent: Vec<u32>,
    val: Vec<Option<Rc<str>>>,
    sk: u64,
}

pub type Var = u32;

#[derive(Debug)]
pub struct UnifyFail;

impl Vars {
    pub fn fresh(&mut self, v: Option<Rc<str>>) -> Var {
        let id = self.parent.len() as u32;
        self.parent.push(id);
        self.val.push(v);
        id
    }
    pub fn find(&mut self, mut x: Var) -> Var {
        while self.parent[x as usize] != x {
            let p = self.parent[x as usize];
            self.parent[x as usize] = self.parent[p as usize];
            x = p;
        }
        x
    }
    pub fn get(&mut self, x: Var) -> Option<Rc<str>> {
        let r = self.find(x);
        self.val[r as usize].clone()
    }
    /// Java `StringObject.unify`: representative of `a` absorbs `b`.
    pub fn unify(&mut self, a: Var, b: Var) -> Result<(), UnifyFail> {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb { return Ok(()); }
        match (&self.val[ra as usize], &self.val[rb as usize]) {
            (Some(x), Some(y)) if x != y => return Err(UnifyFail),
            (None, Some(_)) => self.val[ra as usize] = self.val[rb as usize].clone(),
            _ => {}
        }
        self.parent[rb as usize] = ra;
        Ok(())
    }
    fn skolem(&mut self) -> Rc<str> { self.sk += 1; Rc::from(format!("$SK{}", self.sk)) }
}

/// Feature map: feature name -> variable (Java `FeatureMap`). Order of insertion is kept.
#[derive(Clone, Debug, Default)]
pub struct FeatureMap { pub f: Vec<(Sym, Var)> }

impl FeatureMap {
    pub fn get_opt(&self, name: &str) -> Option<Var> { self.f.iter().find(|(n, _)| &**n == name).map(|x| x.1) }
    /// Java `getFeature`: creates a fresh unbound variable when absent.
    pub fn get(&mut self, vars: &mut Vars, name: &Sym) -> Var {
        if let Some(v) = self.get_opt(name) { return v; }
        let v = vars.fresh(None);
        self.f.push((name.clone(), v));
        v
    }
    pub fn set(&mut self, name: Sym, v: Var) {
        if let Some(e) = self.f.iter_mut().find(|(n, _)| *n == name) { e.1 = v; } else { self.f.push((name, v)); }
    }
    pub fn unify(&mut self, other: &mut FeatureMap, vars: &mut Vars) -> Result<(), UnifyFail> {
        let names: Vec<Sym> = self.f.iter().map(|x| x.0.clone()).collect();
        for n in &names { let a = self.get(vars, n); let b = other.get(vars, n); vars.unify(a, b)?; }
        let names: Vec<Sym> = other.f.iter().map(|x| x.0.clone()).collect();
        for n in &names { let a = self.get(vars, n); let b = other.get(vars, n); vars.unify(a, b)?; }
        Ok(())
    }
    pub fn try_unify(&self, other: &mut FeatureMap, vars: &mut Vars) -> Result<(), UnifyFail> {
        for (n, a) in &self.f { let b = other.get(vars, n); vars.unify(*a, b)?; }
        Ok(())
    }
    pub fn is_similar(&self, other: &FeatureMap, vars: &mut Vars) -> bool {
        for (n, a) in &self.f {
            let s1 = vars.get(*a);
            let s2 = other.get_opt(n).and_then(|b| vars.get(b));
            if let (Some(x), Some(y)) = (s1, s2) { if x != y { return false; } }
        }
        true
    }
    pub fn can_unify(&self, other: &FeatureMap, vars: &mut Vars) -> bool {
        if !self.is_similar(other, vars) { return false; }
        let mut m = VarMap::new();
        let a = self.deep_copy(vars, &mut m);
        let mut m2 = VarMap::new();
        let mut b = other.deep_copy(vars, &mut m2);
        a.try_unify(&mut b, vars).is_ok()
    }
    pub fn skolemize(&self, vars: &mut Vars) {
        for (_, v) in &self.f {
            if vars.get(*v).is_none() { let s = vars.skolem(); let k = vars.fresh(Some(s)); let _ = vars.unify(*v, k); }
        }
    }
    pub fn deep_copy(&self, vars: &mut Vars, m: &mut VarMap) -> FeatureMap {
        FeatureMap { f: self.f.iter().map(|(n, v)| (n.clone(), copy_var(vars, *v, m))).collect() }
    }
    /// Java `FeatureMap.getIdentifier`.
    fn identifier_into(&self, vars: &mut Vars, mvars: &[Var], used: &[Sym], s: &mut String) {
        s.push('(');
        // `used` is sorted; when the map is sorted too, walk both in one pass (merge-join) instead of
        // looking every used name up by string comparison
        if self.f.windows(2).all(|w| w[0].0 <= w[1].0) {
            let (mut i, mut j) = (0, 0);
            while i < used.len() && j < self.f.len() {
                let (n, v) = (&used[i], self.f[j].1);
                let o = if Rc::ptr_eq(n, &self.f[j].0) { std::cmp::Ordering::Equal } else { (**n).cmp(&*self.f[j].0) };
                match o {
                    std::cmp::Ordering::Less => { i += 1; }
                    std::cmp::Ordering::Greater => { j += 1; }
                    std::cmp::Ordering::Equal => {
                        let r = vars.find(v);
                        match &vars.val[r as usize] {
                            None => if let Some(k) = mvars.iter().position(|x| *x == r) { push_num(s, i + 1); s.push(':'); push_num(s, k); s.push(','); },
                            Some(val) => { push_num(s, i + 1); s.push(':'); s.push_str(val); s.push(','); }
                        }
                        i += 1; j += 1;
                    }
                }
            }
            s.push(')');
            return;
        }
        for (i, n) in used.iter().enumerate() {
            let Some(v) = self.get_opt(n) else { continue };
            match vars.get(v) {
                None => {
                    let r = vars.find(v);
                    if let Some(k) = mvars.iter().position(|x| *x == r) { push_num(s, i + 1); s.push(':'); push_num(s, k); s.push(','); }
                }
                Some(val) => { push_num(s, i + 1); s.push(':'); s.push_str(&val); s.push(','); }
            }
        }
        s.push(')');
    }
    pub fn to_string(&self, vars: &mut Vars) -> String {
        if self.f.is_empty() { return String::new(); }
        let mut items: Vec<_> = self.f.iter().collect();
        items.sort_by(|a, b| a.0.cmp(&b.0));
        let parts: Vec<String> = items.iter().map(|(n, v)| match vars.get(*v) {
            Some(s) => format!("{n}:{s}"), None => format!("{n}:{}", vars.find(*v)) }).collect();
        format!("({})", parts.join(","))
    }
}

/// Small variable-renaming map for deep copies (an edge has only a handful of variables, so a
/// linear scan beats hashing).
#[derive(Debug, Default)]
pub struct VarMap(Vec<(u32, u32)>);
impl VarMap {
    pub fn new() -> Self { VarMap(Vec::with_capacity(8)) }
    #[inline] pub fn get(&self, k: &u32) -> Option<&u32> { self.0.iter().find(|x| x.0 == *k).map(|x| &x.1) }
    #[inline] pub fn insert(&mut self, k: u32, v: u32) { self.0.push((k, v)); }
    #[inline] pub fn get_or_insert_with(&mut self, k: u32, f: impl FnOnce() -> u32) -> u32 {
        if let Some(v) = self.get(&k) { return *v; }
        let v = f(); self.0.push((k, v)); v
    }
}

/// FxHash (rustc's hasher): fast, non-cryptographic; chart keys are not attacker-controlled.
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);
impl std::hash::Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        const K: u64 = 0x517c_c1b7_2722_0a95;
        let mut c = bytes.chunks_exact(8);
        for w in &mut c { self.0 = (self.0.rotate_left(5) ^ u64::from_le_bytes(w.try_into().unwrap())).wrapping_mul(K); }
        for b in c.remainder() { self.0 = (self.0.rotate_left(5) ^ *b as u64).wrapping_mul(K); }
    }
    #[inline] fn write_u64(&mut self, i: u64) { self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x517c_c1b7_2722_0a95); }
    #[inline] fn write_usize(&mut self, i: usize) { self.write_u64(i as u64); }
    #[inline] fn write_u32(&mut self, i: u32) { self.write_u64(i as u64); }
    #[inline] fn finish(&self) -> u64 { self.0 }
}
pub type FxBuild = std::hash::BuildHasherDefault<FxHasher>;

pub fn copy_var(vars: &mut Vars, v: Var, m: &mut VarMap) -> Var {
    let r = vars.find(v);
    if let Some(x) = m.get(&r) { return *x; }
    let val = vars.val[r as usize].clone();
    let n = vars.fresh(val);
    m.insert(r, n);
    n
}

#[inline]
pub fn push_num(s: &mut String, mut n: usize) {
    if n < 10 { s.push((b'0' + n as u8) as char); return; }
    let mut buf = [0u8; 20]; let mut i = 20;
    while n > 0 { i -= 1; buf[i] = b'0' + (n % 10) as u8; n /= 10; }
    s.push_str(std::str::from_utf8(&buf[i..]).unwrap());
}

pub const SPECIAL: &[&str] = &[">", ">>", "<", "/<", "//", "#"];

#[derive(Clone, Debug)]
pub enum Cat {
    Term(Sym),
    Pre(Sym, FeatureMap),
    Non(Sym, FeatureMap),
    /// Backward reference `<(+(..), -(..))`
    Bref { pos: Vec<FeatureMap>, neg: Vec<FeatureMap> },
}

impl Cat {
    pub fn name(&self) -> &str {
        match self { Cat::Term(n) | Cat::Pre(n, _) | Cat::Non(n, _) => n, Cat::Bref { .. } => "<" }
    }
    fn kind(&self) -> u8 { match self { Cat::Term(_) => 0, Cat::Pre(..) => 1, Cat::Non(..) => 2, Cat::Bref { .. } => 3 } }
    pub fn is_special(&self) -> bool { SPECIAL.contains(&self.name()) }
    pub fn is_nonterminal(&self) -> bool { matches!(self, Cat::Non(..) | Cat::Bref { .. }) }
    pub fn fm(&self) -> Option<&FeatureMap> { match self { Cat::Pre(_, f) | Cat::Non(_, f) => Some(f), _ => None } }
    pub fn fm_mut(&mut self) -> Option<&mut FeatureMap> { match self { Cat::Pre(_, f) | Cat::Non(_, f) => Some(f), _ => None } }
    fn same_head(&self, o: &Cat) -> bool { self.kind() == o.kind() && self.name() == o.name() }

    pub fn deep_copy(&self, vars: &mut Vars, m: &mut VarMap) -> Cat {
        match self {
            Cat::Term(n) => Cat::Term(n.clone()),
            Cat::Pre(n, f) => Cat::Pre(n.clone(), f.deep_copy(vars, m)),
            Cat::Non(n, f) => Cat::Non(n.clone(), f.deep_copy(vars, m)),
            Cat::Bref { pos, neg } => Cat::Bref {
                pos: pos.iter().map(|f| f.deep_copy(vars, m)).collect(),
                neg: neg.iter().map(|f| f.deep_copy(vars, m)).collect(),
            },
        }
    }
    pub fn copy1(&self, vars: &mut Vars) -> Cat { self.deep_copy(vars, &mut VarMap::new()) }

    pub fn unify(&mut self, o: &mut Cat, vars: &mut Vars) -> Result<(), UnifyFail> {
        match (self, o) {
            (Cat::Term(a), Cat::Term(b)) => if a == b { Ok(()) } else { Err(UnifyFail) },
            (Cat::Bref { pos: p1, neg: n1 }, Cat::Bref { pos: p2, neg: n2 }) => {
                if p1.len() != p2.len() { return Err(UnifyFail); }
                for (a, b) in p1.iter_mut().zip(p2.iter_mut()) { a.unify(b, vars)?; }
                if n1.len() != n2.len() { return Err(UnifyFail); }
                for (a, b) in n1.iter_mut().zip(n2.iter_mut()) { a.unify(b, vars)?; }
                Ok(())
            }
            (s, o) => {
                if !s.same_head(o) { return Err(UnifyFail); }
                match (s.fm_mut(), o.fm_mut()) { (Some(a), Some(b)) => a.unify(b, vars), _ => Err(UnifyFail) }
            }
        }
    }
    pub fn try_unify(&self, o: &mut Cat, vars: &mut Vars) -> Result<(), UnifyFail> {
        match (self, o) {
            (Cat::Term(a), Cat::Term(b)) => if a == b { Ok(()) } else { Err(UnifyFail) },
            (Cat::Bref { pos: p1, neg: n1 }, Cat::Bref { pos: p2, neg: n2 }) => {
                if p1.len() != p2.len() { return Err(UnifyFail); }
                for (a, b) in p1.iter().zip(p2.iter_mut()) { a.try_unify(b, vars)?; }
                if n1.len() != n2.len() { return Err(UnifyFail); }
                for (a, b) in n1.iter().zip(n2.iter_mut()) { a.try_unify(b, vars)?; }
                Ok(())
            }
            (s, o) => {
                if !s.same_head(o) { return Err(UnifyFail); }
                match (s.fm(), o.fm_mut()) { (Some(a), Some(b)) => a.try_unify(b, vars), _ => Err(UnifyFail) }
            }
        }
    }
    pub fn is_similar(&self, o: &Cat, vars: &mut Vars) -> bool {
        match (self, o) {
            (Cat::Term(a), Cat::Term(b)) => a == b,
            (Cat::Bref { pos: p1, .. }, Cat::Bref { pos: p2, .. }) => p1.len() == p2.len(),
            (Cat::Bref { .. }, _) | (_, Cat::Bref { .. }) => false,
            (s, o) => {
                if !s.same_head(o) { return false; }
                let (a, b) = (s.fm().unwrap(), o.fm().unwrap());
                if a.f.is_empty() || b.f.is_empty() { return true; }
                a.is_similar(b, vars)
            }
        }
    }
    pub fn can_unify(&self, o: &Cat, vars: &mut Vars) -> bool {
        if let (Cat::Term(a), Cat::Term(b)) = (self, o) { return a == b; }
        if !self.is_similar(o, vars) { return false; }
        let a = self.copy1(vars);
        let mut b = o.copy1(vars);
        a.try_unify(&mut b, vars).is_ok()
    }
    pub fn subsumes(&self, o: &Cat, vars: &mut Vars) -> bool {
        match (self, o) {
            (Cat::Term(a), Cat::Term(b)) => a == b,
            (Cat::Bref { .. }, Cat::Bref { .. }) => {
                let a = self.copy1(vars); let mut b = o.copy1(vars); b.skolemize(vars);
                a.try_unify(&mut b, vars).is_ok()
            }
            (Cat::Bref { .. }, _) => false,
            (s, o) => {
                if !s.same_head(o) || o.fm().is_none() { return false; }
                if s.fm().map_or(true, |f| f.f.is_empty()) { return true; }
                if !s.fm().unwrap().is_similar(o.fm().unwrap(), vars) { return false; }
                let a = s.copy1(vars); let mut b = o.copy1(vars); b.skolemize(vars);
                a.try_unify(&mut b, vars).is_ok()
            }
        }
    }
    pub fn skolemize(&self, vars: &mut Vars) {
        match self {
            Cat::Term(_) => {}
            Cat::Pre(_, f) | Cat::Non(_, f) => f.skolemize(vars),
            Cat::Bref { pos, neg } => { for f in pos.iter().chain(neg) { f.skolemize(vars); } }
        }
    }
    /// Java `Category.collectVars` (values in a stable order: feature-name order within a map).
    pub fn collect_vars(&self, vars: &mut Vars, seen: &mut Vec<Var>, mvars: &mut Vec<Var>) {
        // variables in feature-name order (Java iterates sorted feature maps)
        fn visit(f: &FeatureMap, vars: &mut Vars, seen: &mut Vec<Var>, mvars: &mut Vec<Var>) {
            let mut one = |v: Var, vars: &mut Vars| {
                let r = vars.find(v);
                if vars.val[r as usize].is_some() { return; }
                if seen.contains(&r) { if !mvars.contains(&r) { mvars.push(r); } } else { seen.push(r); }
            };
            if f.f.windows(2).all(|w| w[0].0 <= w[1].0) { for (_, v) in &f.f { one(*v, vars); } }
            else { for v in sorted_vals(f) { one(v, vars); } }
        }
        match self {
            Cat::Term(_) => {}
            Cat::Pre(_, f) | Cat::Non(_, f) => visit(f, vars, seen, mvars),
            Cat::Bref { pos, neg } => { for f in pos.iter().chain(neg) { visit(f, vars, seen, mvars); } }
        }
    }
    pub fn identifier(&self, vars: &mut Vars, mvars: &[Var], used: &[Sym]) -> String {
        let mut s = String::with_capacity(32);
        self.identifier_into(vars, mvars, used, &mut s);
        s
    }
    /// Java `Category.getIdentifier`, appended to `s` (no intermediate strings).
    pub fn identifier_into(&self, vars: &mut Vars, mvars: &[Var], used: &[Sym], s: &mut String) {
        match self {
            Cat::Term(n) => { s.push('\''); s.push_str(n); s.push('\''); }
            Cat::Pre(n, f) | Cat::Non(n, f) => { s.push_str(n); f.identifier_into(vars, mvars, used, s); }
            Cat::Bref { pos, neg } => {
                s.push_str("<+");
                for f in pos { f.identifier_into(vars, mvars, used, s); }
                s.push('-');
                for f in neg { f.identifier_into(vars, mvars, used, s); }
            }
        }
    }
    /// Java `toString()` (used for abstract-option parity with the Java oracle).
    pub fn display(&self, vars: &mut Vars) -> String {
        match self {
            Cat::Term(n) => format!("'{n}'"),
            Cat::Pre(n, f) => format!("${n}{}", f.to_string(vars)),
            Cat::Non(n, f) => format!("{n}{}", f.to_string(vars)),
            Cat::Bref { pos, neg } => {
                let mut s = String::from("<+");
                for f in pos { s += &f.to_string(vars); }
                s += "-";
                for f in neg { s += &f.to_string(vars); }
                s
            }
        }
    }
}

fn sorted_vals(f: &FeatureMap) -> Vec<Var> {
    let mut items: Vec<_> = f.f.iter().collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items.into_iter().map(|x| x.1).collect()
}
