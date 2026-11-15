//! `acewiki` — the AceWiki server (wiki UI, ACE Editor, OpenAI-compatible endpoint).
//!
//!   acewiki serve --data DIR --wiki geo [--profile OWL2RL] [--addr 127.0.0.1:9090] [--owllink URL] [--read-only]
//!   acewiki ask   --data DIR --wiki geo "Which country borders Germany?"
//!   acewiki import --data DIR --wiki geo        # .acewikidata -> directory form

use acewiki_rs::engine::Config;
use acewiki_rs::owl::profile::Profile;
use acewiki_rs::web::{self, ServeOpts};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "AceWiki in Rust (ape-rs + Scryer Prolog)")]
struct Cli { #[command(subcommand)] cmd: Cmd }

#[derive(clap::Args, Clone)]
struct Common {
    /// Directory containing wiki data (`<wiki>/` directories or `<wiki>.acewikidata` files)
    #[arg(long)] data: PathBuf,
    #[arg(long)] wiki: String,
    /// OWL profile: OWL2EL, OWL2RL, OWL2QL is mapped to RL, OWL2Full uses OWLlink if configured
    #[arg(long, default_value = "OWL2RL")] profile: String,
    #[arg(long, default_value = "http://attempto.ifi.uzh.ch/acewiki/")] base_uri: String,
    /// OWLlink server used for axioms outside the native profile (or always with --reasoner owllink)
    #[arg(long)] owllink: Option<String>,
    #[arg(long, default_value = "native")] reasoner: String,
    /// global_restrictions_policy: no_chains (default) or unchecked
    #[arg(long, default_value = "no_chains")] restrictions: String,
    #[arg(long, env = "APE_PROLOG_DIR", default_value = "../ape-rs/prolog-scryer")] prolog_dir: PathBuf,
}

#[derive(Subcommand)]
enum Cmd {
    Serve {
        #[command(flatten)] c: Common,
        #[arg(long, default_value = "127.0.0.1:9090")] addr: String,
        /// Lexicon file of the ACE Editor (edited in place); default: built-in example lexicon
        #[arg(long)] editor_lexicon: Option<PathBuf>,
        #[arg(long)] read_only: bool,
    },
    Ask { #[command(flatten)] c: Common, #[arg(long, default_value = "acewiki-ask")] model: String, text: String },
    Import { #[command(flatten)] c: Common },
    /// Show the axioms mentioning a word and what the reasoner infers about it
    Inspect { #[command(flatten)] c: Common, word: String },
}

fn opts(c: &Common, editor_lexicon: Option<PathBuf>, read_only: bool) -> ServeOpts {
    ServeOpts {
        data: c.data.clone(), wiki: c.wiki.clone(), prolog_dir: c.prolog_dir.clone(), editor_lexicon, read_only,
        cfg: Config { base_uri: c.base_uri.clone(), profile: Profile::parse(&c.profile), reasoner: c.reasoner.clone(),
            owllink_url: c.owllink.clone(), no_chains: c.restrictions != "unchecked" },
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Serve { c, addr, editor_lexicon, read_only } => {
            let w = web::spawn_worker(opts(&c, editor_lexicon, read_only))?;
            let l = tokio::net::TcpListener::bind(&addr).await?;
            eprintln!("acewiki listening on http://{addr}  (wiki /, ACE Editor /aceeditor, OpenAI /v1/chat/completions)");
            axum::serve(l, web::router(w)).await?;
        }
        Cmd::Ask { c, model, text } => {
            let w = web::spawn_worker(opts(&c, None, true))?;
            println!("{}", w.run(move |st| web::chat_answer(st, &model, &text)).await);
        }
        Cmd::Inspect { c, word } => {
            let w = web::spawn_worker(opts(&c, None, true))?;
            let out = w.run(move |st| {
                let eng = &mut st.eng;
                let Some(e) = eng.wiki.by_word(&word).cloned() else { return format!("no word {word}") };
                let iri = eng.iri(&e);
                let mut s = format!("{} {:?} <{iri}>\n", e.id, e.kind);
                for a in eng.ontology_axioms() {
                    let mut x = String::new(); acewiki_rs::owl::w_axiom(&mut x, &a);
                    if x.contains(&format!("\"{iri}\"")) && !matches!(a, acewiki_rs::owl::Axiom::Declaration(..) | acewiki_rs::owl::Axiom::DifferentIndividuals(_)) { s += &format!("  {a:?}\n"); }
                }
                let names = |eng: &acewiki_rs::engine::Engine, v: Vec<u64>| v.iter().map(|i| eng.wiki.elements[i].word()).collect::<Vec<_>>().join(", ");
                match e.kind {
                    acewiki_rs::model::Kind::Noun => {
                        let (sup, sub, ind, sat) = (eng.super_concepts(e.id), eng.sub_concepts(e.id), eng.individuals_of(e.id), eng.is_satisfiable(e.id));
                        s += &format!("satisfiable: {sat}\nsuper: {}\nsub: {}\nindividuals: {}\n", names(eng, sup), names(eng, sub), names(eng, ind));
                    }
                    acewiki_rs::model::Kind::ProperName => { let t = eng.concepts_of(e.id); s += &format!("types: {}\n", names(eng, t)); }
                    _ => {}
                }
                s += &format!("reasoner: {}\n", eng.reasoner_note);
                s
            }).await;
            print!("{out}");
        }
        Cmd::Import { c } => {
            let _ = web::spawn_worker(opts(&c, None, false))?;
            eprintln!("done");
        }
    }
    Ok(())
}
