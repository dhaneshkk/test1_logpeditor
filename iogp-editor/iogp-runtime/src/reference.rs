//! Reference engine: the executable semantics of a compiled rule set. Written for clarity, not
//! speed; [`crate::fast::Engine`] must produce the same events (tests/differential.rs).
//!
//! Input, per frame, is what the detection framework already computes:
//!
//! ```json
//! {"t": 12.4,
//!  "objects": [{"id": "7", "class": "person"}, {"id": "z1", "zone": "height-zone"}, {"id": "ptw-1", "signal": "permit"}],
//!  "facts": [["in", "7", "z1"], ["wear", "7", "h3"], ["have", "z1", "ptw-1"]]}
//! ```
//!
//! The semantics are those of yappu-fsm's vision events, generalised to the compiled rules:
//! a [`Timer`] per (rule, binding, condition) measures how long a condition has held, tolerating
//! gaps shorter than `grace`; an [`AlertMachine`] raises each (rule, subject) alert once and
//! clears it after `cooldown`. The time-indexed FOL emitted by the compiler states the same
//! condition: `within(S, T, D) => phi(S)` over the observed frames.

use crate::{Cond, CondKind, Entity, NounKind, Rule, Scope};
use serde::{Deserialize, Serialize};
/// Tolerance for comparing frame times (timestamps such as 2.9 - 2.2 are not exact in binary).
pub const EPS: f64 = 1e-9;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
pub struct Object {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub zone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub signal: Option<String>,
    /// camera the object (typically a zone polygon) belongs to; defaults to the frame's camera
    #[serde(default, skip_serializing_if = "Option::is_none")] pub camera: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
pub struct Frame {
    pub t: f64,
    pub objects: Vec<Object>,
    pub facts: Vec<(String, String, String)>,
    /// camera that produced this frame (a site-level frame merging several cameras leaves it empty
    /// and sets `camera` on the zone objects instead)
    #[serde(default, skip_serializing_if = "Option::is_none")] pub camera: Option<String>,
}

/// How long a condition has held, tolerating gaps shorter than `grace` (yappu `Timer`).
#[derive(Default)]
pub struct Timer { pub grace: f64, start: HashMap<String, f64>, last_true: HashMap<String, f64> }

impl Timer {
    pub fn new(grace: f64) -> Self { Timer { grace, ..Default::default() } }
    pub fn observe(&mut self, key: &str, value: bool, t: f64) {
        if value {
            self.start.entry(key.to_string()).or_insert(t);
            self.last_true.insert(key.to_string(), t);
        } else if let Some(l) = self.last_true.get(key) {
            if t - *l > self.grace { self.start.remove(key); self.last_true.remove(key); }
        }
    }
    pub fn duration(&self, key: &str, t: f64) -> f64 {
        match (self.start.get(key), self.last_true.get(key)) {
            (Some(s), Some(l)) if t - l <= self.grace => l - s,
            _ => 0.0,
        }
    }
    pub fn keys(&self) -> Vec<String> { self.start.keys().cloned().collect() }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AlertEvent { pub kind: String, pub rule: String, pub alert: String, pub lsr: u8, pub severity: Option<String>, pub subject: String, pub t: f64, pub proof: Vec<String> }

#[derive(Clone, Copy, Debug, PartialEq)]
enum St { Active, Cooling(f64) }

/// IDLE -> ACTIVE (raised once) -> COOLING -> IDLE after `cooldown` (yappu `AlertMachine`).
pub struct AlertMachine { pub cooldown: f64, state: BTreeMap<(String, String), St> }

impl AlertMachine {
    pub fn new(cooldown: f64) -> Self { AlertMachine { cooldown, state: BTreeMap::new() } }
    /// `derived`: (rule, subject) pairs whose conditions hold now. Returns (raised, cleared) keys.
    pub fn step(&mut self, derived: &BTreeSet<(String, String)>, t: f64) -> (Vec<(String, String)>, Vec<(String, String)>) {
        let (mut raised, mut cleared) = (vec![], vec![]);
        for k in derived {
            if !self.state.contains_key(k) { raised.push(k.clone()); }
            self.state.insert(k.clone(), St::Active);
        }
        let keys: Vec<_> = self.state.keys().cloned().collect();
        for k in keys {
            if derived.contains(&k) { continue; }
            match self.state[&k] {
                St::Active => { self.state.insert(k, St::Cooling(t)); }
                St::Cooling(since) if t - since >= self.cooldown => { self.state.remove(&k); cleared.push(k); }
                _ => {}
            }
        }
        (raised, cleared)
    }
    pub fn active(&self) -> BTreeSet<(String, String)> { self.state.iter().filter(|(_, s)| **s == St::Active).map(|(k, _)| k.clone()).collect() }
}

pub struct Engine {
    /// objects seen within the last `grace` seconds stay candidates for rule bindings, so a missed
    /// detection does not drop an alert whose duration timer is still running (as in yappu-fsm,
    /// where duration facts are produced for active timers of unseen tracks)
    recent: HashMap<String, HashMap<String, f64>>,
    /// last time each positive fact was observed (evidence smoothing)
    fact_seen: HashMap<(String, String, String), f64>,
    /// Evidence semantics (default). A positive fact stays believed for `grace` seconds after it
    /// was last observed; absence is "not believed present", so one real detection resets an
    /// absence timer. Off = yappu-fsm-compatible semantics (grace applied inside timers only).
    pub smoothing: bool,
    pub rules: Vec<Rule>,
    pub timer: Timer,
    pub alerts: AlertMachine,
    /// when false, durations are ignored (per-frame alerting baseline)
    pub use_durations: bool,
}

struct World<'a> {
    by_type: HashMap<String, Vec<&'a str>>, // "class:person", "zone:height-zone", "signal:permit"
    facts: HashSet<(&'a str, &'a str, &'a str)>,
    camera: HashMap<&'a str, &'a str>,      // object id -> camera
}

impl World<'_> {
    /// Candidate objects for an entity: by type, then restricted to a named zone instance and/or a camera.
    fn cands(&self, e: &Entity) -> Vec<&str> {
        let mut v = self.by_type.get(&type_key(e)).cloned().unwrap_or_default();
        if let Some(n) = &e.instance { v.retain(|x| x == n); }
        if let Some(c) = &e.camera { v.retain(|x| self.camera.get(x).is_some_and(|k| k == c)); }
        v
    }
}

fn type_key(e: &Entity) -> String {
    match &e.kind {
        NounKind::Class { detector, .. } => format!("class:{detector}"),
        NounKind::Zone => format!("zone:{}", e.noun),
        NounKind::Signal { .. } => format!("signal:{}", e.noun),
        _ => "none".into(),
    }
}

impl Engine {
    pub fn new(rules: Vec<Rule>, grace: f64, cooldown: f64) -> Self {
        Engine { recent: HashMap::new(), fact_seen: HashMap::new(), smoothing: true, rules, timer: Timer::new(grace), alerts: AlertMachine::new(cooldown), use_durations: true }
    }
    /// yappu-fsm-compatible semantics (for parity checks).
    pub fn yappu_compatible(mut self) -> Self { self.smoothing = false; self
    }

    fn world<'a>(f: &'a Frame, recent: &'a HashMap<String, HashMap<String, f64>>, fact_seen: &'a HashMap<(String, String, String), f64>, grace: f64, durations: bool, smoothing: bool) -> World<'a> {
        let mut by_type: HashMap<String, Vec<&str>> = HashMap::new();
        let mut camera: HashMap<&str, &str> = HashMap::new();
        for o in &f.objects {
            if let Some(c) = o.camera.as_deref().or(f.camera.as_deref()) { camera.insert(&o.id, c); }
            if let Some(c) = &o.class { by_type.entry(format!("class:{c}")).or_default().push(&o.id); }
            if let Some(z) = &o.zone { by_type.entry(format!("zone:{z}")).or_default().push(&o.id); }
            if let Some(s) = &o.signal { by_type.entry(format!("signal:{s}")).or_default().push(&o.id); }
        }
        if durations {
            for (ty, ids) in recent {
                let v = by_type.entry(ty.clone()).or_default();
                for (id, last) in ids { if f.t - last <= grace + EPS && !v.contains(&id.as_str()) { v.push(id.as_str()); } }
            }
        }
        let mut facts: HashSet<(&str, &str, &str)> = f.facts.iter().map(|(r, a, b)| (r.as_str(), a.as_str(), b.as_str())).collect();
        if smoothing {
            for ((r, a, b), last) in fact_seen { if f.t - last <= grace + EPS { facts.insert((r.as_str(), a.as_str(), b.as_str())); } }
        }
        World { by_type, facts, camera }
    }
    fn remember(&mut self, f: &Frame) {
        // every type an object was seen with stays believed for `grace` (an object may be a class and a zone)
        for o in &f.objects {
            let tys = [o.class.as_ref().map(|c| format!("class:{c}")), o.zone.as_ref().map(|z| format!("zone:{z}")), o.signal.as_ref().map(|s| format!("signal:{s}"))];
            for ty in tys.into_iter().flatten() { self.recent.entry(ty).or_default().insert(o.id.clone(), f.t); }
        }
        let g = self.timer.grace;
        for ids in self.recent.values_mut() { ids.retain(|_, last| f.t - *last <= g); }
        if self.smoothing {
            for x in &f.facts { self.fact_seen.insert(x.clone(), f.t); }
            self.fact_seen.retain(|_, last| f.t - *last <= g);
        }
    }

    fn holds(w: &World, c: &Cond, ent: &BTreeMap<&str, &Entity>, b: &HashMap<&str, &str>) -> bool {
        let rel = match &c.kind { CondKind::Spatial { relation, .. } | CondKind::Association { relation, .. } | CondKind::Signal { relation, .. } => relation.as_str() };
        let (s, o) = (ent[c.subject.as_str()], ent[c.object.as_str()]);
        let cands = |e: &Entity| w.cands(e);
        let v = if let Some(k) = s.min_count {
            let ob = b[c.object.as_str()];
            cands(s).iter().filter(|x| w.facts.contains(&(rel, **x, ob))).count() >= k as usize
        } else if o.scope != Scope::Outer {
            let sb = b[c.subject.as_str()];
            cands(o).iter().any(|x| w.facts.contains(&(rel, sb, *x)))
        } else {
            w.facts.contains(&(rel, b[c.subject.as_str()], b[c.object.as_str()]))
        };
        v != c.negated
    }

    /// Evaluate one frame; returns raised and cleared alert events.
    pub fn step(&mut self, f: &Frame) -> Vec<AlertEvent> {
        let recent = std::mem::take(&mut self.recent);
        let fact_seen = std::mem::take(&mut self.fact_seen);
        let w = Self::world(f, &recent, &fact_seen, self.timer.grace, self.use_durations || self.smoothing, self.smoothing && self.use_durations);
        // with evidence smoothing the gaps are already bridged at the fact level, so timers are exact
        let timer_grace = if self.smoothing { 0.0 } else { self.timer.grace };
        let t = f.t;
        let mut derived: BTreeSet<(String, String)> = BTreeSet::new();
        let mut proofs: HashMap<(String, String), Vec<String>> = HashMap::new();
        let mut seen: HashSet<String> = HashSet::new();
        for r in &self.rules {
            let ent: BTreeMap<&str, &Entity> = r.entities.iter().map(|e| (e.var.as_str(), e)).collect();
            let outer: Vec<&Entity> = r.entities.iter().filter(|e| e.scope == Scope::Outer).collect();
            // cartesian product of outer bindings
            let mut binds: Vec<HashMap<&str, &str>> = vec![HashMap::new()];
            for e in &outer {
                let cs = w.cands(e);
                let mut nb = vec![];
                for b in &binds { for x in &cs { let mut b2 = b.clone(); b2.insert(e.var.as_str(), *x); nb.push(b2); } }
                binds = nb;
            }
            for b in binds {
                let bkey: String = outer.iter().map(|e| format!("{}={}", e.var, b[e.var.as_str()])).collect::<Vec<_>>().join(",");
                let mut all = true;
                let mut proof = vec![];
                for (i, c) in r.conditions.iter().enumerate() {
                    let v = Self::holds(&w, c, &ent, &b);
                    let ok = match (c.min_seconds, self.use_durations) {
                        (Some(d), true) => {
                            let key = format!("{}|{}|{}", r.id, bkey, i);
                            let g = self.timer.grace; self.timer.grace = timer_grace;
                            self.timer.observe(&key, v, t);
                            seen.insert(key.clone());
                            let held = self.timer.duration(&key, t);
                            self.timer.grace = g;
                            if held + 1e-9 >= d { proof.push(format!("{} (held {:.1} s)", r.readback.get(i + 1).cloned().unwrap_or_default(), held)); true } else { false }
                        }
                        _ => {
                            if v {
                                let line = r.readback.get(i + 1).cloned().unwrap_or_default();
                                proof.push(if c.exception { format!("exception not met: {}", line.trim_start_matches("unless ")) } else { line });
                            }
                            v
                        }
                    };
                    if !ok { all = false; }
                }
                if all {
                    let subj = r.alert.subject.as_deref().map(|s| b[s].to_string())
                        .or_else(|| outer.iter().find(|e| matches!(e.kind, NounKind::Zone)).map(|e| b[e.var.as_str()].to_string()))
                        .unwrap_or_else(|| "scene".into());
                    let k = (r.id.clone(), subj);
                    proof.insert(0, format!("rule {}: {} [{}]", r.id, r.text, bkey));
                    proofs.entry(k.clone()).or_insert(proof);
                    derived.insert(k);
                }
            }
        }
        let g = self.timer.grace; self.timer.grace = timer_grace;
        for k in self.timer.keys() { if !seen.contains(&k) { self.timer.observe(&k, false, t); } }
        self.timer.grace = g;
        drop(w);
        self.recent = recent;
        self.fact_seen = fact_seen;
        self.remember(f);
        let (raised, cleared) = self.alerts.step(&derived, t);
        let rule = |id: &str| self.rules.iter().find(|r| r.id == id).unwrap();
        let mut out = vec![];
        for (k, kind) in raised.into_iter().map(|k| (k, "raised")).chain(cleared.into_iter().map(|k| (k, "cleared"))) {
            let r = rule(&k.0);
            out.push(AlertEvent { kind: kind.into(), rule: k.0.clone(), alert: r.alert.noun.clone(), lsr: r.alert.lsr, severity: r.alert.severity.clone(),
                subject: k.1.clone(), t, proof: if kind == "raised" { proofs.remove(&k).unwrap_or_default() } else { vec![] } });
        }
        out
    }
    pub fn active(&self) -> BTreeSet<(String, String)> { self.alerts.active() }
}
