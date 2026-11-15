//! Golden parity test: outputs of ape-rs (Rust + Scryer) must be byte-identical to SWI-Prolog APE
//! for deterministic output modes. Fixtures were produced by the original APE on SWI-Prolog 9.0.4.
use ape_rs::Ape;
use serde::Deserialize;

#[derive(Deserialize)]
struct Fx { n: u32, text: String, mode: String, ct: String, c: String }

#[test]
fn byte_parity_with_swi_ape() {
    let h = std::thread::Builder::new().stack_size(256 << 20).spawn(|| {
        let fx: Vec<Fx> = serde_json::from_str(include_str!("fixtures_swi.json")).unwrap();
        let dir = std::env::var("APE_PROLOG_DIR").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/prolog-scryer").into());
        let mut ape = Ape::new(dir).expect("load APE");
        let mut bad = Vec::new();
        for f in &fx {
            let r = ape.solo(&f.text, &f.mode).expect("ape call");
            if r.content != f.c || r.content_type != f.ct {
                bad.push(format!("#{} [{}] {:?}\n  swi: {:?}\n  rs:  {:?}", f.n, f.mode, f.text, f.c, r.content));
            }
        }
        assert!(bad.is_empty(), "{} / {} mismatches:\n{}", bad.len(), fx.len(), bad.join("\n"));
        fx.len()
    }).unwrap();
    let n = h.join().unwrap();
    eprintln!("{n} fixtures byte-identical");
}

#[test]
fn quote_atom_is_injection_safe() {
    assert_eq!(ape_rs::quote_atom("John's \\ x"), "'John\\'s \\\\ x'");
    assert_eq!(ape_rs::quote_atom("a'), halt, ('"), "'a\\'), halt, (\\''");
}

#[test]
fn openai_model_mapping() {
    assert_eq!(ape_rs::openai::model_to_output(Some("ape-fol")).unwrap(), "fol");
    assert_eq!(ape_rs::openai::model_to_output(None).unwrap(), "drspp");
    assert!(ape_rs::openai::model_to_output(Some("gpt-4o")).is_err());
}
