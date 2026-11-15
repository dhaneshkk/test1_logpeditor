//! `ape-parity` — Rust replacement for every Python helper of the migration
//! (`tools/score.py`, the fixture generator, the import-cycle checker, the notebook driver logic).
//!
//!   ape-parity suite     --swi harness/swi_results.txt  [--shards N]   # DRS parity + MCC
//!   ape-parity outmodes  --swi harness/om_swi.txt       [--step 10]    # 22 output modes, exact/normalised
//!   ape-parity fixtures  --swi harness/om_swi.txt --out tests/fixtures_swi.json
//!   ape-parity cycles                                                  # cyclic use_module edges
//!
//! The Scryer side runs in-process (one embedded machine per shard thread, `harness/parity.pl`),
//! so the separate `scryer-prolog` binary and `run_scryer.pl` are no longer needed.

use ape_rs::{Ape, term_text};
use clap::{Parser, Subcommand};
use regex::Regex;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(name = "ape-parity", about = "Differential SWI-APE vs ape-rs harness with MCC scoring")]
struct Cli {
    /// ape-rs checkout root (contains prolog-scryer/ and harness/)
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Full regression suite: DRS variant-equality vs SWI and acceptance MCC
    Suite {
        #[arg(long, default_value = "harness/swi_results.txt")]
        swi: PathBuf,
        #[arg(long, default_value = "harness/tests_canon.pl")]
        tests: PathBuf,
        #[arg(long)]
        shards: Option<usize>,
        #[arg(long, default_value = "harness/parity_report.json")]
        report: PathBuf,
    },
    /// Output-mode parity on the SWI sample (exact and normalised)
    Outmodes {
        #[arg(long, default_value = "harness/om_swi.txt")]
        swi: PathBuf,
        #[arg(long, default_value = "harness/tests_canon.pl")]
        tests: PathBuf,
        #[arg(long)]
        shards: Option<usize>,
        /// print the first normalised mismatch per mode
        #[arg(long)]
        show: bool,
    },
    /// Regenerate tests/fixtures_swi.json (golden SWI outputs for `cargo test`)
    Fixtures {
        #[arg(long, default_value = "harness/om_swi.txt")]
        swi: PathBuf,
        #[arg(long, default_value = "harness/tests_canon.pl")]
        tests: PathBuf,
        #[arg(long, default_value = "tests/fixtures_swi.json")]
        out: PathBuf,
        #[arg(long, default_value_t = 40)]
        texts: usize,
    },
    /// Detect cyclic use_module edges in the converted tree (Scryer cannot load them)
    Cycles,
}

fn main() {
    let cli = Cli::parse();
    let root = cli.root.canonicalize().expect("root");
    // Scryer recursion is deep on big DRSs: do all work on a large-stack thread.
    let code = std::thread::Builder::new().stack_size(512 << 20)
        .spawn(move || match cli.cmd {
            Cmd::Suite { swi, tests, shards, report } => suite(&root, &root.join(swi), &root.join(tests), shards, &root.join(report)),
            Cmd::Outmodes { swi, tests, shards, show } => outmodes(&root, &root.join(swi), &root.join(tests), shards, show),
            Cmd::Fixtures { swi, tests, out, texts } => fixtures(&root, &root.join(swi), &root.join(tests), &root.join(out), texts),
            Cmd::Cycles => cycles(&root.join("prolog-scryer")),
        })
        .unwrap().join().unwrap();
    std::process::exit(code);
}

// ------------------------------------------------------------------------------------------ setup

const OPS: &str = ":- op(400, fx, -). :- op(400, fx, ~). :- op(500, xfx, =>). :- op(500, xfx, v).\n";

/// Wrap a file of r/4 (or t/3) facts into a Scryer module so it can be use_module'd.
fn wrap_module(src: &Path, module: &str, exports: &str, dst: &Path) {
    let body = std::fs::read_to_string(src).unwrap_or_else(|e| panic!("{}: {e}", src.display()));
    std::fs::write(dst, format!(":- module({module}, [{exports}]).\n{OPS}{body}")).unwrap();
}

/// One engine per shard: converted APE + parity helpers + the given data modules.
fn engine(root: &Path, mods: &[&Path]) -> Ape {
    let mut ape = Ape::new(root.join("prolog-scryer")).expect("load APE");
    let mut goal = format!("use_module({})", ape_rs::quote_atom(&root.join("harness/parity").to_string_lossy()));
    for m in mods {
        goal.push_str(&format!(", use_module({})", ape_rs::quote_atom(&m.with_extension("").to_string_lossy())));
    }
    ape.query(&format!("{goal}.")).expect("load harness").expect("harness loaded");
    ape
}

fn shard_count(s: Option<usize>) -> usize {
    s.unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)).max(1)
}

fn int(t: &scryer_prolog::Term) -> i64 { term_text(t).parse().unwrap_or(-1) }

fn test_numbers(ape: &mut Ape) -> Vec<i64> {
    let b = ape.query("findall(N, tests_canon:t(N,_,_), Ns).").unwrap().unwrap();
    match &b["Ns"] { scryer_prolog::Term::List(xs) => xs.iter().map(int).collect(), _ => vec![] }
}

/// Run `f` over `items` split round-robin across `shards` threads, each with its own engine.
fn sharded<T: Send + 'static, R: Send + 'static>(
    root: &Path, mods: Vec<PathBuf>, items: Vec<T>, shards: usize,
    f: fn(&mut Ape, &T) -> R,
) -> Vec<R> where T: Clone {
    let mut buckets: Vec<Vec<(usize, T)>> = (0..shards).map(|_| vec![]).collect();
    for (i, it) in items.into_iter().enumerate() { buckets[i % shards].push((i, it)); }
    let handles: Vec<_> = buckets.into_iter().map(|b| {
        let root = root.to_path_buf(); let mods = mods.clone();
        std::thread::Builder::new().stack_size(512 << 20).spawn(move || {
            let refs: Vec<&Path> = mods.iter().map(|p| p.as_path()).collect();
            let mut ape = engine(&root, &refs);
            b.into_iter().map(|(i, t)| (i, f(&mut ape, &t))).collect::<Vec<_>>()
        }).unwrap()
    }).collect();
    let mut all: Vec<(usize, R)> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
    all.sort_by_key(|(i, _)| *i);
    all.into_iter().map(|(_, r)| r).collect()
}

// ------------------------------------------------------------------------------------------- MCC

#[derive(Default, Debug, Clone, Copy)]
struct Conf { tp: u64, tn: u64, fp: u64, fn_: u64 }
impl Conf {
    fn add(&mut self, reference: bool, candidate: bool) {
        match (reference, candidate) { (true, true) => self.tp += 1, (false, false) => self.tn += 1,
            (false, true) => self.fp += 1, (true, false) => self.fn_ += 1 }
    }
    /// Matthews correlation coefficient (0 when a marginal is empty). Lean: formal/ApeMigration.lean §4.
    fn mcc(&self) -> f64 {
        let (tp, tn, fp, fn_) = (self.tp as f64, self.tn as f64, self.fp as f64, self.fn_ as f64);
        let d = ((tp + fp) * (tp + fn_) * (tn + fp) * (tn + fn_)).sqrt();
        if d == 0.0 { 0.0 } else { (tp * tn - fp * fn_) / d }
    }
    fn json(&self) -> serde_json::Value {
        json!({"tp": self.tp, "tn": self.tn, "fp": self.fp, "fn": self.fn_, "mcc": (self.mcc() * 1e6).round() / 1e6})
    }
}

// ----------------------------------------------------------------------------------------- suite

fn suite(root: &Path, swi: &Path, tests: &Path, shards: Option<usize>, report: &Path) -> i32 {
    let tmp = std::env::temp_dir();
    let (tm, sm) = (tmp.join("ape_tests_canon.pl"), tmp.join("ape_swi_ref.pl"));
    wrap_module(tests, "tests_canon", "t/3", &tm);
    wrap_module(swi, "swi_ref", "r/4", &sm);
    let n = shard_count(shards);
    let t0 = Instant::now();
    let ns = test_numbers(&mut engine(root, &[&tm]));
    eprintln!("{} tests, {n} shard(s)", ns.len());
    let rows = sharded(root, vec![tm, sm], ns, n, |ape, &num| {
        let b = ape.query(&format!("parity:suite_check({num}, G, R, C, S).")).ok().flatten();
        b.map(|b| (num, term_text(&b["G"]), term_text(&b["R"]), term_text(&b["C"]), int(&b["S"])))
    });
    let rows: Vec<_> = rows.into_iter().flatten().collect();
    let valid: Vec<_> = rows.iter().filter(|r| r.2 != "x").collect();
    let (mut scr_swi, mut swi_gold, mut scr_gold) = (Conf::default(), Conf::default(), Conf::default());
    for (_, g, r, c, _) in &valid {
        scr_swi.add(r == "1", c == "1"); swi_gold.add(g == "1", r == "1"); scr_gold.add(g == "1", c == "1");
    }
    let identical = valid.iter().filter(|r| r.4 == 1).count();
    let missing: Vec<_> = rows.iter().filter(|r| r.2 == "x").map(|r| r.0).collect();
    let out = json!({
        "tests_total": rows.len(), "tests_compared": valid.len(), "identical_drs": identical,
        "acceptance_scryer_vs_swi": scr_swi.json(), "acceptance_swi_vs_gold": swi_gold.json(),
        "acceptance_scryer_vs_gold": scr_gold.json(), "missing_in_swi_run": missing,
        "wall_seconds": t0.elapsed().as_secs_f64(),
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
    std::fs::write(report, serde_json::to_string_pretty(&out).unwrap()).ok();
    if identical == valid.len() && scr_swi.fp + scr_swi.fn_ == 0 { 0 } else { 1 }
}

// -------------------------------------------------------------------------------------- outmodes

/// (n, type, swi content type, swi content)
fn load_om(root: &Path, swi: &Path, tests: &Path) -> (Vec<(i64, String, String, String)>, PathBuf, PathBuf) {
    let tmp = std::env::temp_dir();
    let (tm, om) = (tmp.join("ape_tests_canon.pl"), tmp.join("ape_swi_om.pl"));
    wrap_module(tests, "tests_canon", "t/3", &tm);
    wrap_module(swi, "swi_om", "r/4", &om);
    let mut ape = engine(root, &[&tm, &om]);
    let b = ape.query("findall(r(N,T,CT,A), parity:om_ref(N,T,CT,A), Rs).").unwrap().unwrap();
    let rows = match &b["Rs"] {
        scryer_prolog::Term::List(xs) => xs.iter().filter_map(|r| match r {
            scryer_prolog::Term::Compound(_, a) if a.len() == 4 =>
                Some((int(&a[0]), term_text(&a[1]), term_text(&a[2]), term_text(&a[3]))),
            _ => None }).collect(),
        _ => vec![],
    };
    (rows, tm, om)
}

fn run_mode(ape: &mut Ape, job: &(i64, String)) -> (String, String) {
    let (num, ty) = job;
    match ape.query(&format!("parity:om_run({num}, {ty}, CT, C).")) {
        Ok(Some(b)) => (term_text(&b["CT"]), term_text(&b["C"])),
        Ok(None) => ("err".into(), "failed".into()),
        Err(e) => ("err".into(), e.to_string()),
    }
}

/// Normalise output that is nondeterministic or system-specific even within SWI-APE:
/// random OWL node ids, SWI's raw variable names (_123, _G123) vs Scryer's (A, _A),
/// and error-context wording (compare only the formal part of error(Formal, Context)).
struct Norm { ids: Regex, swivar: Regex, letter: Regex, qual: Regex }
impl Norm {
    fn new() -> Self { Norm {
        ids: Regex::new(r"\d{12,}").unwrap(),
        swivar: Regex::new(r"\b_G?\d+\b").unwrap(),
        letter: Regex::new(r"(^|[^\w'])_?[A-Z]\d*([^\w('])").unwrap(),
        qual: Regex::new(r"\b\w+:(\w+/\d)").unwrap(),
    }}
    fn formal(s: &str) -> Option<String> {
        let rest = s.strip_prefix("error(")?;
        let mut depth = 0i32;
        for (i, ch) in rest.char_indices() {
            match ch { '(' | '[' | '{' => depth += 1, ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => return Some(rest[..i].to_string()), _ => {} }
        }
        Some(rest.to_string())
    }
    fn apply(&self, s: &str) -> String {
        if let Some(f) = Self::formal(s) {
            return format!("ERR:{}", self.qual.replace_all(&f.replace("'[|]'", "'.'").replace("[|]", "."), "$1"));
        }
        let s = self.ids.replace_all(s, "#");
        let s = self.swivar.replace_all(&s, "V");
        // twice: adjacent variables share a delimiter
        let s = self.letter.replace_all(&s, "${1}V$2");
        self.letter.replace_all(&s, "${1}V$2").into_owned()
    }
}

fn outmodes(root: &Path, swi: &Path, tests: &Path, shards: Option<usize>, show: bool) -> i32 {
    let (rows, tm, om) = load_om(root, swi, tests);
    let jobs: Vec<(i64, String)> = rows.iter().map(|r| (r.0, r.1.clone())).collect();
    let t0 = Instant::now();
    let got = sharded(root, vec![tm, om], jobs, shard_count(shards), run_mode);
    let norm = Norm::new();
    let mut stats: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    let mut shown = BTreeSet::new();
    for (r, (ct, c)) in rows.iter().zip(got.iter()) {
        let e = stats.entry(r.1.clone()).or_default();
        e.0 += 1;
        if r.3 == *c && r.2 == *ct { e.1 += 1; }
        if norm.apply(&r.3) == norm.apply(c) { e.2 += 1; }
        else if show && shown.insert(r.1.clone()) {
            println!("== {} test {}\nSWI: {:?}\nRS:  {:?}", r.1, r.0, r.3, c);
        }
    }
    println!("{:<12} {:>12} {:>12}", "mode", "exact", "normalised");
    let (mut n, mut ex, mut no) = (0, 0, 0);
    for (m, (a, b, c)) in &stats {
        println!("{m:<12} {:>12} {:>12}", format!("{b}/{a}"), format!("{c}/{a}"));
        n += a; ex += b; no += c;
    }
    println!("{:<12} {:>12} {:>12}   ({:.1}s)", "TOTAL", format!("{ex}/{n}"), format!("{no}/{n}"), t0.elapsed().as_secs_f64());
    if no == n { 0 } else { 1 }
}

// -------------------------------------------------------------------------------------- fixtures

const FIXTURE_MODES: &[&str] = &["drs", "drspp", "drsxml", "fol", "pnf", "paraphrase", "paraphrase1",
    "paraphrase2", "tokens", "sentences", "syntaxpp", "tptp", "drshtml"];

fn fixtures(root: &Path, swi: &Path, tests: &Path, out: &Path, texts: usize) -> i32 {
    let (rows, tm, om) = load_om(root, swi, tests);
    let mut ape = engine(root, &[&tm, &om]);
    let ns: BTreeSet<i64> = rows.iter().map(|r| r.0).collect();
    let keep: BTreeSet<i64> = ns.into_iter().take(texts).collect();
    let fx: Vec<_> = rows.iter()
        .filter(|r| keep.contains(&r.0) && FIXTURE_MODES.contains(&r.1.as_str()) && r.2 != "err")
        .map(|r| {
            let text = term_text(&ape.query(&format!("parity:test_text({}, A).", r.0)).unwrap().unwrap()["A"]);
            json!({"n": r.0, "text": text, "mode": r.1, "ct": r.2, "c": r.3})
        }).collect();
    std::fs::write(out, serde_json::to_string_pretty(&fx).unwrap()).unwrap();
    println!("{} fixtures -> {}", fx.len(), out.display());
    0
}

// ---------------------------------------------------------------------------------------- cycles

fn cycles(dir: &Path) -> i32 {
    let re = Regex::new(r#":- use_module\((?:'([^']+)'|([a-z_]+))[,)]"#).unwrap();
    let mut g: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() { stack.push(p); continue; }
            if p.extension().is_some_and(|x| x == "pl") {
                let src = std::fs::read_to_string(&p).unwrap_or_default();
                let base = p.parent().unwrap();
                let deps = re.captures_iter(&src).filter_map(|c| {
                    let t = c.get(1).or(c.get(2))?.as_str();
                    (!t.starts_with("library")).then(|| normalize(&base.join(t).with_extension("pl")))
                }).collect();
                g.insert(normalize(&p), deps);
            }
        }
    }
    let mut found = BTreeSet::new();
    for start in g.keys() { dfs(&g, start, &mut vec![start.clone()], &mut found); }
    for c in &found {
        println!("{}", c.iter().map(|p| p.strip_prefix(dir).unwrap_or(p).display().to_string()).collect::<Vec<_>>().join(" -> "));
    }
    println!("{} cycle(s)", found.len());
    if found.is_empty() { 0 } else { 1 }
}

fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() { match c { std::path::Component::ParentDir => { out.pop(); } std::path::Component::CurDir => {} c => out.push(c) } }
    out
}

fn dfs(g: &BTreeMap<PathBuf, Vec<PathBuf>>, n: &PathBuf, path: &mut Vec<PathBuf>, found: &mut BTreeSet<Vec<PathBuf>>) {
    for x in g.get(n).into_iter().flatten() {
        if let Some(i) = path.iter().position(|p| p == x) {
            let mut cyc = path[i..].to_vec();
            let k = cyc.iter().enumerate().min_by_key(|(_, p)| (*p).clone()).map(|(k, _)| k).unwrap();
            cyc.rotate_left(k);
            found.insert(cyc);
        } else if path.len() < 64 {
            path.push(x.clone()); dfs(g, x, path, found); path.pop();
        }
    }
}
