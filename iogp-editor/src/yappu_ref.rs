//! Rust port of the yappu-fsm parity reference (formerly `tools/yappu_parity.py`).
//!
//! It reproduces, without Python:
//! * **yappu-fsm's vision event engine** (`yappu_fsm/vision/events.py`): its controlled-English
//!   rule language (the same sentence patterns), the gap-tolerant duration `Timer`, the
//!   `AlertMachine` (IDLE → ACTIVE → COOLING → IDLE), and per-frame fact extraction from tracks
//!   (person box centre in a zone, held objects by intersection-over-area ≥ 0.5);
//! * **the seeded scenario** of the old script, bit for bit: Python's `random.Random(seed)`
//!   (MT19937 with Python's integer seeding, `random()`, `uniform()`, `choice()`).
//!
//! `iogp_eval yappu-ref OUT.jsonl` writes the frames (facts in iogp's runtime format plus the
//! alerts yappu's engine holds active); `iogp_eval parity OUT.jsonl` replays them through iogp's
//! engine in yappu-compatible mode. The port was checked line by line against the output of the
//! real Python yappu-fsm (`tests/yappu_ref.rs`).

use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashMap};

// ------------------------------------------------------------------ Python's random.Random

/// MT19937 seeded and sampled exactly like CPython's `random.Random`.
pub struct PyRandom { mt: [u32; 624], idx: usize }

impl PyRandom {
    /// `random.Random(seed)` for a non-negative integer seed.
    pub fn new(seed: u64) -> Self {
        // CPython: the absolute value split into 32-bit words, least significant first
        let mut key = vec![];
        let mut s = seed;
        loop { key.push((s & 0xffff_ffff) as u32); s >>= 32; if s == 0 { break; } }
        let mut r = PyRandom { mt: [0; 624], idx: 624 };
        r.init_genrand(19650218);
        let (mut i, mut j) = (1usize, 0usize);
        let n = 624usize;
        let mut k = n.max(key.len());
        while k > 0 {
            r.mt[i] = (r.mt[i] ^ ((r.mt[i - 1] ^ (r.mt[i - 1] >> 30)).wrapping_mul(1664525))).wrapping_add(key[j]).wrapping_add(j as u32);
            i += 1; j += 1;
            if i >= n { r.mt[0] = r.mt[n - 1]; i = 1; }
            if j >= key.len() { j = 0; }
            k -= 1;
        }
        k = n - 1;
        while k > 0 {
            r.mt[i] = (r.mt[i] ^ ((r.mt[i - 1] ^ (r.mt[i - 1] >> 30)).wrapping_mul(1566083941))).wrapping_sub(i as u32);
            i += 1;
            if i >= n { r.mt[0] = r.mt[n - 1]; i = 1; }
            k -= 1;
        }
        r.mt[0] = 0x8000_0000;
        r
    }
    fn init_genrand(&mut self, s: u32) {
        self.mt[0] = s;
        for i in 1..624 { self.mt[i] = 1812433253u32.wrapping_mul(self.mt[i - 1] ^ (self.mt[i - 1] >> 30)).wrapping_add(i as u32); }
        self.idx = 624;
    }
    fn genrand_u32(&mut self) -> u32 {
        const UPPER: u32 = 0x8000_0000;
        const LOWER: u32 = 0x7fff_ffff;
        const MATRIX: u32 = 0x9908_b0df;
        if self.idx >= 624 {
            for kk in 0..624 {
                let y = (self.mt[kk] & UPPER) | (self.mt[(kk + 1) % 624] & LOWER);
                self.mt[kk] = self.mt[(kk + 397) % 624] ^ (y >> 1) ^ if y & 1 == 1 { MATRIX } else { 0 };
            }
            self.idx = 0;
        }
        let mut y = self.mt[self.idx];
        self.idx += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }
    /// `random.random()`: 53-bit float in [0, 1).
    pub fn random(&mut self) -> f64 {
        let a = (self.genrand_u32() >> 5) as f64;
        let b = (self.genrand_u32() >> 6) as f64;
        (a * 67108864.0 + b) * (1.0 / 9007199254740992.0)
    }
    /// `random.uniform(a, b)`.
    pub fn uniform(&mut self, a: f64, b: f64) -> f64 { a + (b - a) * self.random() }
    /// `getrandbits(k)` for 0 < k <= 32.
    fn getrandbits(&mut self, k: u32) -> u32 { self.genrand_u32() >> (32 - k) }
    /// Index chosen by `random.choice(seq)` for a sequence of length `n` (`_randbelow`).
    pub fn choice_index(&mut self, n: usize) -> usize {
        let k = usize::BITS - n.leading_zeros();
        loop { let r = self.getrandbits(k) as usize; if r < n { return r; } }
    }
}

// ------------------------------------------------------------------ yappu's rule language

/// One condition of a yappu vision rule.
#[derive(Clone, Debug, PartialEq)]
pub enum Cond {
    InZoneFor { var: String, zone: String, secs: f64 },
    InZone { var: String, zone: String },
    HoldsFor { var: String, obj: String, secs: f64 },
    Holds { var: String, obj: String },
    NotHolds { var: String, obj: String },
    VisibleFor { var: String, secs: f64 },
    Crowd { k: usize, zone: String },
}

#[derive(Clone, Debug)]
pub struct VisionRule { pub id: String, pub conds: Vec<Cond>, pub event: String, pub person: Option<String> }

/// COCO class names (yappu_fsm/vision/detector.py), for object names and ids.
pub const COCO: [&str; 80] = ["person", "bicycle", "car", "motorcycle", "airplane", "bus", "train", "truck", "boat", "traffic light",
    "fire hydrant", "stop sign", "parking meter", "bench", "bird", "cat", "dog", "horse", "sheep", "cow",
    "elephant", "bear", "zebra", "giraffe", "backpack", "umbrella", "handbag", "tie", "suitcase", "frisbee",
    "skis", "snowboard", "sports ball", "kite", "baseball bat", "baseball glove", "skateboard", "surfboard", "tennis racket", "bottle",
    "wine glass", "cup", "fork", "knife", "spoon", "bowl", "banana", "apple", "sandwich", "orange",
    "broccoli", "carrot", "hot dog", "pizza", "donut", "cake", "chair", "couch", "potted plant", "bed",
    "dining table", "toilet", "tv", "laptop", "mouse", "remote", "keyboard", "cell phone", "microwave", "oven",
    "toaster", "sink", "refrigerator", "book", "clock", "vase", "scissors", "teddy bear", "hair drier", "toothbrush"];

/// Parse yappu vision rules (`ID: If ... then ... .` per line), with yappu's own sentence patterns.
pub fn parse_rules(text: &str) -> Result<Vec<VisionRule>, String> {
    let num = r"(\d+(?:\.\d+)?)";
    let name = r"([a-z][a-z0-9]*(?:-[a-z0-9]+)*)";
    let pats: Vec<(&str, Regex)> = vec![
        ("in_zone_for", Regex::new(&format!(r"^a person ([A-Z]) is in the {name}-zone for at least {num} seconds?$")).unwrap()),
        ("in_zone", Regex::new(&format!(r"^a person ([A-Z]) is in the {name}-zone$")).unwrap()),
        ("holds_for", Regex::new(&format!(r"^the person ([A-Z]) holds an? {name} for at least {num} seconds?$")).unwrap()),
        ("holds", Regex::new(&format!(r"^the person ([A-Z]) holds an? {name}$")).unwrap()),
        ("not_holds", Regex::new(&format!(r"^the person ([A-Z]) does not hold an? {name}$")).unwrap()),
        ("visible_for", Regex::new(&format!(r"^a person ([A-Z]) is visible for at least {num} seconds?$")).unwrap()),
        ("crowd", Regex::new(&format!(r"^at least (\d+) persons are in the {name}-zone$")).unwrap()),
    ];
    let alert_person = Regex::new(&format!(r"^an? {name} alert is raised for the person ([A-Z])$")).unwrap();
    let alert_scene = Regex::new(&format!(r"^an? {name} alert is raised$")).unwrap();
    let ifthen = Regex::new(r"^If (.+) then (.+)\.$").unwrap();
    let idline = Regex::new(r"^([A-Za-z][\w.-]*):\s+(.*)$").unwrap();
    let and = Regex::new(r"\s+and\s+").unwrap();
    let object = |n: &str| -> Result<String, String> {
        if COCO.contains(&n.replace('-', " ").as_str()) { Ok(n.to_string()) } else { Err(format!("'{n}' is not a COCO class the detector knows")) }
    };
    let mut out = vec![];
    for (n, raw) in text.lines().map(|l| l.split('#').next().unwrap().trim()).filter(|l| !l.is_empty()).enumerate() {
        let (id, line) = match idline.captures(raw) { Some(c) => (c[1].to_string(), c[2].to_string()), None => (format!("v{}", n + 1), raw.to_string()) };
        let c = ifthen.captures(&line).ok_or(format!("{id}: expected 'If ... then ... .'"))?;
        let mut conds = vec![];
        let mut bound = BTreeSet::new();
        for part in and.split(c[1].trim()) {
            let part = part.trim();
            let (kind, g) = pats.iter().find_map(|(k, rx)| rx.captures(part).map(|g| (*k, g))).ok_or(format!("{id}: condition not in the vision rule language: '{part}'"))?;
            let secs = |i: usize| g[i].parse::<f64>().unwrap();
            conds.push(match kind {
                "in_zone_for" => { bound.insert(g[1].to_string()); Cond::InZoneFor { var: g[1].into(), zone: g[2].into(), secs: secs(3) } }
                "in_zone" => { bound.insert(g[1].to_string()); Cond::InZone { var: g[1].into(), zone: g[2].into() } }
                "holds_for" => { bound.insert(g[1].to_string()); Cond::HoldsFor { var: g[1].into(), obj: object(&g[2])?, secs: secs(3) } }
                "holds" => { bound.insert(g[1].to_string()); Cond::Holds { var: g[1].into(), obj: object(&g[2])? } }
                "not_holds" => Cond::NotHolds { var: g[1].into(), obj: object(&g[2])? },
                "visible_for" => { bound.insert(g[1].to_string()); Cond::VisibleFor { var: g[1].into(), secs: secs(2) } }
                _ => Cond::Crowd { k: g[1].parse().unwrap(), zone: g[2].into() },
            });
        }
        let con = c[2].trim();
        let (event, person) = if let Some(g) = alert_person.captures(con) {
            if !bound.contains(&g[2]) { return Err(format!("{id}: the person {} must be introduced by a positive condition", &g[2])); }
            (g[1].to_string(), Some(g[2].to_string()))
        } else if let Some(g) = alert_scene.captures(con) { (g[1].to_string(), None) } else {
            return Err(format!("{id}: consequence not in the vision rule language: '{con}'"));
        };
        out.push(VisionRule { id, conds, event, person });
    }
    Ok(out)
}

// ------------------------------------------------------------------ yappu's engine

/// Gap-tolerant duration timer (yappu `Timer`).
#[derive(Default)]
struct YTimer { grace: f64, start: HashMap<(String, String, String), f64>, last: HashMap<(String, String, String), f64>, order: Vec<(String, String, String)> }

impl YTimer {
    fn observe(&mut self, key: &(String, String, String), v: bool, t: f64) {
        if v {
            if !self.start.contains_key(key) { self.start.insert(key.clone(), t); self.order.push(key.clone()); }
            self.last.insert(key.clone(), t);
        } else if let Some(l) = self.last.get(key) {
            if t - l > self.grace { self.start.remove(key); self.last.remove(key); self.order.retain(|k| k != key); }
        }
    }
    fn duration(&self, key: &(String, String, String), t: f64) -> f64 {
        match (self.start.get(key), self.last.get(key)) { (Some(s), Some(l)) if t - l <= self.grace => l - s, _ => 0.0 }
    }
    fn active_keys(&self) -> Vec<(String, String, String)> { self.order.clone() }
}

/// A track as yappu receives it from the tracker.
#[derive(Clone, Debug)]
pub struct Track { pub tid: i64, pub cls: usize, pub bx: [f64; 4] }

/// A zone as fractions of the frame.
#[derive(Clone, Debug)]
pub struct Zone { pub name: String, pub bx: [f64; 4] }

/// Facts yappu derives for one frame (yappu tuples).
#[derive(Clone, Debug, PartialEq)]
pub enum Fact { Person(String), InZone(String, String), Holds(String, String), DurFor(String, String, String, f64), VisibleFor(String, f64), Crowd(String, usize) }

pub struct EventEngine {
    rules: Vec<VisionRule>,
    zones: Vec<Zone>,
    durations: Vec<(String, String, f64)>,
    crowds: BTreeSet<(String, usize)>,
    timer: YTimer,
    state: BTreeMap<(String, String), (bool, f64)>, // (subject, event) -> (active?, since)
    cooldown: f64,
    hold_ioa: f64,
    pub last_facts: Vec<Fact>,
}

impl EventEngine {
    pub fn new(rules: Vec<VisionRule>, zones: Vec<Zone>, grace: f64, cooldown: f64) -> Result<Self, String> {
        let mut durations = vec![];
        let mut crowds = BTreeSet::new();
        for r in &rules {
            for c in &r.conds {
                let d = match c {
                    Cond::InZoneFor { zone, secs, .. } => Some(("in_zone".to_string(), zone.clone(), *secs)),
                    Cond::HoldsFor { obj, secs, .. } => Some(("holds".to_string(), obj.clone(), *secs)),
                    Cond::VisibleFor { secs, .. } => Some(("visible".to_string(), String::new(), *secs)),
                    Cond::Crowd { k, zone } => { crowds.insert((zone.clone(), *k)); None }
                    _ => None,
                };
                if let Some(d) = d { if !durations.contains(&d) { durations.push(d); } }
                let zone_ref = match c { Cond::InZoneFor { zone, .. } | Cond::Crowd { zone, .. } => Some(zone), _ => None };
                if let Some(z) = zone_ref { if !zones.iter().any(|x| &x.name == z) { return Err(format!("{}: no zone named '{z}'", r.id)); } }
            }
        }
        Ok(EventEngine { rules, zones, durations, crowds, timer: YTimer { grace, ..Default::default() }, state: BTreeMap::new(), cooldown, hold_ioa: 0.5, last_facts: vec![] })
    }

    fn ioa(inner: &[f64; 4], outer: &[f64; 4]) -> f64 {
        let (x1, y1) = (inner[0].max(outer[0]), inner[1].max(outer[1]));
        let (x2, y2) = (inner[2].min(outer[2]), inner[3].min(outer[3]));
        let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
        inter / ((inner[2] - inner[0]) * (inner[3] - inner[1])).max(1e-9)
    }

    fn facts_for(&mut self, tracks: &[Track], t: f64, w: f64, h: f64) -> Vec<Fact> {
        let persons: Vec<&Track> = tracks.iter().filter(|x| COCO[x.cls] == "person").collect();
        let objects: Vec<&Track> = tracks.iter().filter(|x| COCO[x.cls] != "person").collect();
        let mut facts = vec![];
        let mut seen = BTreeSet::new();
        for p in &persons {
            let pid = p.tid.to_string();
            let (cx, cy) = ((p.bx[0] + p.bx[2]) / 2.0, (p.bx[1] + p.bx[3]) / 2.0);
            facts.push(Fact::Person(pid.clone()));
            let k = ("visible".to_string(), pid.clone(), String::new());
            self.timer.observe(&k, true, t); seen.insert(k);
            for z in &self.zones {
                let (x, y) = (cx / w, cy / h);
                let inside = z.bx[0] <= x && x <= z.bx[2] && z.bx[1] <= y && y <= z.bx[3];
                if inside { facts.push(Fact::InZone(pid.clone(), z.name.clone())); }
                let k = ("in_zone".to_string(), pid.clone(), z.name.clone());
                self.timer.observe(&k, inside, t); seen.insert(k);
            }
            let mut held = BTreeSet::new();
            for o in &objects { if Self::ioa(&o.bx, &p.bx) >= self.hold_ioa { held.insert(COCO[o.cls].replace(' ', "-")); } }
            for n in &held { facts.push(Fact::Holds(pid.clone(), n.clone())); }
            for (kind, obj, _) in self.durations.clone() {
                if kind == "holds" { let k = ("holds".to_string(), pid.clone(), obj.clone()); self.timer.observe(&k, held.contains(&obj), t); seen.insert(k); }
            }
        }
        for k in self.timer.active_keys() { if !seen.contains(&k) { self.timer.observe(&k, false, t); } }
        for (kind, pid, arg) in self.timer.active_keys() {
            let d = self.timer.duration(&(kind.clone(), pid.clone(), arg.clone()), t);
            for (dk, darg, secs) in &self.durations {
                if *dk == kind && *darg == arg && d >= *secs {
                    facts.push(if kind == "visible" { Fact::VisibleFor(pid.clone(), *secs) } else { Fact::DurFor(kind.clone(), pid.clone(), arg.clone(), *secs) });
                }
            }
        }
        for (zone, k) in &self.crowds {
            let n = facts.iter().filter(|f| matches!(f, Fact::InZone(_, z) if z == zone)).count();
            if n >= *k { facts.push(Fact::Crowd(zone.clone(), *k)); }
        }
        facts
    }

    /// The rule bodies are Datalog conjunctions over yappu's facts, so a variable ranges over every
    /// subject of any fact — including a track missed this frame whose `*_for` fact still holds
    /// within the grace period (it has no `person` fact then).
    fn derive(&self, facts: &[Fact]) -> BTreeSet<(String, String)> {
        let mut persons: Vec<String> = vec![];
        for f in facts {
            let s = match f { Fact::Person(p) | Fact::InZone(p, _) | Fact::Holds(p, _) | Fact::DurFor(_, p, _, _) | Fact::VisibleFor(p, _) => p, Fact::Crowd(..) => continue };
            if !persons.contains(s) { persons.push(s.clone()); }
        }
        let mut out = BTreeSet::new();
        for r in &self.rules {
            let mut vars: Vec<String> = vec![];
            for c in &r.conds {
                let v = match c { Cond::InZoneFor { var, .. } | Cond::InZone { var, .. } | Cond::HoldsFor { var, .. } | Cond::Holds { var, .. } | Cond::NotHolds { var, .. } | Cond::VisibleFor { var, .. } => Some(var), Cond::Crowd { .. } => None };
                if let Some(v) = v { if !vars.contains(v) { vars.push(v.clone()); } }
            }
            let mut idx = vec![0usize; vars.len()];
            if !vars.is_empty() && persons.is_empty() { continue; }
            loop {
                let b: HashMap<&str, &str> = vars.iter().zip(&idx).map(|(v, &i)| (v.as_str(), persons[i].as_str())).collect();
                let ok = r.conds.iter().all(|c| match c {
                    Cond::InZoneFor { var, zone, secs } => facts.contains(&Fact::DurFor("in_zone".into(), b[var.as_str()].into(), zone.clone(), *secs)),
                    Cond::InZone { var, zone } => facts.contains(&Fact::InZone(b[var.as_str()].into(), zone.clone())),
                    Cond::HoldsFor { var, obj, secs } => facts.contains(&Fact::DurFor("holds".into(), b[var.as_str()].into(), obj.clone(), *secs)),
                    Cond::Holds { var, obj } => facts.contains(&Fact::Holds(b[var.as_str()].into(), obj.clone())),
                    Cond::NotHolds { var, obj } => !facts.contains(&Fact::Holds(b[var.as_str()].into(), obj.clone())),
                    Cond::VisibleFor { var, secs } => facts.contains(&Fact::VisibleFor(b[var.as_str()].into(), *secs)),
                    Cond::Crowd { k, zone } => facts.contains(&Fact::Crowd(zone.clone(), *k)),
                });
                if ok { out.insert((r.person.as_ref().map(|v| b[v.as_str()].to_string()).unwrap_or_else(|| "scene".into()), r.event.clone())); }
                let mut p = vars.len();
                let mut done = true;
                while p > 0 { p -= 1; idx[p] += 1; if idx[p] < persons.len() { done = false; break; } idx[p] = 0; }
                if done { break; }
            }
        }
        out
    }

    /// One frame: facts, derived alerts, alert machine.
    pub fn step(&mut self, tracks: &[Track], t: f64, w: f64, h: f64) {
        let facts = self.facts_for(tracks, t, w, h);
        let derived = self.derive(&facts);
        for k in &derived { self.state.insert(k.clone(), (true, t)); }
        let cd = self.cooldown;
        self.state.retain(|k, (active, since)| {
            if derived.contains(k) { return true; }
            if *active { *active = false; *since = t; true } else { t - *since < cd }
        });
        self.last_facts = facts;
    }

    /// Alerts whose condition holds now (yappu's `AlertMachine.active()`).
    pub fn active(&self) -> Vec<(String, String)> { self.state.iter().filter(|(_, (a, _))| *a).map(|(k, _)| k.clone()).collect() }
}

// ------------------------------------------------------------------ the parity scenario

/// yappu's versions of the three parity rules (yappu_fsm/data/vision_rules.ace style).
pub const YAPPU_RULES: &str = "
INTRUSION: If a person T is in the restricted-zone for at least 2 seconds then a zone-intrusion alert is raised for the person T.
PHONE: If the person T holds a cell-phone for at least 3 seconds then a phone-use alert is raised for the person T.
CROWD: If at least 2 persons are in the restricted-zone then a crowding alert is raised.
";

/// The seeded scenario of the old Python script: 5 people walking with dwells, phones picked up for
/// random spells, 25 % missed detections, 180 s at 10 fps on a 640×480 frame with the left 40 %
/// restricted. Returns one JSON line per frame: iogp runtime frame + `yappu_active`, formatted
/// byte for byte as the Python script wrote it.
pub fn parity_frames(seed: u64) -> Vec<String> {
    let (w, h, fps, secs, drop) = (640.0, 480.0, 10usize, 180usize, 0.25);
    let person = 0usize;
    let phone = COCO.iter().position(|c| *c == "cell phone").unwrap();
    let map = |e: &str| match e { "zone-intrusion" => "INTRUSION", "phone-use" => "PHONE", _ => "CROWD" };
    let mut rng = PyRandom::new(seed);
    let mut eng = EventEngine::new(parse_rules(YAPPU_RULES).unwrap(), vec![Zone { name: "restricted".into(), bx: [0.0, 0.0, 0.4, 1.0] }], 0.7, 3.0).unwrap();
    struct P { x: f64, vx: f64, phone_until: f64 }
    let mut people: Vec<(i64, P)> = (1..6).map(|pid| { let x = rng.uniform(0.05, 0.95); let vx = rng.uniform(-0.05, 0.05); (pid, P { x, vx, phone_until: -1.0 }) }).collect();
    let mut out = vec![];
    for i in 0..secs * fps {
        let t = i as f64 / fps as f64;
        let mut tracks = vec![];
        for (pid, p) in people.iter_mut() {
            if rng.random() < 0.02 {
                // Python evaluates the list [0.0, 0.0, uniform(...)] before choice()
                let u = rng.uniform(-0.06, 0.06);
                p.vx = [0.0, 0.0, u][rng.choice_index(3)];
            }
            p.x = (p.x + p.vx / fps as f64).max(0.03).min(0.97);
            if p.phone_until < t && rng.random() < 0.004 { p.phone_until = t + rng.uniform(1.0, 8.0); }
            if rng.random() < drop { continue; }
            let cx = p.x * w;
            tracks.push(Track { tid: *pid, cls: person, bx: [cx - 30.0, 150.0, cx + 30.0, 400.0] });
            if p.phone_until >= t && rng.random() >= drop { tracks.push(Track { tid: 100 + *pid, cls: phone, bx: [cx - 10.0, 250.0, cx + 10.0, 280.0] }); }
        }
        eng.step(&tracks, t, w, h);
        // written exactly as Python's json.dumps did (key order, ", " / ": " separators, float repr)
        let q = |x: &str| serde_json::to_string(x).unwrap();
        let mut objects = vec![r#"{"id": "restricted", "zone": "restricted-area"}"#.to_string()];
        let mut facts = vec![];
        for f in &eng.last_facts {
            match f {
                Fact::Person(p) => objects.push(format!(r#"{{"id": {}, "class": "person"}}"#, q(p))),
                Fact::InZone(p, z) => facts.push(format!("[\"in\", {}, {}]", q(p), q(z))),
                Fact::Holds(p, o) => {
                    let oid = format!("{o}@{p}");
                    objects.push(format!(r#"{{"id": {}, "class": {}}}"#, q(&oid), q(if o == "cell-phone" { "phone" } else { o })));
                    facts.push(format!("[\"hold\", {}, {}]", q(p), q(&oid)));
                }
                _ => {}
            }
        }
        let mut active: Vec<(String, String)> = eng.active().into_iter().map(|(s, e)| (map(&e).to_string(), if s == "scene" { "restricted".into() } else { s })).collect();
        active.sort();
        let active: Vec<String> = active.iter().map(|(a, b)| format!("[{}, {}]", q(a), q(b))).collect();
        out.push(format!(r#"{{"t": {t:?}, "objects": [{}], "facts": [{}], "yappu_active": [{}]}}"#, objects.join(", "), facts.join(", "), active.join(", ")));
    }
    out
}
