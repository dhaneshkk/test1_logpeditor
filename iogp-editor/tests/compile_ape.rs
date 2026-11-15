//! Compiler against APE: acceptance set decisions, error messages, FOL shape, runtime behaviour.
use iogp_editor::{catalog::Catalog, runtime::{Engine, Frame, Object}, Compiler};

fn compiler() -> Compiler {
    let dir = std::env::var("APE_PROLOG_DIR").unwrap_or("../ape-rs/prolog-scryer".into());
    Compiler::new(Catalog::builtin(), ape_rs::Ape::new(&dir).expect("APE"))
}

fn run_with_stack<F: FnOnce() + Send + 'static>(f: F) {
    std::thread::Builder::new().stack_size(256 << 20).spawn(f).unwrap().join().unwrap();
}

#[test]
fn acceptance_and_errors() { run_with_stack(|| {
    let mut c = compiler();
    let (mut tp, mut tn, mut fp, mut fn_) = (0, 0, 0, 0);
    for l in include_str!("../data/acceptance.ace").lines().filter(|l| l.starts_with('+') || l.starts_with('-')) {
        let (label, s) = l.split_once(": ").unwrap();
        let want = label.starts_with('+');
        let got = c.compile("t", s).is_ok();
        match (want, got) { (true, true) => tp += 1, (false, false) => tn += 1, (false, true) => fp += 1, (true, false) => fn_ += 1 }
        assert_eq!(want, got, "{label}: {s} -> {:?}", c.compile("t", s).err());
    }
    assert!(tp >= 10 && tn >= 12 && fp == 0 && fn_ == 0);
    // misspelling: one error, with the correction
    let e = c.compile("t", "If a workr is in a height-zone then VisionGuard raises a critical working-at-height-alert for the workr.").unwrap_err();
    assert_eq!(e.len(), 1, "{e:?}"); assert!(e[0].hint.contains("worker"));
    // negated visual condition needs a duration
    let e = c.compile("t", "If a worker is in a height-zone and does not wear a harness then VisionGuard raises a critical working-at-height-alert for the worker.").unwrap_err();
    assert!(e.iter().any(|x| x.message.contains("duration") || x.hint.contains("for at least")), "{e:?}");
})}

#[test]
fn fol_and_runtime() { run_with_stack(|| {
    let mut c = compiler();
    let r = c.compile("H", "If a worker is in a height-zone and does not wear a harness for at least 3 seconds then VisionGuard raises a critical working-at-height-alert for the worker.").unwrap();
    assert!(r.fol.contains("within(S, T, 3)") && r.fol.contains("alert(working_at_height, critical, W, T)"), "{}", r.fol);
    let mut eng = Engine::new(vec![r], 0.7, 3.0);
    let mut raised = vec![];
    for i in 0..80 {
        let t = i as f64 / 10.0;
        let mut objects = vec![Object { id: "p1".into(), class: Some("person".into()), ..Default::default() }, Object { id: "z".into(), zone: Some("height-zone".into()), ..Default::default() }];
        let mut facts = vec![("in".to_string(), "p1".to_string(), "z".to_string())];
        // harness seen until 0.9 s, then flickers back for single frames (2.0 s, 2.2 s) -> keeps resetting
        if t < 1.0 || (t - 2.0).abs() < 1e-9 || (t - 2.2).abs() < 1e-9 {
            objects.push(Object { id: "h".into(), class: Some("harness".into()), ..Default::default() });
            facts.push(("wear".into(), "p1".into(), "h".into()));
        }
        for e in eng.step(&Frame { t, objects, facts, camera: None }) { if e.kind == "raised" { raised.push(e.t); } }
    }
    // last harness sighting 2.2 s -> still believed at 2.9 s (gap = grace) -> absent from 3.0 s -> 3 s later: 6.0 s
    assert_eq!(raised.len(), 1); assert!((raised[0] - 6.0).abs() < 1e-6, "{raised:?}");
})}

fn obj(id: &str, class: Option<&str>, zone: Option<&str>, signal: Option<&str>, camera: Option<&str>) -> Object {
    Object { id: id.into(), class: class.map(Into::into), zone: zone.map(Into::into), signal: signal.map(Into::into), camera: camera.map(Into::into) }
}

#[test]
fn scoping_and_exceptions_at_runtime() { run_with_stack(|| {
    let mut c = compiler();
    let a = c.compile("MOON", "If a worker is in Moonpool then VisionGuard raises a high work-authorisation-alert for the worker unless Moonpool has a permit.").unwrap();
    assert!(a.conditions.iter().any(|x| x.exception));
    let b = c.compile("CAM3", "If a worker is in a height-zone of CAM-03 then VisionGuard raises a critical working-at-height-alert for the worker.").unwrap();
    let mut eng = Engine::new(vec![a, b], 0.0, 0.0);
    let f = |t: f64, objects: Vec<Object>, facts: Vec<(&str, &str, &str)>| Frame { t, objects, facts: facts.into_iter().map(|(r, x, y)| (r.into(), x.into(), y.into())).collect(), camera: None };
    let zones = || vec![obj("Moonpool", None, Some("restricted-area"), None, Some("CAM-02")), obj("Other-Area", None, Some("restricted-area"), None, Some("CAM-02")),
                        obj("hz3", None, Some("height-zone"), None, Some("CAM-03")), obj("hz4", None, Some("height-zone"), None, Some("CAM-04"))];
    let mut ev = |fr: Frame| eng.step(&fr).into_iter().filter(|e| e.kind == "raised").map(|e| format!("{}:{}", e.rule, e.subject)).collect::<Vec<_>>();
    // worker in another restricted area and in a CAM-04 height zone: nothing
    let mut o = zones(); o.push(obj("w1", Some("person"), None, None, None));
    assert!(ev(f(0.0, o, vec![("in", "w1", "Other-Area"), ("in", "w1", "hz4")])).is_empty());
    // Moonpool with a permit: still nothing (exception); CAM-03 zone: raised
    let mut o = zones(); o.push(obj("w1", Some("person"), None, None, None)); o.push(obj("ptw1", None, None, Some("permit"), None));
    assert_eq!(ev(f(1.0, o, vec![("in", "w1", "Moonpool"), ("in", "w1", "hz3"), ("have", "Moonpool", "ptw1")])), vec!["CAM3:w1".to_string()]);
    // permit withdrawn: Moonpool alert raised
    let mut o = zones(); o.push(obj("w1", Some("person"), None, None, None));
    assert_eq!(ev(f(2.0, o, vec![("in", "w1", "Moonpool"), ("in", "w1", "hz3")])), vec!["MOON:w1".to_string()]);
})}
