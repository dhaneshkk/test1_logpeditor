//! Synthetic sample of a recorded site, in the raw format `iogp_eval replay` reads (Rust port of the
//! former `tools/sample_vg360_log.py`, byte-identical output for the same seed):
//! `site.json` (cameras, zone polygons, permit timeline), `detections.jsonl` (tracked boxes per camera
//! frame, with realistic detection drop-outs), `labels.jsonl` (the alert intervals a safety officer
//! would mark while reviewing the footage) and `rules.ace` (the rules under test).
//!
//! It is a format example and a pipeline check, not evidence: replace it with a real VisionGuard360
//! export (same field names) to measure the rules on your site.
//!
//! ```text
//! iogp_eval sample-site OUTDIR [--seed 7]
//! ```

use crate::yappu_ref::PyRandom;
use std::path::Path;

/// A JSON value with insertion-ordered keys, serialised exactly like Python's `json.dumps`.
#[derive(Clone, Debug)]
pub enum PyJson { Int(i64), Float(f64), Str(String), Bool(bool), List(Vec<PyJson>), Obj(Vec<(String, PyJson)>) }

fn q(s: &str) -> String { serde_json::to_string(s).unwrap() }

impl PyJson {
    /// `json.dumps(v)` (separators ", " and ": ").
    pub fn dumps(&self) -> String {
        match self {
            PyJson::Int(i) => i.to_string(),
            PyJson::Float(f) => format!("{f:?}"),
            PyJson::Str(s) => q(s),
            PyJson::Bool(b) => if *b { "true".into() } else { "false".into() },
            PyJson::List(v) => format!("[{}]", v.iter().map(|x| x.dumps()).collect::<Vec<_>>().join(", ")),
            PyJson::Obj(v) => format!("{{{}}}", v.iter().map(|(k, x)| format!("{}: {}", q(k), x.dumps())).collect::<Vec<_>>().join(", ")),
        }
    }
    /// `json.dump(v, f, indent=n)`.
    pub fn dumps_indent(&self, n: usize, level: usize) -> String {
        let pad = |l: usize| " ".repeat(n * l);
        match self {
            PyJson::List(v) if !v.is_empty() => format!("[\n{}\n{}]", v.iter().map(|x| format!("{}{}", pad(level + 1), x.dumps_indent(n, level + 1))).collect::<Vec<_>>().join(",\n"), pad(level)),
            PyJson::Obj(v) if !v.is_empty() => format!("{{\n{}\n{}}}", v.iter().map(|(k, x)| format!("{}{}: {}", pad(level + 1), q(k), x.dumps_indent(n, level + 1))).collect::<Vec<_>>().join(",\n"), pad(level)),
            x => x.dumps(),
        }
    }
}

fn s(x: &str) -> PyJson { PyJson::Str(x.into()) }
fn i(x: i64) -> PyJson { PyJson::Int(x) }
fn obj(v: Vec<(&str, PyJson)>) -> PyJson { PyJson::Obj(v.into_iter().map(|(k, x)| (k.to_string(), x)).collect()) }
fn pts(v: &[[i64; 2]]) -> PyJson { PyJson::List(v.iter().map(|p| PyJson::List(vec![i(p[0]), i(p[1])])).collect()) }

/// A coordinate that is an int or a float, as Python would have it.
#[derive(Clone, Copy)]
enum N { I(i64), F(f64) }
impl N {
    fn add(self, k: i64) -> N { match self { N::I(a) => N::I(a + k), N::F(a) => N::F(a + k as f64) } }
    fn j(self) -> PyJson { match self { N::I(a) => i(a), N::F(a) => PyJson::Float(a) } }
}
fn bbox(b: [N; 4]) -> PyJson { PyJson::List(b.iter().map(|x| x.j()).collect()) }

pub const RULES: &str = "WAH3: If a worker is in a height-zone of CAM-03 and does not wear a harness for at least 3 seconds then VisionGuard raises a critical working-at-height-alert for the worker.
LOF: If a worker is under a suspended-load for at least 2 seconds then VisionGuard raises a critical line-of-fire-alert for the worker.
MOON: If a worker is in Moonpool for at least 5 seconds then VisionGuard raises a high work-authorisation-alert for the worker unless Moonpool has a permit.
";

pub struct Sample { pub site: String, pub detections: String, pub labels: String, pub rules: String, pub frames: usize, pub incidents: usize }

/// Build the four files' contents.
pub fn generate(seed: u64) -> Sample {
    const FPS: usize = 5;
    const SECS: usize = 240;
    let mut rnd = PyRandom::new(seed);
    let site = obj(vec![
        ("cameras", PyJson::List(vec![
            obj(vec![("name", s("CAM-03")), ("zones", PyJson::List(vec![obj(vec![("id", s("Rig-Floor-East")), ("type", s("height-zone")), ("polygon", pts(&[[100, 400], [900, 400], [900, 700], [100, 700]]))])]))]),
            obj(vec![("name", s("CAM-02")), ("zones", PyJson::List(vec![obj(vec![("id", s("Moonpool")), ("type", s("restricted-area")), ("polygon", pts(&[[500, 380], [1100, 380], [1100, 700], [500, 700]]))])]))]),
        ])),
        ("class_map", obj(vec![("hard hat", s("hardhat")), ("safety harness", s("harness")), ("cell phone", s("phone")), ("load", s("suspended_load"))])),
        ("signals", PyJson::List(vec![obj(vec![("subject", s("Moonpool")), ("signal", s("permit")), ("id", s("PTW-77")), ("from", i(0)), ("to", i(100))])])),
    ]);
    let person_box = |x: N, inside: bool| { let y2 = if inside { 600 } else { 330 }; [x, N::I(y2 - 170), x.add(60), N::I(y2)] };
    let det = |tid: i64, class: &str, b: PyJson, score: f64| obj(vec![("track_id", i(tid)), ("class", s(class)), ("bbox", b), ("score", PyJson::Float(score))]);
    let mut lines = vec![];
    for k in 0..SECS * FPS {
        let t = k as f64 / FPS as f64; // round(i / FPS, 2) leaves these values unchanged
        // CAM-03: rig floor. w1 takes the harness off 60-90 s while on the floor; w2's harness is
        // occluded for 1 s at 120 s (shorter than the 3 s rule: must NOT alert); a load swings over w2 150-156 s.
        let mut dets = vec![];
        let workers = [
            (1, N::F(200.0 + 3.0 * (t % 40.0)), (20.0..=200.0).contains(&t), !(60.0..90.0).contains(&t)),
            (2, N::I(600), (10.0..=230.0).contains(&t), !(120.0..121.0).contains(&t)),
        ];
        for (tid, x, inside, harness) in workers {
            if inside || t < 230.0 {
                let b = person_box(x, inside);
                if !(rnd.random() < 0.03) { dets.push(det(tid, "person", bbox(b), 0.9)); }
                // harness boxes are small and often missed
                if harness && !(rnd.random() < 0.20) {
                    dets.push(det(100 + tid, "safety harness", bbox([b[0].add(10), b[1].add(40), b[0].add(50), b[1].add(100)]), 0.7));
                }
            }
        }
        if (150.0..156.0).contains(&t) && !(rnd.random() < 0.1) {
            dets.push(det(50, "load", bbox([N::I(560), N::I(200), N::I(720), N::I(300)]), 0.8));
        }
        lines.push(obj(vec![("t", PyJson::Float(t)), ("camera", s("CAM-03")), ("detections", PyJson::List(dets))]).dumps());
        // CAM-02: moonpool. w3 is inside 30-50 s (permit valid until 100 s: fine) and 120-150 s (no permit).
        let mut dets = vec![];
        let inside = (30.0..50.0).contains(&t) || (120.0..150.0).contains(&t);
        let x = if inside { 700 } else { 200 };
        let y2 = if inside { 600 } else { 330 };
        if !(rnd.random() < 0.05) { dets.push(det(3, "person", bbox([N::I(x), N::I(y2 - 170), N::I(x + 60), N::I(y2)]), 0.88)); }
        lines.push(obj(vec![("t", PyJson::Float(t)), ("camera", s("CAM-02")), ("detections", PyJson::List(dets))]).dumps());
    }
    let labels = [
        ("WAH3", "CAM-03", 60, 90, "1", "w1 without harness on the rig floor"),
        ("LOF", "CAM-03", 150, 156, "2", "w2 under the swinging load"),
        ("MOON", "CAM-02", 120, 150, "3", "w3 in the moonpool after the permit expired"),
    ];
    let labels: Vec<String> = labels.iter().map(|(r, c, a, b, sub, note)| obj(vec![("rule", s(r)), ("camera", s(c)), ("start", i(*a)), ("end", i(*b)), ("subject", s(sub)), ("note", s(note))]).dumps()).collect();
    Sample {
        site: site.dumps_indent(1, 0),
        frames: lines.len(),
        detections: lines.join("\n") + "\n",
        incidents: labels.len(),
        labels: labels.join("\n") + "\n",
        rules: RULES.to_string(),
    }
}

/// Write site.json, detections.jsonl, labels.jsonl and rules.ace into `out`.
pub fn write(out: &Path, seed: u64) -> std::io::Result<Sample> {
    std::fs::create_dir_all(out)?;
    let smp = generate(seed);
    std::fs::write(out.join("site.json"), &smp.site)?;
    std::fs::write(out.join("detections.jsonl"), &smp.detections)?;
    std::fs::write(out.join("labels.jsonl"), &smp.labels)?;
    std::fs::write(out.join("rules.ace"), &smp.rules)?;
    Ok(smp)
}
