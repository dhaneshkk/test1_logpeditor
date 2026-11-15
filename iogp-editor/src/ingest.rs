//! Raw detection logs → runtime frames, for replaying recorded site footage against rules.
//!
//! Input 1, the **site file** (JSON):
//! ```json
//! { "cameras": [ { "name": "CAM-03", "px_per_metre": 40,
//!                  "zones": [ { "id": "Rig-Floor-East", "type": "height-zone", "polygon": [[x,y], ...] } ] } ],
//!   "class_map": { "hardhat": "hardhat", "cell phone": "phone" },
//!   "signals":  [ { "subject": "Moonpool", "signal": "permit", "id": "PTW-1182", "from": 0, "to": 3600 } ] }
//! ```
//! Input 2, the **detection log** (JSON lines, one frame of one camera per line):
//! ```json
//! { "t": 12.4, "camera": "CAM-03",
//!   "detections": [ { "track_id": 7, "class": "person", "bbox": [x1, y1, x2, y2], "score": 0.91 } ],
//!   "facts": [["wear", "7", "12"]] }
//! ```
//! `t` is seconds (or `ts` as RFC 3339). Facts the detector already computes (e.g. PPE association)
//! may be passed through in `facts` (track ids, resolved per camera); otherwise they are derived
//! from geometry, by the catalog's detector names:
//!
//! | detector | rule | geometry |
//! |---|---|---|
//! | `inside_polygon` | is in ZONE | bottom-centre (foot point) inside the zone polygon |
//! | `below_footprint` | is under X | foot point within X's horizontal extent (±10 %) and X above |
//! | `within_distance` | is near X | foot points closer than the catalog distance; scale from `px_per_metre` or person height (1.7 m) |
//! | `standing_on` | is on X | foot point inside the lower 60 % of X |
//! | `ppe_association` | wears X | ≥ 60 % of X's box inside the person's box |
//! | `carry/hold/use_association` | carries/holds/uses X | X's centre inside the person's box widened by 15 % |
//! | `driver_association` | drives X | the person's centre inside X's box |

use crate::catalog::Catalog;
use crate::runtime::{Frame, Object};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize)]
pub struct Site {
    #[serde(default)] pub cameras: Vec<SiteCamera>,
    #[serde(default)] pub class_map: BTreeMap<String, String>,
    #[serde(default)] pub signals: Vec<SignalSpan>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct SiteCamera { pub name: String, #[serde(default)] pub px_per_metre: Option<f64>, #[serde(default)] pub zones: Vec<SiteZone> }
#[derive(Clone, Debug, Deserialize)]
pub struct SiteZone { pub id: String, #[serde(rename = "type")] pub zone: String, pub polygon: Vec<[f64; 2]> }
#[derive(Clone, Debug, Deserialize)]
pub struct SignalSpan { pub subject: String, pub signal: String, pub id: String, pub from: f64, pub to: f64 }

#[derive(Clone, Debug, Deserialize)]
pub struct RawFrame {
    #[serde(default)] pub t: Option<f64>,
    #[serde(default)] pub ts: Option<String>,
    pub camera: String,
    #[serde(default)] pub detections: Vec<RawDet>,
    #[serde(default)] pub facts: Vec<(String, String, String)>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct RawDet {
    #[serde(alias = "id", alias = "track")] pub track_id: serde_json::Value,
    #[serde(alias = "label")] pub class: String,
    pub bbox: [f64; 4],
    #[serde(default)] pub score: Option<f64>,
}

type BBox = [f64; 4];
fn foot(b: &BBox) -> (f64, f64) { ((b[0] + b[2]) / 2.0, b[3]) }
fn centre(b: &BBox) -> (f64, f64) { ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0) }
fn inside(b: &BBox, p: (f64, f64)) -> bool { p.0 >= b[0] && p.0 <= b[2] && p.1 >= b[1] && p.1 <= b[3] }
fn area(b: &BBox) -> f64 { ((b[2] - b[0]).max(0.0)) * ((b[3] - b[1]).max(0.0)) }
fn inter(a: &BBox, b: &BBox) -> f64 { area(&[a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]) }

/// Ray-casting point-in-polygon.
pub fn in_polygon(poly: &[[f64; 2]], p: (f64, f64)) -> bool {
    let mut c = false;
    let n = poly.len();
    if n < 3 { return false; }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi, xj, yj) = (poly[i][0], poly[i][1], poly[j][0], poly[j][1]);
        if (yi > p.1) != (yj > p.1) && p.0 < (xj - xi) * (p.1 - yi) / (yj - yi) + xi { c = !c; }
        j = i;
    }
    c
}

/// Parse an RFC 3339 UTC timestamp ("2026-11-14T08:30:12.5Z") to seconds since the epoch.
pub fn parse_ts(s: &str) -> Option<f64> {
    let re = regex::Regex::new(r"^(\d{4})-(\d{2})-(\d{2})[T ](\d{2}):(\d{2}):(\d{2}(?:\.\d+)?)(Z|[+-]\d{2}:\d{2})?$").unwrap();
    let c = re.captures(s.trim())?;
    let (y, m, d): (i64, i64, i64) = (c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?);
    let (hh, mm, ss): (i64, i64, f64) = (c[4].parse().ok()?, c[5].parse().ok()?, c[6].parse().ok()?);
    let (y2, m2) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let mut off = 0i64;
    if let Some(z) = c.get(7).map(|x| x.as_str()).filter(|z| *z != "Z") {
        let sign = if z.starts_with('-') { -1 } else { 1 };
        off = sign * (z[1..3].parse::<i64>().ok()? * 3600 + z[4..6].parse::<i64>().ok()? * 60);
    }
    Some((days * 86400 + hh * 3600 + mm * 60 - off) as f64 + ss)
}

pub struct Ingest<'a> { pub cat: &'a Catalog, pub site: &'a Site, pub warnings: Vec<String> }

impl<'a> Ingest<'a> {
    pub fn new(cat: &'a Catalog, site: &'a Site) -> Self { Ingest { cat, site, warnings: vec![] } }

    /// detector label used by the catalog for a raw class name (class_map first, then identity)
    fn label(&self, raw: &str) -> Option<String> {
        let l = self.site.class_map.get(raw).cloned().unwrap_or_else(|| raw.to_string());
        self.cat.classes.iter().any(|c| c.detector == l).then_some(l)
    }
    fn is_human(&self, label: &str) -> bool { self.cat.classes.iter().any(|c| c.detector == label && c.human) }

    /// One raw camera frame → runtime frame (object ids are `CAMERA/track`, zone ids as in the site file).
    pub fn frame(&mut self, rf: &RawFrame, t0: f64) -> Frame {
        let t = rf.t.or_else(|| rf.ts.as_deref().and_then(parse_ts)).unwrap_or(0.0) - t0;
        let t = (t * 1000.0).round() / 1000.0;
        let cam = self.site.cameras.iter().find(|c| c.name == rf.camera);
        if cam.is_none() && !self.warnings.iter().any(|w| w.contains(&rf.camera)) { self.warnings.push(format!("camera {} is not in the site file (no zones)", rf.camera)); }
        let mut objects = vec![];
        let mut facts: Vec<(String, String, String)> = vec![];
        let tid = |v: &serde_json::Value| match v { serde_json::Value::String(s) => s.clone(), x => x.to_string() };
        let dets: Vec<(String, String, BBox)> = rf.detections.iter().filter_map(|d| {
            match self.label(&d.class) {
                Some(l) => Some((format!("{}/{}", rf.camera, tid(&d.track_id)), l, d.bbox)),
                None => { if !self.warnings.iter().any(|w| w.contains(&format!("'{}'", d.class))) { self.warnings.push(format!("class '{}' is not in the catalog (map it in class_map)", d.class)); } None }
            }
        }).collect();
        for (id, l, _) in &dets { objects.push(Object { id: id.clone(), class: Some(l.clone()), ..Default::default() }); }
        if let Some(c) = cam {
            for z in &c.zones { objects.push(Object { id: z.id.clone(), zone: Some(z.zone.clone()), camera: Some(c.name.clone()), ..Default::default() }); }
        }
        let ppm = |b: &BBox, human: bool| -> f64 {
            cam.and_then(|c| c.px_per_metre).unwrap_or_else(|| if human { (b[3] - b[1]) / 1.7 } else { 40.0 })
        };
        // spatial facts
        for sp in &self.cat.spatial {
            for (ia, la, ba) in &dets {
                match sp.detector.as_str() {
                    "inside_polygon" => if let Some(c) = cam {
                        for z in &c.zones { if in_polygon(&z.polygon, foot(ba)) { facts.push((sp.prep.clone(), ia.clone(), z.id.clone())); } }
                    },
                    det => for (ib, _lb, bb) in &dets {
                        if ia == ib { continue; }
                        let hit = match det {
                            "below_footprint" => { let w = (bb[2] - bb[0]) * 0.1; let (fx, _) = foot(ba); fx >= bb[0] - w && fx <= bb[2] + w && bb[3] < ba[3] && bb[3] <= ba[1] + (ba[3] - ba[1]) * 0.5 }
                            "within_distance" => {
                                let m = sp.default_metres.unwrap_or(3.0);
                                let (a, b) = (foot(ba), foot(bb));
                                ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt() <= m * ppm(ba, self.is_human(la))
                            }
                            "standing_on" => { let lower = [bb[0], bb[1] + (bb[3] - bb[1]) * 0.4, bb[2], bb[3]]; inside(&lower, foot(ba)) }
                            _ => false,
                        };
                        if hit { facts.push((sp.prep.clone(), ia.clone(), ib.clone())); }
                    },
                }
            }
        }
        // association facts (subject = person-like)
        for rel in self.cat.relations.iter().filter(|r| r.detector != "signal") {
            for (ia, la, ba) in dets.iter().filter(|d| self.is_human(&d.1)) {
                let _ = la;
                for (ib, _lb, bb) in &dets {
                    if ia == ib { continue; }
                    let hit = match rel.detector.as_str() {
                        "ppe_association" => area(bb) > 0.0 && inter(ba, bb) / area(bb) >= 0.6,
                        "carry_association" | "hold_association" | "use_association" => { let w = (ba[2] - ba[0]) * 0.15; inside(&[ba[0] - w, ba[1], ba[2] + w, ba[3]], centre(bb)) }
                        "driver_association" => inside(bb, centre(ba)),
                        _ => false,
                    };
                    if hit { facts.push((rel.verb.clone(), ia.clone(), ib.clone())); }
                }
            }
        }
        // passthrough facts (track ids of this camera)
        for (r, a, b) in &rf.facts {
            let res = |x: &str| if cam.is_some_and(|c| c.zones.iter().any(|z| z.id == x)) { x.to_string() } else { format!("{}/{x}", rf.camera) };
            facts.push((r.clone(), res(a), res(b)));
        }
        // signals in force
        for s in self.site.signals.iter().filter(|s| t + t0 >= s.from && t + t0 <= s.to) {
            let subject = if cam.is_some_and(|c| c.zones.iter().any(|z| z.id == s.subject)) || self.site.cameras.iter().any(|c| c.zones.iter().any(|z| z.id == s.subject)) { s.subject.clone() } else { format!("{}/{}", rf.camera, s.subject) };
            objects.push(Object { id: s.id.clone(), signal: Some(s.signal.clone()), ..Default::default() });
            facts.push(("have".into(), subject, s.id.clone()));
        }
        facts.sort(); facts.dedup();
        Frame { t, objects, facts, camera: Some(rf.camera.clone()) }
    }
}

/// A labelled alert interval (from safety officers reviewing footage).
#[derive(Clone, Debug, Deserialize)]
pub struct Label {
    /// rule id, or "LSR<n>" to match any rule for that Life-Saving Rule
    pub rule: String,
    pub start: f64,
    pub end: f64,
    #[serde(default)] pub camera: Option<String>,
    #[serde(default)] pub subject: Option<String>,
    #[serde(default)] pub note: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn geometry() {
        let sq = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert!(in_polygon(&sq, (5.0, 5.0)) && !in_polygon(&sq, (11.0, 5.0)) && !in_polygon(&sq, (5.0, -1.0)));
        let l = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [5.0, 3.0], [0.0, 10.0]]; // concave
        assert!(!in_polygon(&l, (5.0, 8.0)) && in_polygon(&l, (5.0, 1.0)));
        assert_eq!(parse_ts("1970-01-01T00:01:00Z"), Some(60.0));
        assert_eq!(parse_ts("2026-11-14T00:00:00.5+05:30"), Some(parse_ts("2026-11-13T18:30:00Z").unwrap() + 0.5));
    }
    #[test]
    fn derives_facts_from_boxes() {
        let cat = Catalog::builtin();
        let site: Site = serde_json::from_value(serde_json::json!({
            "cameras": [{"name": "CAM-03", "zones": [{"id": "Rig-Floor-East", "type": "height-zone", "polygon": [[0,300],[400,300],[400,480],[0,480]]}]}],
            "class_map": {"hard hat": "hardhat"},
            "signals": [{"subject": "Rig-Floor-East", "signal": "permit", "id": "PTW-1", "from": 0, "to": 10}]})).unwrap();
        let rf: RawFrame = serde_json::from_value(serde_json::json!({"t": 5, "camera": "CAM-03", "detections": [
            {"track_id": 1, "class": "person", "bbox": [100, 250, 160, 420]},
            {"track_id": 2, "class": "hard hat", "bbox": [115, 250, 145, 270]},
            {"track_id": 3, "class": "suspended_load", "bbox": [80, 100, 200, 180]},
            {"track_id": 9, "class": "unicorn", "bbox": [0, 0, 1, 1]}]})).unwrap();
        let mut ing = Ingest::new(&cat, &site);
        let f = ing.frame(&rf, 0.0);
        let has = |r: &str, a: &str, b: &str| f.facts.contains(&(r.into(), a.into(), b.into()));
        assert!(has("in", "CAM-03/1", "Rig-Floor-East"), "{:?}", f.facts);
        assert!(has("wear", "CAM-03/1", "CAM-03/2"));
        assert!(has("under", "CAM-03/1", "CAM-03/3"));
        assert!(has("have", "Rig-Floor-East", "PTW-1"));
        assert!(ing.warnings.iter().any(|w| w.contains("unicorn")));
    }
}
