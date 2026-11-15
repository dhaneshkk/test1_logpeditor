//! The Rust port of the former `tools/sample_vg360_log.py`: the SHA-256 values are of the four
//! files (site.json, detections.jsonl, labels.jsonl, rules.ace, concatenated) the Python script
//! wrote for these seeds; the port reproduces them byte for byte.
use iogp_editor::sample_site::{generate, PyJson};
use sha2::{Digest, Sha256};

fn sha(seed: u64) -> String {
    let s = generate(seed);
    let all = format!("{}{}{}{}", s.site, s.detections, s.labels, s.rules);
    Sha256::digest(all.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn sample_site_matches_python_byte_for_byte() {
    assert_eq!(sha(7), "5b6485d39e090e1e7ae537a554c6c10549a3900e91a4d9da422cbf9d53a1f4bd");
    assert_eq!(sha(42), "4466fc249ad9e2da68eefc8737634d240dcef59148ba324368ad0305faeee1e6");
}

#[test]
fn sample_site_shape() {
    let s = generate(7);
    assert_eq!((s.frames, s.incidents), (2400, 3));
    let site: serde_json::Value = serde_json::from_str(&s.site).unwrap();
    assert_eq!(site["cameras"].as_array().unwrap().len(), 2);
    for l in s.detections.lines() { let _: serde_json::Value = serde_json::from_str(l).unwrap(); }
}

#[test]
fn python_json_formatting() {
    // json.dumps({"a": [1, 2.5, "x"], "b": []}) and json.dumps(..., indent=1)
    let v = PyJson::Obj(vec![("a".into(), PyJson::List(vec![PyJson::Int(1), PyJson::Float(2.5), PyJson::Str("x".into())])), ("b".into(), PyJson::List(vec![]))]);
    assert_eq!(v.dumps(), r#"{"a": [1, 2.5, "x"], "b": []}"#);
    assert_eq!(v.dumps_indent(1, 0), "{\n \"a\": [\n  1,\n  2.5,\n  \"x\"\n ],\n \"b\": []\n}");
}
