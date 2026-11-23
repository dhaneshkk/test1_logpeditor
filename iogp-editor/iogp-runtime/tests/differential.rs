//! The fast engine must produce exactly the reference engine's events: same kind, rule, subject,
//! alert, LSR, severity and time, in the same order, and the same active set after every frame.
//! Proof texts may name a different (equally valid) first binding, so only their header is compared.

use iogp_runtime::{load_config, reference, Engine, Frame, Object};

const CONFIG: &str = include_str!("fixtures/config.json");

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

fn compare(mut a: reference::Engine, mut b: Engine, frames: &[Frame], label: &str) -> usize {
    let mut n = 0;
    for f in frames {
        let ea = a.step(f);
        let eb = b.step(f);
        let key = |e: &iogp_runtime::AlertEvent| (e.kind.clone(), e.rule.clone(), e.subject.clone(), e.alert.clone(), e.lsr, e.severity.clone(), e.t.to_bits());
        assert_eq!(ea.iter().map(key).collect::<Vec<_>>(), eb.iter().map(key).collect::<Vec<_>>(), "{label}: events differ at t={}", f.t);
        for e in &eb {
            if e.kind == "raised" {
                assert!(e.proof.first().is_some_and(|h| h.starts_with(&format!("rule {}:", e.rule))), "{label}: proof header missing at t={}: {:?}", f.t, e.proof);
                let ra = ea.iter().find(|x| x.rule == e.rule && x.subject == e.subject).unwrap();
                assert_eq!(ra.proof.len(), e.proof.len(), "{label}: proof length differs at t={}\n{:?}\n{:?}", f.t, ra.proof, e.proof);
            }
        }
        assert_eq!(a.active(), b.active(), "{label}: active sets differ at t={}", f.t);
        n += eb.len();
    }
    n
}

#[test]
fn fast_engine_matches_reference_in_all_modes() {
    let rs = load_config(CONFIG).unwrap();
    assert_eq!(rs.rules.len(), 15);
    let mut total = 0;
    for (seed, miss) in [(1u64, 0.0), (2, 0.1), (3, 0.25), (4, 0.4)] {
        let frames = world(seed, 6000, miss);
        let mk = || (reference::Engine::new(rs.rules.clone(), rs.grace_seconds, rs.cooldown_seconds), Engine::new(rs.rules.clone(), rs.grace_seconds, rs.cooldown_seconds));
        let (a, b) = mk();
        total += compare(a, b, &frames, &format!("smoothing seed {seed} miss {miss}"));
        let (a, b) = mk();
        total += compare(a.yappu_compatible(), b.yappu_compatible(), &frames, &format!("yappu seed {seed}"));
        let (mut a, mut b) = mk();
        a.use_durations = false; b.use_durations = false;
        total += compare(a, b, &frames, &format!("per-frame seed {seed}"));
    }
    assert!(total > 500, "the scenario should exercise many alerts, got {total}");
    eprintln!("{total} identical events");
}

#[test]
fn builder_api_equals_frame_api_and_symbols_are_collected() {
    let rs = load_config(CONFIG).unwrap();
    let frames = world(9, 3000, 0.2);
    let mut a = Engine::new(rs.rules.clone(), rs.grace_seconds, rs.cooldown_seconds);
    let mut b = Engine::new(rs.rules.clone(), rs.grace_seconds, rs.cooldown_seconds);
    for f in &frames {
        let ea = a.step(f);
        b.begin(f.t, f.camera.as_deref());
        for o in &f.objects {
            use iogp_runtime::fast::Kind;
            for (k, ty) in [(Kind::Class, &o.class), (Kind::Zone, &o.zone), (Kind::Signal, &o.signal)] { if let Some(ty) = ty { b.object(&o.id, k, ty, o.camera.as_deref()); } }
        }
        for (r, x, y) in &f.facts { b.fact(r, x, y); }
        let eb = b.end().to_vec();
        assert_eq!(ea, eb, "t={}", f.t);
    }
    // a long stream of ever-new track ids must not grow the symbol table without bound
    let mut e = Engine::new(rs.rules.clone(), rs.grace_seconds, rs.cooldown_seconds);
    for i in 0..20_000u32 {
        let id = format!("track-{i}");
        e.begin(i as f64 / 10.0, None);
        e.object("hz", iogp_runtime::fast::Kind::Zone, "height-zone", None);
        e.object(&id, iogp_runtime::fast::Kind::Class, "person", None);
        e.fact("in", &id, "hz");
        e.end();
    }
    assert!(e.symbols() < 2_000, "symbols: {}", e.symbols());
}

#[test]
fn config_errors_are_reported() {
    assert!(Engine::from_config("{}").is_err());
    let bad = CONFIG.replacen("\"subject\": \"W\"", "\"subject\": \"Q\"", 1);
    let e = Engine::from_config(&bad).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(e.contains("rule LSR1") && (e.contains("undefined variable") || e.contains("not an outer variable")), "{e}");
}

/// An object can have more than one type: a zone that is also a detected class, or a track that is both
/// a `person` and a `fall_pose`. Every type must stay believed for `grace` seconds after it was last
/// seen, not only the first one — otherwise one missed frame drops the object from the rule's binding
/// and restarts its "for at least" timer. Here a height-zone that is also a detected ladder is missed
/// for one frame at t = 1.5 s while a worker stands in it without a harness: LSR9 (3 s) must still be
/// raised at t = 3.0 s, by the reference engine, by the fast engine's frame API and by its builder API
/// (where the second type of an id used to be ignored when remembering).
#[test]
fn every_type_of_an_object_is_bridged_over_missed_frames() {
    let rs = load_config(CONFIG).unwrap();
    let frame = |k: usize| -> Frame {
        let t = k as f64 * 0.1;
        let mut objects = vec![Object { id: "7".into(), class: Some("person".into()), ..Default::default() }];
        if k != 15 {
            objects.push(Object { id: "Z1".into(), class: Some("ladder".into()), zone: Some("height-zone".into()), ..Default::default() });
        }
        Frame { t, objects, facts: vec![("in".into(), "7".into(), "Z1".into())], camera: None }
    };
    let raised = |evs: &[iogp_runtime::AlertEvent], t: f64| evs.iter().filter(|e| e.kind == "raised" && e.rule == "LSR9").map(|_| t).next();
    let (mut r, mut f, mut b) = (reference::Engine::new(rs.rules.clone(), rs.grace_seconds, rs.cooldown_seconds),
                                 Engine::new(rs.rules.clone(), rs.grace_seconds, rs.cooldown_seconds),
                                 Engine::new(rs.rules.clone(), rs.grace_seconds, rs.cooldown_seconds));
    let (mut tr, mut tf, mut tb) = (None, None, None);
    for k in 0..60 {
        let fr = frame(k);
        tr = tr.or(raised(&r.step(&fr), fr.t));
        tf = tf.or(raised(&f.step(&fr), fr.t));
        // builder API, class given first: the zone type is the id's *second* type
        b.begin(fr.t, None);
        for o in &fr.objects {
            use iogp_runtime::fast::Kind;
            if let Some(c) = &o.class { b.object(&o.id, Kind::Class, c, None); }
            if let Some(z) = &o.zone { b.object(&o.id, Kind::Zone, z, None); }
        }
        for (x, y, z) in &fr.facts { b.fact(x, y, z); }
        tb = tb.or(raised(b.end(), fr.t));
    }
    let close = |t: Option<f64>| t.is_some_and(|t| (t - 3.0).abs() < 1e-9);
    assert!(close(tr), "reference engine raised LSR9 at {tr:?}, expected 3.0");
    assert!(close(tf), "fast engine (frame API) raised LSR9 at {tf:?}, expected 3.0");
    assert!(close(tb), "fast engine (builder API) raised LSR9 at {tb:?}, expected 3.0");
}
