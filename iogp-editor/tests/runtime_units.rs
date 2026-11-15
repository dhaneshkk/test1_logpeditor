//! Timer (yappu semantics) and AlertMachine unit tests — no APE needed.
use iogp_editor::runtime::{AlertMachine, Timer};
use std::collections::BTreeSet;

#[test]
fn timer_tolerates_gaps_up_to_grace() {
    let mut t = Timer::new(0.7);
    for (i, v) in [true, true, false, false, true, true].iter().enumerate() { t.observe("k", *v, i as f64 * 0.3); }
    // gap 0.3..1.2 is 0.9 > 0.7 after last true at 0.3? last true 0.3, false at 0.6 (0.3 gap) and 0.9 (0.6 gap) -> kept
    assert!((t.duration("k", 1.5) - 1.5).abs() < 1e-9);
    let mut t = Timer::new(0.7);
    t.observe("k", true, 0.0); t.observe("k", false, 0.5); t.observe("k", false, 1.0); t.observe("k", true, 1.1);
    assert!((t.duration("k", 1.1) - 0.0).abs() < 1e-9, "gap of 1.0 s > grace resets the timer");
}

fn set(keys: &[&str]) -> BTreeSet<(String, String)> { keys.iter().map(|k| (k.to_string(), "p1".to_string())).collect() }

#[test]
fn alert_raised_once_per_episode_and_after_cooldown_again() {
    let mut m = AlertMachine::new(3.0);
    let mut raises = vec![];
    // derived 0..2, absent 2..4 (< cooldown, so same episode), derived 4..5, absent 5..9, derived 9
    for i in 0..=90 {
        let t = i as f64 / 10.0;
        let d = (t < 2.0) || (4.0..5.0).contains(&t) || t >= 9.0;
        let (r, _) = m.step(&if d { set(&["R"]) } else { set(&[]) }, t);
        if !r.is_empty() { raises.push(t); }
    }
    assert_eq!(raises, vec![0.0, 9.0]);
}

#[test]
fn alert_cleared_only_after_cooldown() {
    let mut m = AlertMachine::new(3.0);
    m.step(&set(&["R"]), 0.0);
    let (_, c) = m.step(&set(&[]), 1.0); assert!(c.is_empty());
    let (_, c) = m.step(&set(&[]), 3.9); assert!(c.is_empty());
    let (_, c) = m.step(&set(&[]), 4.0); assert_eq!(c.len(), 1);
    assert!(m.active().is_empty());
}
