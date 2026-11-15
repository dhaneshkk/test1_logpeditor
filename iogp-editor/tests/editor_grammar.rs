//! The predictive editor grammar must accept every template and every positive acceptance sentence
//! word by word (ending complete), and must not offer continuations the compiler rejects.
use iogp_editor::{catalog::Catalog, grammar::RuleGrammar};

fn positives(cat: &Catalog) -> Vec<String> {
    let mut v: Vec<String> = cat.templates.iter().map(|t| t.text.clone()).collect();
    for l in include_str!("../data/acceptance.ace").lines().filter(|l| l.starts_with('+')) {
        v.push(l.splitn(2, ": ").nth(1).unwrap().to_string());
    }
    for l in include_str!("../tools/yappu_parity_rules.ace").lines().filter(|l| !l.starts_with('#') && l.contains(": ")) {
        v.push(l.splitn(2, ": ").nth(1).unwrap().to_string());
    }
    v
}

#[test]
fn templates_and_acceptance_sentences_are_typeable() {
    let cat = Catalog::builtin();
    let g = RuleGrammar::new(&cat).unwrap();
    let mut failures = vec![];
    for t in positives(&cat) {
        let mut p = g.parser();
        match acewiki_rs::web::feed_text(&mut p, &t) {
            Ok(_) if p.is_complete() => {}
            Ok(toks) => failures.push(format!("incomplete after {:?}: {t}", toks)),
            Err((_, m)) => failures.push(format!("{m}\n    {t}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn suggestions_respect_sorts() {
    let cat = Catalog::builtin();
    let g = RuleGrammar::new(&cat).unwrap();
    let mut p = g.parser();
    acewiki_rs::web::feed_text(&mut p, "If a worker wears a").unwrap();
    let words: Vec<String> = p.next_token_options().con.iter().map(|c| c.word.clone()).collect();
    assert!(words.contains(&"harness".to_string()));
    assert!(!words.contains(&"permit".to_string()), "signals are not wearable: {words:?}");
    let mut p = g.parser();
    acewiki_rs::web::feed_text(&mut p, "If a restricted-area has a").unwrap();
    let words: Vec<String> = p.next_token_options().con.iter().map(|c| c.word.clone()).collect();
    assert_eq!(words, vec!["permit".to_string(), "gas-test".to_string()].into_iter().filter(|w| words.contains(w)).collect::<Vec<_>>());
    assert!(!words.contains(&"worker".to_string()));
}

#[test]
fn scoping_and_exceptions_are_typeable() {
    let cat = Catalog::builtin();
    let g = RuleGrammar::new(&cat).unwrap();
    for t in [
        "If a worker is in Rig-Floor-East and does not wear a harness for at least 3 seconds then VisionGuard raises a critical working-at-height-alert for the worker.",
        "If a worker is in a height-zone of CAM-03 and does not wear a harness for at least 3 seconds then VisionGuard raises a critical working-at-height-alert for the worker.",
        "If a worker is in Moonpool then VisionGuard raises a high work-authorisation-alert for the worker unless Moonpool has a permit or the worker has a permit.",
        "If a worker is in a restricted-area for at least 10 seconds then VisionGuard raises a high work-authorisation-alert for the worker unless the restricted-area has a permit.",
    ] {
        let mut p = g.parser();
        let r = acewiki_rs::web::feed_text(&mut p, t);
        assert!(r.is_ok() && p.is_complete(), "{t}: {r:?}");
    }
    // after "unless the restricted-area has a" only records are offered
    let mut p = g.parser();
    acewiki_rs::web::feed_text(&mut p, "If a worker is in a restricted-area then VisionGuard raises a high work-authorisation-alert unless the restricted-area has a").unwrap();
    let words: Vec<String> = p.next_token_options().con.iter().map(|c| c.word.clone()).collect();
    assert!(words.contains(&"permit".to_string()) && !words.contains(&"harness".to_string()), "{words:?}");
}
