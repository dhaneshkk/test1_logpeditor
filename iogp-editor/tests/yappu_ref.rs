//! The Rust port of yappu-fsm's vision engine and of the old Python parity script.
//! The SHA-256 values are of the files the real Python yappu-fsm wrote (`tools/yappu_parity.py`,
//! removed after the port) for these seeds; the port reproduces them byte for byte.
use iogp_editor::yappu_ref::{parity_frames, parse_rules, PyRandom, YAPPU_RULES};
use sha2::{Digest, Sha256};

fn sha(seed: u64) -> String {
    let s = parity_frames(seed).join("\n") + "\n";
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn python_random_matches_cpython() {
    // python3 -c "import random; r=random.Random(459); print(repr(r.random()), r.choice([10,20,30]), repr(r.uniform(1,8)))"
    let mut r = PyRandom::new(459);
    assert_eq!(r.random(), 0.9592810917579966);
    assert_eq!([10, 20, 30][r.choice_index(3)], 30);
    assert_eq!(r.uniform(1.0, 8.0), 2.048432207126128);
}

#[test]
fn parity_frames_match_python_yappu_byte_for_byte() {
    assert_eq!(sha(459), "0c85ca78d2f0471a5b484d28cdfcd623d8d53a9ea18a7d72660cf33829ea10dc");
    assert_eq!(sha(7), "24ceec1d2cbd4adb867b239d5438093423e12189becb05808079f021ae2a3872");
    assert_eq!(sha(1099511627779), "0fb8b4a903d4152aa9aef0a94b5d8debd24ff06bc7c931bab4f617102d73bb3b");
}

#[test]
fn yappu_rule_language() {
    let r = parse_rules(YAPPU_RULES).unwrap();
    assert_eq!(r.len(), 3);
    assert!(parse_rules("X: If the person T holds a unicorn then a u alert is raised for the person T.").is_err());
    assert!(parse_rules("X: If the person T does not hold a cell-phone then a u alert is raised for the person T.").is_err());
}
