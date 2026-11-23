//! Fast evaluator with the same semantics as [`crate::reference::Engine`].
//!
//! What makes it fast:
//! * object ids, types, relations and names are **interned** to `u32` once; a frame is evaluated
//!   on integers (no string hashing or comparison in the hot loop);
//! * per-frame indexes (objects by type, fact set, camera map) are **reused** across frames, so a
//!   steady stream allocates almost nothing;
//! * bindings are enumerated with an odometer over candidate lists (no per-binding hash maps);
//!   duration timers are keyed by `(rule, condition)` then the binding slice, looked up without
//!   allocating;
//! * proof text is built only for alerts that are actually raised;
//! * unused symbols (tracks that left long ago) are released periodically, so long runs do not grow.
//!
//! Frames come in either as [`Frame`] values or through the zero-copy builder
//! ([`Engine::begin`] / [`Engine::object`] / [`Engine::fact`] / [`Engine::end`]) used by the C ABI.

use crate::reference::{AlertEvent, Frame, EPS};
use crate::{NounKind, Rule, Scope};
use std::collections::BTreeSet;
use std::hash::{BuildHasherDefault, Hasher};

/// Fx-style hasher (as used inside rustc): a multiply-rotate per word. Internal keys are small
/// integers and short strings under our control, so DoS resistance (SipHash) is not needed here.
#[derive(Default, Clone, Copy)]
pub struct Fx(u64);
const K: u64 = 0x51_7c_c1_b7_27_22_0a_95;
impl Hasher for Fx {
    #[inline] fn finish(&self) -> u64 { self.0 }
    #[inline] fn write(&mut self, bytes: &[u8]) {
        let mut c = bytes.chunks_exact(8);
        for w in &mut c { self.add(u64::from_le_bytes(w.try_into().unwrap())); }
        let r = c.remainder();
        if !r.is_empty() { let mut b = [0u8; 8]; b[..r.len()].copy_from_slice(r); self.add(u64::from_le_bytes(b)); }
    }
    #[inline] fn write_u8(&mut self, i: u8) { self.add(i as u64) }
    #[inline] fn write_u32(&mut self, i: u32) { self.add(i as u64) }
    #[inline] fn write_u64(&mut self, i: u64) { self.add(i) }
    #[inline] fn write_usize(&mut self, i: usize) { self.add(i as u64) }
}
impl Fx { #[inline] fn add(&mut self, w: u64) { self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(K); } }
type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<Fx>>;
type HashSet<K> = std::collections::HashSet<K, BuildHasherDefault<Fx>>;

pub type Sym = u32;
pub const NONE: Sym = u32::MAX;

/// String interner with slot reuse.
#[derive(Default)]
pub struct Interner { map: HashMap<String, Sym>, names: Vec<Option<String>>, free: Vec<Sym> }

impl Interner {
    pub fn intern(&mut self, s: &str) -> Sym {
        if let Some(&x) = self.map.get(s) { return x; }
        let id = match self.free.pop() {
            Some(f) => { self.names[f as usize] = Some(s.to_string()); f }
            None => { self.names.push(Some(s.to_string())); (self.names.len() - 1) as Sym }
        };
        self.map.insert(s.to_string(), id);
        id
    }
    pub fn get(&self, s: &str) -> Option<Sym> { self.map.get(s).copied() }
    pub fn name(&self, x: Sym) -> &str { self.names.get(x as usize).and_then(|n| n.as_deref()).unwrap_or("?") }
    fn release(&mut self, x: Sym) { if let Some(n) = self.names[x as usize].take() { self.map.remove(&n); self.free.push(x); } }
    pub fn len(&self) -> usize { self.map.len() }
    pub fn is_empty(&self) -> bool { self.map.is_empty() }
}

/// Object kinds (also the C ABI's `kind` argument).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum Kind { Class = 0, Zone = 1, Signal = 2 }

#[derive(Clone, Copy)]
struct SObj { id: Sym, ty: Sym, cam: Sym }

#[derive(Default)]
struct SFrame { t: f64, cam: Sym, objs: Vec<SObj>, facts: Vec<(Sym, Sym, Sym)> }

#[derive(Clone, Copy, PartialEq)]
enum Mode { Count(u32), Inner, Plain }

struct CEnt { var: String, ty: Sym, inst: Sym, cam: Sym, bpos: Option<usize> }
struct CCond { rel: Sym, s: usize, o: usize, negated: bool, min_seconds: Option<f64>, exception: bool, mode: Mode, line: String }
enum Subj { Outer(usize), Scene }
struct CRule { id: String, text: String, ents: Vec<CEnt>, outer: Vec<usize>, conds: Vec<CCond>, subj: Subj }

#[derive(Clone, Copy)]
struct TState { start: f64, last: f64, seen: u64 }

#[derive(Clone, Copy, PartialEq)]
enum St { Active, Cooling(f64) }

pub struct Engine {
    pub rules: Vec<Rule>,
    crules: Vec<CRule>,
    sy: Interner,
    scene: Sym,
    grace: f64,
    cooldown: f64,
    /// evidence smoothing (default on); off = yappu-fsm-compatible timers
    pub smoothing: bool,
    /// when false, durations are ignored (per-frame baseline)
    pub use_durations: bool,
    // persistent state
    recent: HashMap<Sym, HashMap<Sym, f64>>,
    fact_seen: HashMap<(Sym, Sym, Sym), f64>,
    timers: Vec<Vec<HashMap<Box<[Sym]>, TState>>>,
    alerts: HashMap<(u32, Sym), St>,
    frame_no: u64,
    // reused per-frame buffers
    sf: SFrame,
    by_type: HashMap<Sym, Vec<Sym>>,
    facts: HashSet<(Sym, Sym, Sym)>,
    camera: HashMap<Sym, Sym>,
    cands: Vec<Vec<Sym>>,
    derived: HashSet<(u32, Sym)>,
    pending: HashMap<(u32, Sym), (Vec<Sym>, Vec<f64>, Vec<bool>)>,
    events: Vec<AlertEvent>,
    /// raw type name -> type symbol, per kind (lookup by &str, no allocation)
    tycache: [HashMap<String, Sym>; 3],
}

fn type_key(kind: Kind, ty: &str) -> String { match kind { Kind::Class => format!("c:{ty}"), Kind::Zone => format!("z:{ty}"), Kind::Signal => format!("s:{ty}") } }

impl Engine {
    pub fn new(rules: Vec<Rule>, grace: f64, cooldown: f64) -> Self {
        let mut sy = Interner::default();
        let scene = sy.intern("scene");
        let crules = rules.iter().map(|r| compile_rule(r, &mut sy)).collect::<Vec<_>>();
        let timers = crules.iter().map(|r| r.conds.iter().map(|_| HashMap::default()).collect()).collect();
        Engine { rules, crules, sy, scene, grace, cooldown, smoothing: true, use_durations: true,
            recent: HashMap::default(), fact_seen: HashMap::default(), timers, alerts: HashMap::default(), frame_no: 0,
            sf: SFrame::default(), by_type: HashMap::default(), facts: HashSet::default(), camera: HashMap::default(),
            cands: vec![], derived: HashSet::default(), pending: HashMap::default(), events: vec![], tycache: Default::default() }
    }
    /// Load the `config.json` exported by iogp-editor.
    pub fn from_config(json: &str) -> Result<Self, crate::ConfigError> {
        let rs = crate::load_config(json)?;
        Ok(Self::new(rs.rules, rs.grace_seconds, rs.cooldown_seconds))
    }
    /// yappu-fsm-compatible semantics (for parity checks).
    pub fn yappu_compatible(mut self) -> Self { self.smoothing = false; self }
    pub fn grace(&self) -> f64 { self.grace }
    pub fn symbols(&self) -> usize { self.sy.len() }

    fn ty_sym(&mut self, kind: Kind, ty: &str) -> Sym {
        if let Some(&x) = self.tycache[kind as usize].get(ty) { return x; }
        let x = self.sy.intern(&type_key(kind, ty));
        self.tycache[kind as usize].insert(ty.to_string(), x);
        x
    }

    // ------------------------------------------------------------ zero-copy frame builder
    pub fn begin(&mut self, t: f64, camera: Option<&str>) {
        self.sf.t = t;
        self.sf.cam = camera.map(|c| self.sy.intern(c)).unwrap_or(NONE);
        self.sf.objs.clear();
        self.sf.facts.clear();
    }
    /// Add an object (call once per type it has; every type is remembered through missed frames).
    pub fn object(&mut self, id: &str, kind: Kind, ty: &str, camera: Option<&str>) {
        let id = self.sy.intern(id);
        let ty = self.ty_sym(kind, ty);
        let cam = camera.map(|c| self.sy.intern(c)).unwrap_or(NONE);
        self.sf.objs.push(SObj { id, ty, cam });
    }
    pub fn fact(&mut self, rel: &str, a: &str, b: &str) {
        let f = (self.sy.intern(rel), self.sy.intern(a), self.sy.intern(b));
        self.sf.facts.push(f);
    }
    /// Evaluate the frame built with begin/object/fact; the events stay available until the next frame.
    pub fn end(&mut self) -> &[AlertEvent] { self.eval(); &self.events }
    /// Like [`Engine::end`] but hands the events over (the C ABI copies them).
    pub fn end_events(&mut self) -> Vec<AlertEvent> { self.eval(); std::mem::take(&mut self.events) }

    /// Evaluate one frame; returns raised and cleared alert events.
    pub fn step(&mut self, f: &Frame) -> Vec<AlertEvent> {
        self.begin(f.t, f.camera.as_deref());
        for o in &f.objects {
            let cam = o.camera.as_deref();
            let mut first = true;
            for (k, ty) in [(Kind::Class, &o.class), (Kind::Zone, &o.zone), (Kind::Signal, &o.signal)] {
                if let Some(ty) = ty {
                    let id = self.sy.intern(&o.id);
                    let tys = self.ty_sym(k, ty);
                    let c = cam.map(|c| self.sy.intern(c)).unwrap_or(NONE);
                    self.sf.objs.push(SObj { id, ty: tys, cam: c });
                    first = false;
                }
            }
            if first {
                // an object with no type still has a camera for the camera map
                let id = self.sy.intern(&o.id);
                let c = cam.map(|c| self.sy.intern(c)).unwrap_or(NONE);
                self.sf.objs.push(SObj { id, ty: NONE, cam: c });
            }
        }
        for (r, a, b) in &f.facts { self.fact(r, a, b); }
        self.eval();
        std::mem::take(&mut self.events)
    }

    pub fn active(&self) -> BTreeSet<(String, String)> {
        self.alerts.iter().filter(|(_, s)| **s == St::Active).map(|((r, s), _)| (self.crules[*r as usize].id.clone(), self.sy.name(*s).to_string())).collect()
    }

    // ------------------------------------------------------------ evaluation
    fn eval(&mut self) {
        let t = self.sf.t;
        let g = self.grace;
        self.frame_no += 1;
        self.events.clear();
        let durations = self.use_durations || self.smoothing;
        let smooth_facts = self.smoothing && self.use_durations;
        // world: objects by type, camera map, believed facts
        for v in self.by_type.values_mut() { v.clear(); }
        self.camera.clear();
        self.facts.clear();
        for o in &self.sf.objs {
            let c = if o.cam != NONE { o.cam } else { self.sf.cam };
            if c != NONE { self.camera.insert(o.id, c); }
            if o.ty != NONE { self.by_type.entry(o.ty).or_default().push(o.id); }
        }
        if durations {
            for (ty, ids) in &self.recent {
                let v = self.by_type.entry(*ty).or_default();
                for (id, last) in ids { if t - last <= g + EPS && !v.contains(id) { v.push(*id); } }
            }
        }
        self.facts.extend(self.sf.facts.iter().copied());
        if smooth_facts { for (f, last) in &self.fact_seen { if t - last <= g + EPS { self.facts.insert(*f); } } }

        let timer_grace = if self.smoothing { 0.0 } else { g };
        self.derived.clear();
        self.pending.clear();
        let mut b: Vec<Sym> = Vec::new();
        let mut held: Vec<f64> = Vec::new();
        let mut okv: Vec<bool> = Vec::new();
        for ri in 0..self.crules.len() {
            let r = &self.crules[ri];
            // candidates per entity (filtered by named instance / camera)
            while self.cands.len() < r.ents.len() { self.cands.push(Vec::new()); }
            for (ei, e) in r.ents.iter().enumerate() {
                let out = &mut self.cands[ei];
                out.clear();
                if let Some(v) = self.by_type.get(&e.ty) {
                    for &x in v {
                        if e.inst != NONE && x != e.inst { continue; }
                        if e.cam != NONE && self.camera.get(&x) != Some(&e.cam) { continue; }
                        out.push(x);
                    }
                }
            }
            if r.outer.iter().any(|&ei| self.cands[ei].is_empty()) { continue; }
            // odometer over outer candidates, first outer entity most significant (as the reference)
            let n = r.outer.len();
            let mut idx = vec![0usize; n];
            b.clear(); b.resize(n, NONE);
            held.clear(); held.resize(r.conds.len(), 0.0);
            okv.clear(); okv.resize(r.conds.len(), false);
            loop {
                for (p, &ei) in r.outer.iter().enumerate() { b[p] = self.cands[ei][idx[p]]; }
                let mut all = true;
                for (ci, c) in r.conds.iter().enumerate() {
                    let v = holds(c, r, &b, &self.cands, &self.facts);
                    let ok = match (c.min_seconds, self.use_durations) {
                        (Some(d), true) => {
                            let tm = &mut self.timers[ri][ci];
                            // observe(v)
                            if v {
                                match tm.get_mut(&b[..]) {
                                    Some(s) => { s.last = t; s.seen = self.frame_no; }
                                    None => { tm.insert(b.clone().into_boxed_slice(), TState { start: t, last: t, seen: self.frame_no }); }
                                }
                            } else if let Some(s) = tm.get_mut(&b[..]) {
                                s.seen = self.frame_no;
                                if t - s.last > timer_grace { tm.remove(&b[..]); }
                            }
                            let h = match tm.get(&b[..]) { Some(s) if t - s.last <= timer_grace => s.last - s.start, _ => 0.0 };
                            held[ci] = h;
                            h + 1e-9 >= d
                        }
                        _ => v,
                    };
                    okv[ci] = ok;
                    if !ok { all = false; }
                }
                if all {
                    let subj = match r.subj { Subj::Outer(p) => b[p], Subj::Scene => self.scene };
                    let key = (ri as u32, subj);
                    if self.derived.insert(key) && !self.alerts.contains_key(&key) {
                        self.pending.insert(key, (b.clone(), held.clone(), okv.clone()));
                    }
                }
                // next binding (last position fastest; done when every position wrapped)
                let mut done = true;
                let mut p = n;
                while p > 0 {
                    p -= 1;
                    idx[p] += 1;
                    if idx[p] < self.cands[r.outer[p]].len() { done = false; break; }
                    idx[p] = 0;
                }
                if done { break; }
            }
        }
        // timers not evaluated this frame see `false`
        let fno = self.frame_no;
        for per_rule in &mut self.timers { for tm in per_rule { tm.retain(|_, s| s.seen == fno || t - s.last <= timer_grace); } }
        // remember what was seen (candidates and smoothed facts for later frames)
        // every type an object was seen with stays believed for `grace` — not only its first one
        for o in &self.sf.objs { if o.ty != NONE { self.recent.entry(o.ty).or_default().insert(o.id, t); } }
        for ids in self.recent.values_mut() { ids.retain(|_, last| t - *last <= g); }
        if self.smoothing {
            for f in &self.sf.facts { self.fact_seen.insert(*f, t); }
            self.fact_seen.retain(|_, last| t - *last <= g);
        }
        // alert machine
        let mut raised: Vec<(u32, Sym)> = self.derived.iter().filter(|k| !self.alerts.contains_key(k)).copied().collect();
        for k in &self.derived { self.alerts.insert(*k, St::Active); }
        let mut cleared = vec![];
        let cd = self.cooldown;
        let derived = &self.derived;
        self.alerts.retain(|k, s| {
            if derived.contains(k) { return true; }
            match *s {
                St::Active => { *s = St::Cooling(t); true }
                St::Cooling(since) if t - since >= cd => { cleared.push(*k); false }
                _ => true,
            }
        });
        let name = |k: &(u32, Sym), sy: &Interner, cr: &[CRule]| (cr[k.0 as usize].id.clone(), sy.name(k.1).to_string());
        raised.sort_by_cached_key(|k| name(k, &self.sy, &self.crules));
        cleared.sort_by_cached_key(|k| name(k, &self.sy, &self.crules));
        for k in raised {
            let proof = self.proof(k);
            let ev = self.event("raised", k, t, proof);
            self.events.push(ev);
        }
        for k in cleared { let ev = self.event("cleared", k, t, vec![]); self.events.push(ev); }
        if self.frame_no % 1024 == 0 { self.collect_garbage(); }
    }

    fn event(&self, kind: &str, k: (u32, Sym), t: f64, proof: Vec<String>) -> AlertEvent {
        let r = &self.rules[k.0 as usize];
        AlertEvent { kind: kind.into(), rule: r.id.clone(), alert: r.alert.noun.clone(), lsr: r.alert.lsr, severity: r.alert.severity.clone(), subject: self.sy.name(k.1).to_string(), t, proof }
    }

    fn proof(&self, k: (u32, Sym)) -> Vec<String> {
        let Some((b, held, okv)) = self.pending.get(&k) else { return vec![] };
        let r = &self.crules[k.0 as usize];
        let bkey: String = r.outer.iter().enumerate().map(|(p, &ei)| format!("{}={}", r.ents[ei].var, self.sy.name(b[p]))).collect::<Vec<_>>().join(",");
        let mut out = vec![format!("rule {}: {} [{}]", r.id, r.text, bkey)];
        for (ci, c) in r.conds.iter().enumerate() {
            if !okv[ci] { continue; }
            match (c.min_seconds, self.use_durations) {
                (Some(_), true) => out.push(format!("{} (held {:.1} s)", c.line, held[ci])),
                _ => out.push(if c.exception { format!("exception not met: {}", c.line.trim_start_matches("unless ")) } else { c.line.clone() }),
            }
        }
        out
    }

    /// Release symbols no longer referenced (tracks that left, old facts).
    fn collect_garbage(&mut self) {
        let mut live = vec![false; self.sy.names.len()];
        let mut mark = |x: Sym| if (x as usize) < live.len() { live[x as usize] = true; };
        mark(self.scene);
        for r in &self.crules { for e in &r.ents { mark(e.ty); mark(e.inst); mark(e.cam); } for c in &r.conds { mark(c.rel); } }
        for (ty, ids) in &self.recent { mark(*ty); for id in ids.keys() { mark(*id); } }
        for (a, b, c) in self.fact_seen.keys() { mark(*a); mark(*b); mark(*c); }
        for per_rule in &self.timers { for tm in per_rule { for k in tm.keys() { for x in k.iter() { mark(*x); } } } }
        for (_, s) in self.alerts.keys() { mark(*s); }
        // type keys stay (few, and by_type is keyed by them)
        for ty in self.by_type.keys() { mark(*ty); }
        for c in &self.tycache { for x in c.values() { mark(*x); } }
        for (x, l) in live.iter().enumerate() { if !l { self.sy.release(x as Sym); } }
    }
}

fn holds(c: &CCond, r: &CRule, b: &[Sym], cands: &[Vec<Sym>], facts: &HashSet<(Sym, Sym, Sym)>) -> bool {
    let bound = |ei: usize| r.ents[ei].bpos.map(|p| b[p]).unwrap_or(NONE);
    let v = match c.mode {
        Mode::Count(k) => { let ob = bound(c.o); cands[c.s].iter().filter(|x| facts.contains(&(c.rel, **x, ob))).count() >= k as usize }
        Mode::Inner => { let sb = bound(c.s); cands[c.o].iter().any(|x| facts.contains(&(c.rel, sb, *x))) }
        Mode::Plain => facts.contains(&(c.rel, bound(c.s), bound(c.o))),
    };
    v != c.negated
}

fn compile_rule(r: &Rule, sy: &mut Interner) -> CRule {
    let mut outer = vec![];
    let ents: Vec<CEnt> = r.entities.iter().map(|e| {
        let tk = match &e.kind {
            NounKind::Class { detector, .. } => type_key(Kind::Class, detector),
            NounKind::Zone => type_key(Kind::Zone, &e.noun),
            NounKind::Signal { .. } => type_key(Kind::Signal, &e.noun),
            _ => "none".into(),
        };
        let bpos = (e.scope == Scope::Outer).then(|| { outer.push(outer.len()); outer.len() - 1 });
        CEnt { var: e.var.clone(), ty: sy.intern(&tk), inst: e.instance.as_deref().map(|x| sy.intern(x)).unwrap_or(NONE), cam: e.camera.as_deref().map(|x| sy.intern(x)).unwrap_or(NONE), bpos }
    }).collect();
    let outer: Vec<usize> = r.entities.iter().enumerate().filter(|(_, e)| e.scope == Scope::Outer).map(|(i, _)| i).collect();
    let ix = |v: &str| r.entities.iter().position(|e| e.var == v).expect("validated");
    let conds = r.conditions.iter().enumerate().map(|(i, c)| {
        let (s, o) = (ix(&c.subject), ix(&c.object));
        let mode = if let Some(k) = r.entities[s].min_count { Mode::Count(k) } else if r.entities[o].scope != Scope::Outer { Mode::Inner } else { Mode::Plain };
        CCond { rel: sy.intern(c.kind.relation()), s, o, negated: c.negated, min_seconds: c.min_seconds, exception: c.exception, mode, line: r.readback.get(i + 1).cloned().unwrap_or_default() }
    }).collect();
    let pos = |ei: usize| outer.iter().position(|&x| x == ei);
    let subj = match &r.alert.subject {
        Some(v) => pos(ix(v)).map(Subj::Outer).unwrap_or(Subj::Scene),
        None => outer.iter().position(|&ei| matches!(r.entities[ei].kind, NounKind::Zone)).map(Subj::Outer).unwrap_or(Subj::Scene),
    };
    CRule { id: r.id.clone(), text: r.text.clone(), ents, outer, conds, subj }
}
