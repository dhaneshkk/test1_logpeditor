//! iogp — compile controlled-English IOGP alert rules; serve the editor.
use clap::{Parser, Subcommand};
use iogp_editor::{catalog::Catalog, config, parse_lines, Compiler};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "IOGP Life-Saving Rule alert editor (ACE via ape-rs -> FOL + detector config)")]
struct Cli {
    #[arg(long, env = "APE_PROLOG_DIR", default_value = "../ape-rs/prolog-scryer", global = true)]
    prolog_dir: PathBuf,
    /// detection vocabulary (JSON); default: built-in VisionGuard360 catalog
    #[arg(long, global = true)]
    catalog: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Compile sentences (one per line, optional "ID: " prefix) from a file or arguments
    Compile { #[arg(long)] file: Option<PathBuf>, #[arg(long, default_value = "json")] format: String, sentences: Vec<String> },
    /// Print the ACE lexicon generated from the catalog
    Lexicon,
    /// Write the rules as a clingo program (answer set programming), for the detection team or review
    Lp { #[arg(long)] file: PathBuf },
    /// Review a rule set with clingo: example scene per rule, rules that can never fire, rules
    /// implied by other rules, Life-Saving Rules and named zones with no rule
    Review { #[arg(long)] file: PathBuf, #[arg(long)] json: bool },
    /// Show what APE makes of a sentence (DRS, paraphrase, messages) — for debugging the language
    Drs { sentence: String },
    /// Serve the editor UI and the OpenAI-compatible endpoint
    Serve {
        #[arg(long, default_value = "127.0.0.1:9191")] addr: String,
        /// rule store (versions); the audit log and compile-attempt log live next to it
        #[arg(long, default_value = "iogp_rules.json")] rules: PathBuf,
        /// users file (names, roles, salted password hashes); without it: demo mode
        #[arg(long)] users: Option<PathBuf>,
    },
    /// Audit log tools
    Audit { #[command(subcommand)] cmd: AuditCmd },
    /// Manage the users file: iogp user add NAME --role author --role approver --password ...
    User { #[command(subcommand)] cmd: UserCmd },
}

#[derive(Subcommand)]
enum AuditCmd {
    /// Check every entry's SHA-256 and its link to the previous entry
    Verify { file: PathBuf },
}

#[derive(Subcommand)]
enum UserCmd {
    /// Add or replace a user
    Add { name: String, #[arg(long = "role", required = true)] roles: Vec<String>, #[arg(long, env = "IOGP_PASSWORD")] password: String, #[arg(long, default_value = "users.json")] file: PathBuf },
    /// List users and roles
    List { #[arg(long, default_value = "users.json")] file: PathBuf },
}

fn catalog(p: &Option<PathBuf>) -> anyhow::Result<Catalog> {
    Ok(match p { Some(p) => Catalog::from_json(&std::fs::read_to_string(p)?)?, None => Catalog::builtin() })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cat = catalog(&cli.catalog)?;
    match cli.cmd {
        Cmd::Lexicon => print!("{}", cat.lexicon_text()),
        Cmd::Lp { file } => {
            let mut c = Compiler::new(cat.clone(), ape_rs::Ape::new(&cli.prolog_dir)?);
            let rules = compile_all(&mut c, &std::fs::read_to_string(file)?)?;
            print!("{}", iogp_editor::asp::ruleset_lp(&rules));
        }
        Cmd::Review { file, json } => {
            let mut c = Compiler::new(cat.clone(), ape_rs::Ape::new(&cli.prolog_dir)?);
            let rules = compile_all(&mut c, &std::fs::read_to_string(file)?)?;
            let rv = iogp_editor::asp::review(&cat, &rules).map_err(|e| anyhow::anyhow!(e))?;
            if json { println!("{}", serde_json::to_string_pretty(&rv)?); } else { print_review(&rv); }
        }
        Cmd::Drs { sentence } => {
            let mut c = Compiler::new(cat.clone(), ape_rs::Ape::new(&cli.prolog_dir)?);
            let o = c.analyze(&c.prepare(&sentence).0)?;
            println!("DRS:\n{}\n\nparaphrase: {}\n", o.drs, o.paraphrase);
            for m in o.messages { println!("{} {} {}", m.0, m.1, m.2); }
        }
        Cmd::Compile { file, format, sentences } => {
            let mut items = match &file { Some(f) => parse_lines(&std::fs::read_to_string(f)?), None => vec![] };
            for (i, s) in sentences.iter().enumerate() { items.push((format!("r{}", i + 1), s.clone())); }
            let mut c = Compiler::new(cat.clone(), ape_rs::Ape::new(&cli.prolog_dir)?);
            let (mut ok, mut bad) = (vec![], 0);
            for (id, s) in &items {
                match c.compile(id, s) {
                    Ok(r) => ok.push(r),
                    Err(es) => { bad += 1; eprintln!("✗ {id}: {s}"); for e in es { eprintln!("    {} — {}", e.message, e.hint); } }
                }
            }
            match format.as_str() {
                "tptp" => print!("{}", config::ruleset_tptp(&cat, &ok)),
                "readback" => for r in &ok { println!("{}: {}\n  {}\n", r.id, r.text, r.readback.join("\n  • ")); },
                _ => println!("{}", serde_json::to_string_pretty(&config::ruleset_json(&cat, &ok))?),
            }
            if bad > 0 { std::process::exit(2); }
        }
        Cmd::Serve { addr, rules, users } => iogp_editor::web::serve(iogp_editor::web::ServeOpts { cat, prolog_dir: cli.prolog_dir, addr, rules, users }).await?,
        Cmd::Audit { cmd: AuditCmd::Verify { file } } => match iogp_editor::store::verify_audit(&file) {
            Ok((n, head)) => println!("audit log OK: {n} entries, head {head}"),
            Err(e) => { eprintln!("audit log BROKEN: {e}"); std::process::exit(3); }
        },
        Cmd::User { cmd: UserCmd::Add { name, roles, password, file } } => {
            use iogp_editor::store::{Role, Users};
            let roles: Vec<Role> = roles.iter().map(|r| match r.as_str() { "author" => Ok(Role::Author), "approver" => Ok(Role::Approver), "admin" => Ok(Role::Admin), x => Err(anyhow::anyhow!("unknown role '{x}' (author, approver, admin)")) }).collect::<anyhow::Result<_>>()?;
            let mut us = if file.exists() { Users::load(&file)? } else { Users::default() };
            us.set(&name, roles, &password);
            us.save(&file)?;
            println!("{name} saved to {}", file.display());
        }
        Cmd::User { cmd: UserCmd::List { file } } => {
            for u in iogp_editor::store::Users::load(&file)?.users { println!("{}\t{:?}", u.name, u.roles); }
        }
    }
    Ok(())
}

fn compile_all(c: &mut Compiler, text: &str) -> anyhow::Result<Vec<iogp_editor::compile::Rule>> {
    let mut out = vec![];
    for (id, s) in parse_lines(text) {
        match c.compile(&id, &s) {
            Ok(r) => out.push(r),
            Err(es) => anyhow::bail!("{id} is not a valid rule: {}", es[0].message),
        }
    }
    Ok(out)
}

fn print_review(rv: &iogp_editor::asp::Review) {
    println!("Example scenes (smallest scene in which the rule fires at the last frame):");
    for w in &rv.witnesses {
        println!("  {} — over {} s:", w.rule, w.seconds);
        for l in &w.lines { println!("      {l}"); }
    }
    if rv.dead.is_empty() { println!("Rules that can never fire: none"); } else { println!("Rules that can NEVER fire: {}", rv.dead.join(", ")); }
    if rv.implications.is_empty() { println!("Rules implied by other rules: none"); }
    for i in &rv.implications { println!("Whenever {} fires, {} fires too", i.a, i.b); }
    if rv.uncovered_lsr.is_empty() { println!("Every IOGP Life-Saving Rule has a rule"); }
    for (n, name) in &rv.uncovered_lsr { println!("No rule for IOGP Life-Saving Rule {n} ({name})"); }
    if !rv.unused_zones.is_empty() { println!("Named zones no rule covers: {}", rv.unused_zones.join(", ")); }
    println!("(bounded check: {})", rv.bound);
}
