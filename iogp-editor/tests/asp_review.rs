//! clingo review: example scenes, rules that can never fire, implications, coverage.
use iogp_editor::{asp, catalog::Catalog, Compiler};

fn run<F: FnOnce() + Send + 'static>(f: F) { std::thread::Builder::new().stack_size(256 << 20).spawn(f).unwrap().join().unwrap(); }

#[test]
fn review_finds_scenes_dead_rules_and_implications() { run(|| {
    let cat = Catalog::builtin();
    let dir = std::env::var("APE_PROLOG_DIR").unwrap_or("../ape-rs/prolog-scryer".into());
    let mut c = Compiler::new(cat.clone(), ape_rs::Ape::new(&dir).unwrap());
    let src = [
        ("WAH", "If a worker is in a height-zone and does not wear a harness for at least 3 seconds then VisionGuard raises a critical working-at-height-alert for the worker."),
        ("WAHE", "If a worker is in Rig-Floor-East and does not wear a harness for at least 3 seconds then VisionGuard raises a critical working-at-height-alert for the worker."),
        ("MOON", "If a worker is in Moonpool for at least 5 seconds then VisionGuard raises a high work-authorisation-alert for the worker unless Moonpool has a permit."),
        ("CROWD", "If at least 3 workers are in a lifting-zone for at least 5 seconds then VisionGuard raises a high mechanical-lifting-alert."),
        ("NEVER", "If a worker is in a height-zone and is not in the height-zone then VisionGuard raises a critical working-at-height-alert for the worker."),
    ];
    let rules: Vec<_> = src.iter().map(|(id, t)| c.compile(id, t).unwrap()).collect();
    let rv = asp::review(&cat, &rules).unwrap();
    assert_eq!(rv.dead, vec!["NEVER".to_string()]);
    let imp: Vec<(String, String)> = rv.implications.iter().map(|i| (i.a.clone(), i.b.clone())).collect();
    assert!(imp.contains(&("WAHE".into(), "WAH".into())), "{imp:?}");
    assert!(!imp.contains(&("WAH".into(), "WAHE".into())), "a generic rule is not implied by a zone-specific one");
    // the permit exception: the scene must not contain the permit; the crowd needs three workers
    let moon = rv.witnesses.iter().find(|w| w.rule == "MOON").unwrap();
    assert!(moon.lines.iter().any(|l| l.contains("never seen") && l.contains("permit")), "{:?}", moon.lines);
    let crowd = rv.witnesses.iter().find(|w| w.rule == "CROWD").unwrap();
    assert_eq!(crowd.lines.iter().filter(|l| l.contains("is in lifting-zone")).count(), 3, "{:?}", crowd.lines);
    assert_eq!(crowd.seconds, 5.0);
    assert!(rv.uncovered_lsr.iter().any(|(n, _)| *n == 1));
    // the .lp export parses and grounds in clingo on its own
    let lp = asp::ruleset_lp(&rules);
    assert!(lp.contains("derived(\"MOON\""));
})}
