//! The ASP encoding must derive exactly what the reference engine derives, frame by frame, on
//! noisy multi-camera scenes, so the clingo-based review reasons about the real semantics.
use iogp_editor::asp;
use iogp_runtime::{load_config, reference, Frame, Object};

const CONFIG: &str = include_str!("../iogp-runtime/tests/fixtures/config.json");

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 { self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); self.0 >> 33 }
    fn chance(&mut self, p: f64) -> bool { (self.next() % 10_000) as f64 / 10_000.0 < p }
    fn pick(&mut self, n: usize) -> usize { (self.next() % n as u64) as usize }
}

/// A busy multi-camera site with persistent workers, PPE that comes and goes, missed detections,
/// permits that start and end, vehicles, loads and phones.
fn world(seed: u64, frames: usize, miss: f64) -> Vec<Frame> {
    let mut r = Lcg(seed);
    let zones = [("Rig-Floor-East", "height-zone", "CAM-03"), ("hz4", "height-zone", "CAM-04"), ("Moonpool", "restricted-area", "CAM-02"),
                 ("cs1", "confined-space", "CAM-01"), ("lz1", "lifting-zone", "CAM-02"), ("hw1", "hot-work-area", "CAM-04"),
                 ("le1", "live-equipment-zone", "CAM-01"), ("mg1", "machine-guard-zone", "CAM-03"), ("ra2", "restricted-area", "CAM-04")];
    let n = 7;
    let mut zone_of: Vec<Option<usize>> = vec![None; n];
    let mut harness = vec![true; n];
    let mut gas = vec![true; n];
    let mut phone = vec![false; n];
    let mut permit = [true, false, true];
    let mut out = vec![];
    for i in 0..frames {
        let t = (i as f64) / 10.0;
        for w in 0..n {
            if r.chance(0.02) { zone_of[w] = if r.chance(0.25) { None } else { Some(r.pick(zones.len())) }; }
            if r.chance(0.02) { harness[w] = !harness[w]; }
            if r.chance(0.01) { gas[w] = !gas[w]; }
            if r.chance(0.015) { phone[w] = !phone[w]; }
        }
        for p in permit.iter_mut() { if r.chance(0.01) { *p = !*p; } }
        let mut objects: Vec<Object> = zones.iter().map(|(id, ty, cam)| Object { id: id.to_string(), zone: Some(ty.to_string()), camera: Some(cam.to_string()), ..Default::default() }).collect();
        let mut facts = vec![];
        for w in 0..n {
            if r.chance(miss) { continue; } // the person is missed this frame
            let id = format!("w{w}");
            objects.push(Object { id: id.clone(), class: Some("person".into()), ..Default::default() });
            if let Some(z) = zone_of[w] { facts.push(("in".to_string(), id.clone(), zones[z].0.to_string())); }
            if harness[w] && !r.chance(miss) { objects.push(Object { id: format!("h{w}"), class: Some("harness".into()), ..Default::default() }); facts.push(("wear".into(), id.clone(), format!("h{w}"))); }
            if gas[w] && !r.chance(miss) { objects.push(Object { id: format!("g{w}"), class: Some("gas_detector".into()), ..Default::default() }); facts.push(("carry".into(), id.clone(), format!("g{w}"))); }
            if phone[w] && !r.chance(miss) { objects.push(Object { id: format!("ph{w}"), class: Some("phone".into()), ..Default::default() }); facts.push(("use".into(), id.clone(), format!("ph{w}"))); }
            if w == 0 && (i / 70) % 3 == 1 { objects.push(Object { id: "v1".into(), class: Some("vehicle".into()), ..Default::default() }); facts.push(("near".into(), id.clone(), "v1".into())); facts.push(("drive".into(), id.clone(), "v1".into())); }
            if w == 1 && (i / 50) % 4 == 2 { objects.push(Object { id: "L1".into(), class: Some("suspended_load".into()), ..Default::default() }); facts.push(("under".into(), id.clone(), "L1".into())); }
            if w == 2 && permit[2] { objects.push(Object { id: "PTW-W2".into(), signal: Some("permit".into()), ..Default::default() }); facts.push(("have".into(), id.clone(), "PTW-W2".into())); }
        }
        for (k, z) in ["Moonpool", "ra2", "hw1"].iter().enumerate() {
            if permit[k] { let pid = format!("PTW-{z}"); objects.push(Object { id: pid.clone(), signal: Some("permit".into()), ..Default::default() }); facts.push(("have".into(), z.to_string(), pid)); }
        }
        if (i / 90) % 2 == 0 { objects.push(Object { id: "ISO-1".into(), signal: Some("isolation-certificate".into()), ..Default::default() }); facts.push(("have".into(), "le1".into(), "ISO-1".into())); }
        if (i / 40) % 5 == 3 { objects.push(Object { id: "f1".into(), class: Some("flame".into()), ..Default::default() }); facts.push(("in".into(), "f1".into(), "hw1".into())); }
        out.push(Frame { t, objects, facts, camera: None });
    }
    out
}


#[test]
fn asp_encoding_derives_what_the_reference_derives() {
    let rs = load_config(CONFIG).unwrap();
    let mut checked = 0;
    for (seed, miss, n) in [(11u64, 0.0, 400), (12, 0.15, 400), (13, 0.3, 400)] {
        let frames = world(seed, n, miss);
        let asp = asp::derive(&rs.rules, &frames, rs.grace_seconds).unwrap();
        // cooldown 0: after each step the active set is exactly what was derived at that frame
        let mut eng = reference::Engine::new(rs.rules.clone(), rs.grace_seconds, 0.0);
        for f in &frames {
            eng.step(f);
            let want = eng.active();
            let got = asp.get(&((f.t * 1000.0).round() as i64)).cloned().unwrap_or_default();
            assert_eq!(got, want, "seed {seed}: derived sets differ at t={}", f.t);
            checked += want.len();
        }
    }
    assert!(checked > 300, "scenario should derive many alerts ({checked})");
    eprintln!("{checked} derived (rule, subject, frame) triples identical between clingo and the reference engine");
}
