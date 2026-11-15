//! iogp-eval — MCC evaluation of compiled alert rules.
//!
//!   iogp-eval sample-site OUTDIR [--seed 7]
//!       write a synthetic sample site in the raw replay format (site, detections, labels, rules)
//!   iogp-eval yappu-ref FRAMES.jsonl [--seed 459]
//!       regenerate yappu-fsm's parity frames with the Rust port of its vision engine
//!   iogp-eval parity FRAMES.jsonl [--rules tools/yappu_parity_rules.ace]
//!       replay yappu-fsm's per-frame facts; compare active alerts with yappu's own engine
//!   iogp-eval synthetic [--seconds 600] [--miss 0.25] [--seed 459]
//!       all nine LSR templates on a seeded world with missed detections; engine vs per-frame baseline
//!   iogp-eval replay --site site.json --detections log.jsonl (--rules R.ace | --store rules.json) [--labels labels.jsonl]
//!       recorded detections (raw boxes per camera) -> alerts with proofs; with labelled alert
//!       intervals: frame MCC, incidents caught (recall), false alarms per hour, latency
//!   iogp-eval corpus attempts.jsonl [--store rules.json]
//!       what users wrote: acceptance rate, error types, unknown words (vocabulary gaps), intents
use clap::{Parser, Subcommand};
use iogp_editor::{catalog::Catalog, compile::Rule, runtime::{Engine, Frame, Object}, Compiler};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Parser)]
struct Cli {
    #[arg(long, env = "APE_PROLOG_DIR", default_value = "../ape-rs/prolog-scryer", global = true)]
    prolog_dir: PathBuf,
    #[arg(long, global = true)]
    report: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}
#[derive(Subcommand)]
enum Cmd {
    Parity { frames: PathBuf, #[arg(long, default_value = "tools/yappu_parity_rules.ace")] rules: PathBuf },
    Synthetic { #[arg(long, default_value_t = 600.0)] seconds: f64, #[arg(long, default_value_t = 0.25)] miss: f64, #[arg(long, default_value_t = 459)] seed: u64 },
    Replay {
        #[arg(long)] site: PathBuf,
        #[arg(long)] detections: PathBuf,
        /// rule sentences ("ID: sentence" per line)
        #[arg(long)] rules: Option<PathBuf>,
        /// rule store: replay the approved (live) versions
        #[arg(long)] store: Option<PathBuf>,
        /// labelled alert intervals (JSON lines: rule, start, end, camera?, subject?)
        #[arg(long)] labels: Option<PathBuf>,
        /// catalog JSON (default: built-in)
        #[arg(long)] catalog: Option<PathBuf>,
        /// seconds of slack when matching alerts to labelled incidents
        #[arg(long, default_value_t = 2.0)] tolerance: f64,
        /// also write the derived runtime frames (JSON lines) for inspection
        #[arg(long)] frames_out: Option<PathBuf>,
    },
    Corpus { attempts: PathBuf, #[arg(long)] store: Option<PathBuf> },
    /// regenerate the yappu-fsm parity frames (Rust port of yappu's vision engine; no Python)
    YappuRef { out: PathBuf, #[arg(long, default_value_t = 459)] seed: u64 },
    /// write a SYNTHETIC sample site (site.json, detections.jsonl, labels.jsonl, rules.ace) for `replay`
    SampleSite { out: PathBuf, #[arg(long, default_value_t = 7)] seed: u64 },
}

#[derive(Default, Clone, Copy)]
struct Conf { tp: u64, tn: u64, fp: u64, fn_: u64 }
impl Conf {
    fn add(&mut self, truth: bool, pred: bool) { match (truth, pred) { (true, true) => self.tp += 1, (false, false) => self.tn += 1, (false, true) => self.fp += 1, (true, false) => self.fn_ += 1 } }
    fn mcc(&self) -> f64 {
        let (tp, tn, fp, fn_) = (self.tp as f64, self.tn as f64, self.fp as f64, self.fn_ as f64);
        let d = ((tp + fp) * (tp + fn_) * (tn + fp) * (tn + fn_)).sqrt();
        if d == 0.0 { if fp == 0.0 && fn_ == 0.0 { 1.0 } else { 0.0 } } else { (tp * tn - fp * fn_) / d }
    }
    fn j(&self) -> serde_json::Value { json!({"mcc": (self.mcc() * 1e6).round() / 1e6, "tp": self.tp, "tn": self.tn, "fp": self.fp, "fn": self.fn_}) }
}

fn compile_file(c: &mut Compiler, text: &str) -> anyhow::Result<Vec<Rule>> {
    let re = regex::Regex::new(r"^([A-Za-z][\w.-]*):\s+(.*)$").unwrap();
    let mut out = vec![];
    for l in text.lines() {
        let l = l.split('#').next().unwrap().trim();
        if l.is_empty() { continue; }
        let cap = re.captures(l).ok_or_else(|| anyhow::anyhow!("expected 'ID: sentence': {l}"))?;
        out.push(c.compile(&cap[1], &cap[2]).map_err(|e| anyhow::anyhow!("{}: {}", &cap[1], e.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("; ")))?);
    }
    Ok(out)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; (self.0 >> 11) as f64 / (1u64 << 53) as f64 }
    fn chance(&mut self, p: f64) -> bool { self.next() < p }
    fn pick(&mut self, n: usize) -> usize { ((self.next() * n as f64) as usize).min(n - 1) }
}

/// A seeded world exercising every LSR template. Returns (truth frames, noisy detector frames).
fn world(seconds: f64, miss: f64, seed: u64) -> (Vec<Frame>, Vec<Frame>) {
    // the world and the detector noise use separate streams, so the ground truth is the same for every miss rate
    let mut r = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
    let mut rn = Rng(seed.wrapping_add(0x5851F42D4C957F2D).wrapping_mul(0xD1B54A32D192ED03) | 1);
    let zones = ["machine-guard-zone", "confined-space", "height-zone", "lifting-zone", "live-equipment-zone", "restricted-area", "hot-work-area"];
    let nw = 6;
    let fps = 10.0;
    let dt = 1.0 / fps;
    // per-worker state
    let mut zone: Vec<Option<usize>> = vec![None; nw];
    let mut harness = vec![true; nw];
    let mut gas = vec![true; nw];
    let mut phone = vec![false; nw];
    let mut driving = vec![false; nw];
    let mut under = vec![false; nw];
    let mut flame = false;
    let mut permit_hot = true;
    let mut permit_restricted = true;
    let mut iso_cert = true;
    let (mut truth, mut noisy) = (vec![], vec![]);
    let steps = (seconds * fps) as usize;
    for i in 0..steps {
        let t = (i as f64 * dt * 1000.0).round() / 1000.0;
        let p = |rate: f64| rate * dt; // per-second rate -> per-frame probability
        for w in 0..nw {
            if r.chance(p(0.06)) { zone[w] = if r.chance(0.3) { None } else if r.chance(0.35) { Some(3) /* lifting-zone: busy, so LSR7 crowds occur */ } else { Some(r.pick(zones.len())) }; }
            if r.chance(p(0.05)) { harness[w] = !harness[w]; }
            if r.chance(p(0.04)) { gas[w] = !gas[w]; }
            if r.chance(p(if phone[w] { 0.25 } else { 0.04 })) { phone[w] = !phone[w]; }
            if r.chance(p(if driving[w] { 0.05 } else { 0.02 })) { driving[w] = !driving[w]; }
            if r.chance(p(if under[w] { 0.3 } else { 0.03 })) { under[w] = !under[w]; }
        }
        if r.chance(p(if flame { 0.05 } else { 0.03 })) { flame = !flame; }
        if r.chance(p(0.02)) { permit_hot = !permit_hot; }
        if r.chance(p(0.015)) { permit_restricted = !permit_restricted; }
        if r.chance(p(0.02)) { iso_cert = !iso_cert; }

        let mut objs: Vec<Object> = zones.iter().map(|z| Object { id: format!("z:{z}"), zone: Some(z.to_string()), ..Default::default() }).collect();
        let mut facts: Vec<(String, String, String)> = vec![];
        let obj = |id: String, class: &str| Object { id, class: Some(class.into()), ..Default::default() };
        objs.push(obj("load1".into(), "suspended_load"));
        for w in 0..nw {
            let wid = format!("w{w}");
            objs.push(obj(wid.clone(), "person"));
            if let Some(z) = zone[w] { facts.push(("in".into(), wid.clone(), format!("z:{}", zones[z]))); }
            if harness[w] { let h = format!("harness{w}"); objs.push(obj(h.clone(), "harness")); facts.push(("wear".into(), wid.clone(), h)); }
            if gas[w] { let g = format!("gas{w}"); objs.push(obj(g.clone(), "gas_detector")); facts.push(("carry".into(), wid.clone(), g)); }
            if phone[w] { let g = format!("phone{w}"); objs.push(obj(g.clone(), "phone")); facts.push(("use".into(), wid.clone(), g)); }
            if driving[w] { let v = format!("veh{w}"); objs.push(obj(v.clone(), "vehicle")); facts.push(("drive".into(), wid.clone(), v)); }
            if under[w] { facts.push(("under".into(), wid.clone(), "load1".into())); }
        }
        if flame { objs.push(obj("flame1".into(), "flame")); facts.push(("in".into(), "flame1".into(), "z:hot-work-area".into())); }
        for (have, zid, sig, sid) in [(permit_hot, "z:hot-work-area", "permit", "ptw-hot"), (permit_restricted, "z:restricted-area", "permit", "ptw-rst"), (iso_cert, "z:live-equipment-zone", "isolation-certificate", "iso-1")] {
            if have { objs.push(Object { id: sid.into(), signal: Some(sig.into()), ..Default::default() }); facts.push(("have".into(), zid.into(), sid.into())); }
        }
        // detector output: each detected object is missed with probability `miss` (its facts go with it);
        // each association is additionally missed with probability miss/2; zones and signals are not visual
        let mut dropped = BTreeSet::new();
        let mut nobjs = vec![];
        for o in &objs {
            if o.class.is_some() && rn.chance(miss) { dropped.insert(o.id.clone()); } else { nobjs.push(o.clone()); }
        }
        let nfacts: Vec<_> = facts.iter().filter(|(rel, a, b)| !dropped.contains(a) && !dropped.contains(b) && (rel == "have" || rel == "in" && b.starts_with("z:") || !rn.chance(miss / 2.0))).cloned().collect();
        truth.push(Frame { t, objects: objs, facts, camera: None });
        noisy.push(Frame { t, objects: nobjs, facts: nfacts, camera: None });
    }
    (truth, noisy)
}

fn run(rules: &[Rule], frames: &[Frame], grace: f64, durations: bool, cooldown: f64) -> Vec<BTreeSet<(String, String)>> { run_mode(rules, frames, grace, durations, cooldown, true) }
fn run_mode(rules: &[Rule], frames: &[Frame], grace: f64, durations: bool, cooldown: f64, smoothing: bool) -> Vec<BTreeSet<(String, String)>> {
    let mut e = Engine::new(rules.to_vec(), grace, cooldown);
    e.use_durations = durations;
    e.smoothing = smoothing && durations;
    frames.iter().map(|f| { e.step(f); e.active() }).collect()
}

fn score(truth: &[BTreeSet<(String, String)>], pred: &[BTreeSet<(String, String)>]) -> (Conf, BTreeMap<String, Conf>) {
    let keys: BTreeSet<(String, String)> = truth.iter().chain(pred.iter()).flatten().cloned().collect();
    let (mut all, mut per) = (Conf::default(), BTreeMap::<String, Conf>::new());
    for (t, p) in truth.iter().zip(pred) {
        for k in &keys {
            let (a, b) = (t.contains(k), p.contains(k));
            all.add(a, b);
            per.entry(k.0.clone()).or_default().add(a, b);
        }
    }
    (all, per)
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cat = match &cli.cmd { Cmd::Replay { catalog: Some(p), .. } => Catalog::from_json(&std::fs::read_to_string(p)?)?, _ => Catalog::builtin() };
    if let Cmd::YappuRef { out, seed } = &cli.cmd {
        let lines = iogp_editor::yappu_ref::parity_frames(*seed);
        std::fs::write(out, lines.join("\n") + "\n")?;
        println!("yappu-ref: {} frames -> {}", lines.len(), out.display());
        return Ok(());
    }
    if let Cmd::SampleSite { out, seed } = &cli.cmd {
        let s = iogp_editor::sample_site::write(out, *seed)?;
        println!("{} camera frames, {} labelled incidents -> {}/", s.frames, s.incidents, out.display());
        return Ok(());
    }
    if let Cmd::Corpus { attempts, store } = &cli.cmd {
        let r = corpus(attempts, store.as_ref())?;
        if let Some(p) = cli.report { std::fs::write(p, serde_json::to_string_pretty(&r)?)?; }
        return Ok(());
    }
    let mut c = Compiler::new(cat.clone(), ape_rs::Ape::new(&cli.prolog_dir)?);
    let report = match cli.cmd {
        Cmd::Parity { frames, rules } => {
            let rules = compile_file(&mut c, &std::fs::read_to_string(rules)?)?;
            let mut fs = vec![];
            let mut yappu = vec![];
            for l in std::fs::read_to_string(&frames)?.lines() {
                let v: serde_json::Value = serde_json::from_str(l)?;
                yappu.push(v["yappu_active"].as_array().unwrap().iter().map(|p| (p[0].as_str().unwrap().to_string(), p[1].as_str().unwrap().to_string())).collect::<BTreeSet<_>>());
                fs.push(serde_json::from_value::<Frame>(v)?);
            }
            let ours = run_mode(&rules, &fs, cat.defaults.grace_seconds, true, cat.defaults.cooldown_seconds, false);
            let (all, per) = score(&yappu, &ours);
            let identical = yappu.iter().zip(&ours).filter(|(a, b)| a == b).count();
            for (i, (a, b)) in yappu.iter().zip(&ours).enumerate() {
                if a != b { println!("  differs at t={}: yappu-only {:?}  iogp-only {:?}", fs[i].t, a.difference(b).collect::<Vec<_>>(), b.difference(a).collect::<Vec<_>>()); }
            }
            println!("yappu-fsm parity: {} frames, identical active-alert sets in {identical}", fs.len());
            println!("  all rules   mcc={:.6}  tp={} tn={} fp={} fn={}", all.mcc(), all.tp, all.tn, all.fp, all.fn_);
            for (k, v) in &per { println!("  {k:<11} mcc={:.6}  tp={} fp={} fn={}", v.mcc(), v.tp, v.fp, v.fn_); }
            json!({"frames": fs.len(), "identical_frames": identical, "all": all.j(), "per_rule": per.iter().map(|(k, v)| (k.clone(), v.j())).collect::<BTreeMap<_, _>>()})
        }
        Cmd::Replay { site, detections, rules, store, labels, tolerance, frames_out, .. } => {
            replay(&mut c, &cat, &site, &detections, rules.as_ref(), store.as_ref(), labels.as_ref(), tolerance, frames_out.as_ref())?
        }
        Cmd::Corpus { .. } | Cmd::YappuRef { .. } | Cmd::SampleSite { .. } => unreachable!(),
        Cmd::Synthetic { seconds, miss, seed } => {
            let rules: Vec<Rule> = cat.templates.iter().map(|t| c.compile(&format!("LSR{}", t.lsr), &t.text).map_err(|e| anyhow::anyhow!("LSR{}: {:?}", t.lsr, e))).collect::<anyhow::Result<_>>()?;
            let (truth, noisy) = world(seconds, miss, seed);
            let gt = run(&rules, &truth, 0.0, true, 0.0);
            let eng = run(&rules, &noisy, cat.defaults.grace_seconds, true, 0.0);
            let base = run(&rules, &noisy, 0.0, false, 0.0);
            let yc = run_mode(&rules, &noisy, cat.defaults.grace_seconds, true, 0.0, false);
            let (ya, yp) = score(&gt, &yc);
            let (ea, ep) = score(&gt, &eng);
            let (ba, bp) = score(&gt, &base);
            println!("synthetic world: {seconds} s at 10 fps, {} rules, detection miss rate {miss}", rules.len());
            println!("  {:<6} {:>12} {:>14} {:>14}", "rule", "engine MCC", "yappu-timers", "per-frame MCC");
            for r in &rules {
                let k = &r.id;
                let (e, y, b) = (ep.get(k).copied().unwrap_or_default(), yp.get(k).copied().unwrap_or_default(), bp.get(k).copied().unwrap_or_default());
                let truth_pos = e.tp + e.fn_;
                if truth_pos == 0 && e.fp == 0 && y.fp == 0 && b.fp == 0 {
                    println!("  {k:<6} {:>12} {:>14} {:>14}   (no alert in the ground truth or any prediction)", "-", "-", "-");
                } else {
                    println!("  {k:<6} {:>12.4} {:>14.4} {:>14.4}{}", e.mcc(), y.mcc(), b.mcc(), if truth_pos == 0 { "   (no alert in the ground truth)" } else { "" });
                }
            }
            println!("  {:<6} {:>12.4} {:>14.4} {:>14.4}", "ALL", ea.mcc(), ya.mcc(), ba.mcc());
            json!({"seconds": seconds, "miss": miss, "seed": seed, "engine": ea.j(), "per_frame_baseline": ba.j(), "yappu_timer_semantics": ya.j(),
                   "per_rule": ep.iter().map(|(k, v)| (k.clone(), json!({"engine": v.j(), "per_frame": bp.get(k).map(|x| x.j())}))).collect::<BTreeMap<_, _>>()})
        }
    };
    if let Some(p) = cli.report { std::fs::write(p, serde_json::to_string_pretty(&report)?)?; }
    Ok(())
}


// ------------------------------------------------------------------------------------------- replay

fn label_matches(l: &iogp_editor::ingest::Label, r: &Rule, camera: &str) -> bool {
    let rule_ok = l.rule == r.id || l.rule.strip_prefix("LSR").and_then(|n| n.parse::<u8>().ok()) == Some(r.alert.lsr);
    rule_ok && l.camera.as_deref().map_or(true, |c| c == camera)
}

#[allow(clippy::too_many_arguments)]
fn replay(c: &mut Compiler, cat: &Catalog, site: &PathBuf, detections: &PathBuf, rules: Option<&PathBuf>, store: Option<&PathBuf>, labels: Option<&PathBuf>, tol: f64, frames_out: Option<&PathBuf>) -> anyhow::Result<serde_json::Value> {
    use iogp_editor::ingest::{Ingest, Label, RawFrame, Site};
    let site: Site = serde_json::from_str(&std::fs::read_to_string(site)?)?;
    let rules: Vec<Rule> = match (rules, store) {
        (Some(p), _) => compile_file(c, &std::fs::read_to_string(p)?)?,
        (None, Some(p)) => {
            let st = iogp_editor::store::Store::open(p)?;
            let mut v = vec![];
            for (r, ver) in st.active() { v.push(c.compile(&r.id, &ver.text).map_err(|e| anyhow::anyhow!("{} v{}: {}", r.id, ver.v, e[0].message))?); }
            v
        }
        _ => anyhow::bail!("give --rules or --store"),
    };
    anyhow::ensure!(!rules.is_empty(), "no rules to replay");
    let labels: Vec<Label> = match labels { Some(p) => std::fs::read_to_string(p)?.lines().filter(|l| !l.trim().is_empty()).map(serde_json::from_str).collect::<Result<_, _>>()?, None => vec![] };
    let raw: Vec<RawFrame> = std::fs::read_to_string(detections)?.lines().filter(|l| !l.trim().is_empty()).enumerate()
        .map(|(i, l)| serde_json::from_str(l).map_err(|e| anyhow::anyhow!("detections line {}: {e}", i + 1))).collect::<anyhow::Result<_>>()?;
    // absolute timestamps (ts) are rebased to the first frame
    let t0 = raw.iter().filter_map(|f| f.ts.as_deref().and_then(iogp_editor::ingest::parse_ts)).fold(f64::INFINITY, f64::min);
    let t0 = if t0.is_finite() { t0 } else { 0.0 };
    let mut ing = Ingest::new(cat, &site);
    let mut per_cam: BTreeMap<String, Vec<Frame>> = BTreeMap::new();
    for rf in &raw { per_cam.entry(rf.camera.clone()).or_default().push(ing.frame(rf, t0)); }
    for w in &ing.warnings { println!("  warning: {w}"); }
    if let Some(p) = frames_out {
        let mut out = String::new();
        for fs in per_cam.values() { for f in fs { out += &serde_json::to_string(f)?; out.push('\n'); } }
        std::fs::write(p, out)?;
    }
    let (mut events, mut all, mut per_rule) = (vec![], Conf::default(), BTreeMap::<String, Conf>::new());
    let mut hours = 0.0;
    for (cam, frames) in &per_cam {
        let mut fs = frames.clone();
        fs.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
        if fs.len() > 1 { hours += (fs.last().unwrap().t - fs[0].t) / 3600.0; }
        let mut e = Engine::new(rules.clone(), cat.defaults.grace_seconds, cat.defaults.cooldown_seconds);
        for f in &fs {
            for ev in e.step(f) { if ev.kind == "raised" { events.push((cam.clone(), ev)); } }
            if !labels.is_empty() {
                let active = e.active();
                for r in &rules {
                    let truth = labels.iter().any(|l| label_matches(l, r, cam) && f.t >= l.start && f.t <= l.end);
                    let pred = active.iter().any(|(rid, _)| rid == &r.id);
                    all.add(truth, pred);
                    per_rule.entry(r.id.clone()).or_default().add(truth, pred);
                }
            }
        }
    }
    events.sort_by(|a, b| a.1.t.partial_cmp(&b.1.t).unwrap());
    println!("replay: {} raw frames, {} cameras, {:.2} h of footage, {} rules, {} alerts raised", raw.len(), per_cam.len(), hours, rules.len(), events.len());
    for (cam, ev) in &events { println!("  t={:>7.1}  {cam:<8} {:<10} {:<14} {}", ev.t, ev.rule, ev.subject, ev.proof.get(1..).map(|p| p.join(" / ")).unwrap_or_default()); }
    let mut out = json!({"frames": raw.len(), "cameras": per_cam.keys().collect::<Vec<_>>(), "hours": hours, "warnings": ing.warnings,
        "alerts": events.iter().map(|(cam, e)| json!({"camera": cam, "t": e.t, "rule": e.rule, "lsr": e.lsr, "subject": e.subject, "proof": e.proof})).collect::<Vec<_>>()});
    if !labels.is_empty() {
        // incidents: a label is caught if a matching alert is raised within [start - tol, end + tol]
        let mut caught = 0; let mut latency = vec![];
        let mut missed = vec![];
        for l in &labels {
            let hit = events.iter().filter(|(cam, e)| rules.iter().find(|r| r.id == e.rule).is_some_and(|r| label_matches(l, r, cam)) && e.t >= l.start - tol && e.t <= l.end + tol).map(|(_, e)| e.t).fold(f64::INFINITY, f64::min);
            if hit.is_finite() { caught += 1; latency.push(hit - l.start); } else { missed.push(l); }
        }
        let false_alarms: Vec<&(String, iogp_editor::runtime::AlertEvent)> = events.iter().filter(|(cam, e)| {
            let r = rules.iter().find(|r| r.id == e.rule).unwrap();
            !labels.iter().any(|l| label_matches(l, r, cam) && e.t >= l.start - tol && e.t <= l.end + tol)
        }).collect();
        latency.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = if latency.is_empty() { f64::NAN } else { latency[latency.len() / 2] };
        println!("labelled incidents: {} | caught {caught} (recall {:.3}) | false alarms {} ({:.2}/h) | median latency {:.1} s",
            labels.len(), caught as f64 / labels.len() as f64, false_alarms.len(), false_alarms.len() as f64 / hours.max(1e-9), med);
        for l in &missed { println!("  missed: {} {:?} {:.1}-{:.1} {}", l.rule, l.camera, l.start, l.end, l.note); }
        for (cam, e) in &false_alarms { println!("  false alarm: t={:.1} {cam} {} {}", e.t, e.rule, e.subject); }
        println!("frame-level MCC (alert active vs labelled): all {:.4}", all.mcc());
        for (k, v) in &per_rule { println!("  {k:<12} mcc={:.4}  tp={} fp={} fn={}", v.mcc(), v.tp, v.fp, v.fn_); }
        out["labels"] = json!({"incidents": labels.len(), "caught": caught, "recall": caught as f64 / labels.len() as f64, "false_alarms": false_alarms.len(),
            "false_alarms_per_hour": false_alarms.len() as f64 / hours.max(1e-9), "median_latency_s": med, "frame_mcc": all.j(),
            "per_rule": per_rule.iter().map(|(k, v)| (k.clone(), v.j())).collect::<BTreeMap<_, _>>()});
    }
    Ok(out)
}

// ------------------------------------------------------------------------------------------- corpus

fn corpus(attempts: &PathBuf, store: Option<&PathBuf>) -> anyhow::Result<serde_json::Value> {
    let rows: Vec<serde_json::Value> = std::fs::read_to_string(attempts)?.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect();
    // one attempt per (user, text): editors re-compile the same sentence as words are added
    let mut seen = BTreeSet::new();
    let rows: Vec<&serde_json::Value> = rows.iter().filter(|r| seen.insert((r["user"].to_string(), r["text"].to_string()))).collect();
    let ok = rows.iter().filter(|r| r["ok"] == true).count();
    let mut users: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut unknown: BTreeMap<String, usize> = BTreeMap::new();
    let mut lsr: BTreeMap<String, usize> = BTreeMap::new();
    let word = regex::Regex::new(r"^'([^']+)' is not a word").unwrap();
    let norm = regex::Regex::new(r"'[^']*'|\d+").unwrap();
    for r in &rows {
        let u = users.entry(r["user"].as_str().unwrap_or("?").to_string()).or_default();
        u.0 += 1; if r["ok"] == true { u.1 += 1; }
        if let Some(n) = r["lsr"].as_u64() { *lsr.entry(format!("LSR{n}")).or_default() += 1; }
        for e in r["errors"].as_array().into_iter().flatten() {
            let m = e["message"].as_str().unwrap_or("");
            if let Some(c) = word.captures(m) { *unknown.entry(c[1].to_lowercase()).or_default() += 1; }
            *kinds.entry(norm.replace_all(m, "…").to_string()).or_default() += 1;
        }
    }
    let top = |m: &BTreeMap<String, usize>, n: usize| { let mut v: Vec<(&String, &usize)> = m.iter().collect(); v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0))); v.into_iter().take(n).map(|(k, c)| (k.clone(), *c)).collect::<Vec<_>>() };
    println!("corpus: {} distinct sentences from {} users, {} valid ({:.0} %)", rows.len(), users.len(), ok, 100.0 * ok as f64 / rows.len().max(1) as f64);
    for (u, (n, k)) in &users { println!("  {u:<16} {n:>4} sentences, {k:>4} valid"); }
    println!("most common problems:"); for (k, c) in top(&kinds, 10) { println!("  {c:>4}  {k}"); }
    println!("unknown words (vocabulary gaps — add to the catalog or teach the synonym):"); for (k, c) in top(&unknown, 20) { println!("  {c:>4}  {k}"); }
    println!("valid rules by Life-Saving Rule:"); for (k, c) in &lsr { println!("  {k:<6} {c}"); }
    let mut intents = vec![];
    if let Some(p) = store {
        let st = iogp_editor::store::Store::open(p)?;
        for r in &st.rules { for v in &r.versions { if !v.intent.is_empty() { intents.push(json!({"rule": r.id, "v": v.v, "intent": v.intent, "text": v.text, "status": v.status})); } } }
        println!("intents recorded with rule versions: {}", intents.len());
        for i in &intents { println!("  {} v{}: “{}” → {}", i["rule"].as_str().unwrap(), i["v"], i["intent"].as_str().unwrap(), i["text"].as_str().unwrap()); }
    }
    Ok(json!({"sentences": rows.len(), "valid": ok, "users": users.iter().map(|(u, (n, k))| (u.clone(), json!({"sentences": n, "valid": k}))).collect::<BTreeMap<_, _>>(),
        "problems": top(&kinds, 50), "unknown_words": top(&unknown, 100), "by_lsr": lsr, "intents": intents}))
}
