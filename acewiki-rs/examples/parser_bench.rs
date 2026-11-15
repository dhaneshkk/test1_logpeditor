//! Predictive-editor benchmark: for every sentence of a wiki, feed its editor tokens one by one and
//! at every prefix compute what the editor shows (abstract + concrete next-token options, complete,
//! reference) — the same work Java AceWiki's PreditorWindow does. No APE involved.
//!   cargo run --release --example parser_bench -- DATA_DIR WIKI [ROUNDS]
use acewiki_rs::chartparser::{ChartParser, Grammar};
use acewiki_rs::engine::{WikiLexicon, ACEWIKI_GRAMMAR};
use acewiki_rs::model::Statement;
use acewiki_rs::storage::Storage;
use std::sync::Arc;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let (data, wiki, rounds) = (&a[1], &a[2], a.get(3).map(|x| x.parse().unwrap()).unwrap_or(3));
    let w = Storage::new(data).load(wiki)?;
    let g = Arc::new(Grammar::from_json(ACEWIKI_GRAMMAR)?);
    let mut sents: Vec<Vec<String>> = vec![];
    for e in w.elements.values() {
        for s in &e.article {
            if let Statement::Sentence { serialized, .. } = s {
                let tes = w.tokenize(serialized, &g.terminals)?;
                sents.push(w.editor_elements(&tes).iter().map(|t| w.original_text(t)).collect());
            }
        }
    }
    if let Ok(n) = std::env::var("LIMIT") { sents.truncate(n.parse()?); }
    let lex = WikiLexicon::from(&w);
    let cache = acewiki_rs::chartparser::parser::RuleCache::default();
    let mut best = f64::MAX;
    let mut checksum = 0usize;
    for r in 0..rounds {
        let t = Instant::now();
        let (mut prefixes, mut opts) = (0usize, 0usize);
        // prepared empty-prefix parser, forked per sentence (what the engine does)
        let mut p0 = ChartParser::new(g.clone(), "text");
        p0.set_dynamic_lexicon(Box::new(lex.clone()));
        p0.set_rule_cache(cache.clone());
        for toks in &sents {
            let mut p = p0.clone();
            for k in 0..=toks.len() {
                if k > 0 { p.add_token(&toks[k - 1]); }
                let o = p.next_token_options();
                opts += o.con.len() + o.abs.len();
                let _ = p.abstract_option_strings();
                let _ = p.is_complete();
                let _ = p.reference();
                prefixes += 1;
            }
        }
        let s = t.elapsed().as_secs_f64();
        best = best.min(s);
        checksum = opts;
        eprintln!("round {r}: {} sentences, {prefixes} prefixes, {opts} options, {s:.3}s ({:.1} µs/prefix)", sents.len(), s * 1e6 / prefixes as f64);
    }
    println!("best {best:.3}s checksum {checksum}");
    Ok(())
}
