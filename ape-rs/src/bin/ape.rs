//! `ape` command line — Rust replacement for SWI-APE's `ape.exe` / `ape.pl` CLI.
//!
//!   ape --text "Every man owns a car." --solo drspp
//!   ape --file input.ace --cdrspp --cfol --cparaphrase   (multi-mode XML, like ape.exe -cdrspp ...)
//!   ape serve --addr 127.0.0.1:8000

use ape_rs::{Ape, default_prolog_dir, server};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ape", about = "Attempto Parsing Engine on Rust + Scryer Prolog")]
struct Cli {
    /// Directory with the converted Prolog sources (default: $APE_PROLOG_DIR or ./prolog-scryer)
    #[arg(long, global = true)]
    prolog_dir: Option<PathBuf>,
    /// Per-request time limit in seconds (enforced as an inference budget inside Scryer)
    #[arg(long, global = true, default_value_t = 10)]
    time_limit: u32,
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// ACE text to parse
    #[arg(long)]
    text: Option<String>,
    /// File containing the ACE text
    #[arg(long)]
    file: Option<PathBuf>,
    /// Output a single result of this type (drs, drspp, fol, owlfss, paraphrase1, tptp, ...)
    #[arg(long)]
    solo: Option<String>,
    /// User lexicon file (ulex)
    #[arg(long)]
    ulexfile: Option<PathBuf>,
    /// Guess unknown words
    #[arg(long)]
    guess: bool,
    /// Do not load the common lexicon (Clex)
    #[arg(long)]
    noclex: bool,
    /// Multi-mode outputs, e.g. --c cdrspp --c cfol (any of the c* flags of SWI-APE)
    #[arg(long = "c", value_name = "FLAG")]
    multi: Vec<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the HTTP server (APE web service + OpenAI-compatible /v1/chat/completions)
    Serve {
        #[arg(long, default_value = "127.0.0.1:8000")]
        addr: String,
    },
}

fn main() {
    let cli = Cli::parse();
    let dir = cli.prolog_dir.clone().unwrap_or_else(default_prolog_dir);
    if let Some(Cmd::Serve { addr }) = &cli.cmd {
        let dir2 = dir.clone();
        let tl = cli.time_limit;
        let addr = addr.clone();
        let h = std::thread::Builder::new().stack_size(64 << 20).spawn(move || server::serve(dir2, &addr, tl).map_err(|e| e.to_string())).unwrap();
        if let Err(e) = h.join().unwrap() { eprintln!("{e}"); std::process::exit(1); }
        return;
    }
    // Scryer recursion is deep for big DRSs: run on a thread with a large stack.
    let code = std::thread::Builder::new().stack_size(256 << 20).spawn(move || run_cli(cli, dir)).unwrap().join().unwrap();
    std::process::exit(code);
}

fn run_cli(cli: Cli, dir: PathBuf) -> i32 {
    let text = match (&cli.text, &cli.file) {
        (Some(t), _) => t.clone(),
        (None, Some(f)) => match std::fs::read_to_string(f) { Ok(s) => s, Err(e) => { eprintln!("{e}"); return 2; } },
        (None, None) => { eprintln!("give --text or --file (or `ape serve`)"); return 2; }
    };
    let ulex = cli.ulexfile.as_ref().map(|f| std::fs::read_to_string(f).unwrap_or_default());
    let mut ape = match Ape::new(&dir) { Ok(a) => a.with_time_limit(cli.time_limit), Err(e) => { eprintln!("{e}"); return 1; } };
    let mut p: Vec<(&str, &str)> = vec![("text", text.as_str())];
    if let Some(s) = &cli.solo { p.push(("solo", s.as_str())); }
    if cli.guess { p.push(("guess", "on")); }
    if cli.noclex { p.push(("noclex", "on")); }
    if let Some(u) = &ulex { p.push(("ulextext", u.as_str())); }
    for m in &cli.multi { p.push((m.as_str(), "on")); }
    if cli.solo.is_none() && cli.multi.is_empty() { p.push(("solo", "drspp")); }
    match ape.get_results(&p) {
        Ok(r) => { println!("{}", r.content); 0 }
        Err(e) => { eprintln!("{e}"); 1 }
    }
}
