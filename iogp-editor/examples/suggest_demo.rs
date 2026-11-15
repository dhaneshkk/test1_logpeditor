use iogp_editor::{catalog::Catalog, grammar::RuleGrammar, suggest};
fn main() {
    let cat = Catalog::builtin();
    let g = RuleGrammar::new(&cat).unwrap();
    let m = suggest::train(&g, cat.templates.iter().map(|t| t.text.clone()));
    for t in std::env::args().skip(1) {
        let s = suggest::suggest(&g, &cat, &m, &t);
        println!("{t:?}\n  partial={:?} complete={} err={:?}\n  top: {:?}\n  ghost: {:?}", s.partial, s.complete, s.error,
            s.options.iter().take(6).map(|o| o.w.as_str()).collect::<Vec<_>>(), s.ghost);
    }
}
