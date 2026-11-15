//! `acewiki-parity` — score acewiki-rs against the Java AceWiki oracle (oracle/Oracle.java).
//!
//!   acewiki-parity --data DIR --wiki geo --profile OWL2RL --oracle out/geo_rl.jsonl [--parser out/geo_parser.jsonl]
//!
//! Reports exact-agreement rates and MCC (Matthews correlation) for every binary decision:
//! sentence flags, entailed types / instances / sub- and super-classes, satisfiability,
//! question answers, and chart-parser completeness; plus next-token option set equality.

use acewiki_rs::chartparser::parser::canonical_vars;
use acewiki_rs::engine::{Config, Engine};
use acewiki_rs::model::{Kind, Statement};
use acewiki_rs::owl::profile::Profile;
use acewiki_rs::storage::Storage;
use clap::Parser;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

#[derive(Parser)]
struct Cli {
    #[arg(long)] data: PathBuf,
    #[arg(long)] wiki: String,
    #[arg(long, default_value = "OWL2RL")] profile: String,
    #[arg(long)] oracle: Option<PathBuf>,
    #[arg(long)] parser: Option<PathBuf>,
    #[arg(long, default_value = "../ape-rs/prolog-scryer")] prolog_dir: PathBuf,
    #[arg(long)] report: Option<PathBuf>,
    #[arg(long)] owllink: Option<String>,
    #[arg(long, default_value = "native")] reasoner: String,
    #[arg(long, default_value_t = 5)] show: usize,
}

#[derive(Default, Clone, Copy, Debug)]
struct Conf { tp: u64, tn: u64, fp: u64, fn_: u64 }
impl Conf {
    fn add(&mut self, r: bool, c: bool) { match (r, c) { (true, true) => self.tp += 1, (false, false) => self.tn += 1, (false, true) => self.fp += 1, (true, false) => self.fn_ += 1 } }
    fn mcc(&self) -> f64 {
        let (tp, tn, fp, fn_) = (self.tp as f64, self.tn as f64, self.fp as f64, self.fn_ as f64);
        let d = ((tp + fp) * (tp + fn_) * (tn + fp) * (tn + fn_)).sqrt();
        if d == 0.0 { if fp + fn_ == 0.0 { 1.0 } else { 0.0 } } else { (tp * tn - fp * fn_) / d }
    }
    fn j(&self) -> Value { json!({"tp": self.tp, "tn": self.tn, "fp": self.fp, "fn": self.fn_, "mcc": (self.mcc() * 1e6).round() / 1e6,
        "agreement": if self.tp + self.tn + self.fp + self.fn_ == 0 { 1.0 } else { (self.tp + self.tn) as f64 / (self.tp + self.tn + self.fp + self.fn_) as f64 }}) }
}

fn lines(p: &PathBuf) -> Vec<Value> {
    std::fs::read_to_string(p).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}
fn strs(v: &Value) -> BTreeSet<String> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() }

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    std::thread::Builder::new().stack_size(512 << 20).spawn(move || run(cli)).unwrap().join().unwrap()
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let t0 = std::time::Instant::now();
    let wiki = Storage::new(&cli.data).load(&cli.wiki)?;
    let ape = ape_rs::Ape::new(&cli.prolog_dir)?.with_time_limit(20);
    let cfg = Config { profile: Profile::parse(&cli.profile), owllink_url: cli.owllink.clone(), reasoner: cli.reasoner.clone(), ..Default::default() };
    let mut eng = Engine::new(wiki, cfg, ape)?;
    eng.set_cache_file(cli.data.join(format!("{}.ape-cache.json", cli.wiki)));
    eng.load_all()?;
    let t_load = t0.elapsed().as_secs_f64();
    let mut report = json!({"wiki": cli.wiki, "profile": cli.profile, "elements": eng.wiki.elements.len(), "load_seconds": t_load});

    let word = |eng: &Engine, id: u64| eng.wiki.elements[&id].word();
    if let Some(op) = &cli.oracle {
        let o = lines(op);
        let meta = o.iter().find(|x| x["kind"] == "meta").cloned().unwrap_or_default();
        // ---------- sentences
        let mut flags: BTreeMap<&str, Conf> = BTreeMap::new();
        let (mut text_eq, mut tok_eq, mut n_s) = (0, 0, 0);
        let mut mism: Vec<Value> = vec![];
        for s in o.iter().filter(|x| x["kind"] == "sentence") {
            let id = s["element"].as_u64().unwrap();
            let idx = s["idx"].as_u64().unwrap() as usize;
            let Some(e) = eng.wiki.elements.get(&id) else { continue };
            let sents: Vec<(String, bool)> = e.article.iter().filter_map(|st| match st { Statement::Sentence { serialized, integrated } => Some((serialized.clone(), *integrated)), _ => None }).collect();
            let Some((ser, integ)) = sents.get(idx).cloned() else { continue };
            n_s += 1;
            let a = eng.analyze(&ser)?;
            let tes = eng.tokenize(&ser)?;
            if a.text == s["text"].as_str().unwrap_or("") { text_eq += 1; } else if mism.len() < cli.show { mism.push(json!({"text_rs": a.text, "text_java": s["text"]})); }
            let orig: Vec<String> = eng.wiki.editor_elements(&tes).iter().map(|t| eng.wiki.original_text(t)).collect();
            if json!(orig) == s["orig_tokens"] { tok_eq += 1; } else if mism.len() < cli.show * 2 { mism.push(json!({"tokens_rs": orig, "tokens_java": s["orig_tokens"], "serialized": ser})); }
            for (k, v) in [("owl", a.is_owl), ("owlswrl", a.is_owlswrl), ("reasonable", a.reasonable), ("integrated", integ), ("question", Engine::is_question(&ser))] {
                let r = s[k].as_bool().unwrap_or(false);
                flags.entry(k).or_default().add(r, v);
                if r != v && mism.len() < cli.show * 3 { mism.push(json!({"flag": k, "java": r, "rs": v, "text": a.text, "violations": a.profile_violations})); }
            }
        }
        eng.flush_cache();
        report["sentences"] = json!({"n": n_s, "text_identical": text_eq, "parser_tokens_identical": tok_eq,
            "flags": flags.iter().map(|(k, c)| (k.to_string(), c.j())).collect::<serde_json::Map<_, _>>(), "examples": mism});
        // ---------- reasoning
        let t1 = std::time::Instant::now();
        let consistent = eng.is_consistent();
        let els: Vec<(u64, Kind)> = eng.wiki.elements.values().map(|e| (e.id, e.kind.clone())).collect();
        let inds: Vec<u64> = els.iter().filter(|x| x.1 == Kind::ProperName).map(|x| x.0).collect();
        let cons: Vec<u64> = els.iter().filter(|x| x.1 == Kind::Noun).map(|x| x.0).collect();
        let oe: HashMap<u64, &Value> = o.iter().filter(|x| x["kind"] == "element").map(|x| (x["id"].as_u64().unwrap(), x)).collect();
        let (mut types, mut insts, mut sup, mut sub, mut sat) = (Conf::default(), Conf::default(), Conf::default(), Conf::default(), Conf::default());
        let mut rex: Vec<Value> = vec![];
        let cword: Vec<(u64, String)> = cons.iter().map(|c| (*c, word(&eng, *c))).collect();
        let iword: Vec<(u64, String)> = inds.iter().map(|i| (*i, word(&eng, *i))).collect();
        for i in &inds {
            let Some(j) = oe.get(i) else { continue };
            let jr = strs(&j["concepts"]);
            let rs: BTreeSet<String> = eng.concepts_of(*i).into_iter().map(|x| word(&eng, x)).collect();
            for (_, w) in &cword { types.add(jr.contains(w), rs.contains(w)); }
            if jr != rs && rex.len() < cli.show { rex.push(json!({"types_of": word(&eng, *i), "java_only": jr.difference(&rs).collect::<Vec<_>>(), "rs_only": rs.difference(&jr).collect::<Vec<_>>()})); }
        }
        for c in &cons {
            let Some(j) = oe.get(c) else { continue };
            let ji = strs(&j["individuals"]);
            let ri: BTreeSet<String> = eng.individuals_of(*c).into_iter().map(|x| word(&eng, x)).collect();
            for (_, w) in &iword { insts.add(ji.contains(w), ri.contains(w)); }
            let (js, rsup) = (strs(&j["super"]), eng.super_concepts(*c).into_iter().map(|x| word(&eng, x)).collect::<BTreeSet<_>>());
            let (jb, rsub) = (strs(&j["sub"]), eng.sub_concepts(*c).into_iter().map(|x| word(&eng, x)).collect::<BTreeSet<_>>());
            for (d, w) in &cword { if d == c { continue; } sup.add(js.contains(w), rsup.contains(w)); sub.add(jb.contains(w), rsub.contains(w)); }
            sat.add(j["satisfiable"].as_bool().unwrap_or(true), eng.is_satisfiable(*c));
            if (js != rsup || jb != rsub) && rex.len() < cli.show * 2 { rex.push(json!({"concept": word(&eng, *c), "super_java_only": js.difference(&rsup).collect::<Vec<_>>(), "super_rs_only": rsup.difference(&js).collect::<Vec<_>>(), "sub_java_only": jb.difference(&rsub).collect::<Vec<_>>(), "sub_rs_only": rsub.difference(&jb).collect::<Vec<_>>()})); }
        }
        let mut ans = Conf::default();
        let (mut q_exact, mut q_n) = (0, 0);
        let universe: Vec<String> = iword.iter().chain(cword.iter()).map(|x| x.1.clone()).collect();
        for s in o.iter().filter(|x| x["kind"] == "sentence" && x["question"] == true) {
            let id = s["element"].as_u64().unwrap(); let idx = s["idx"].as_u64().unwrap() as usize;
            let e = &eng.wiki.elements[&id];
            let sents: Vec<String> = e.article.iter().filter_map(|st| match st { Statement::Sentence { serialized, .. } => Some(serialized.clone()), _ => None }).collect();
            let Some(ser) = sents.get(idx).cloned() else { continue };
            let ja = if s["answer"].is_null() { BTreeSet::new() } else { strs(&s["answer"]) };
            let ra: BTreeSet<String> = eng.answer(&ser)?.unwrap_or_default().into_iter().map(|x| word(&eng, x)).collect();
            q_n += 1; if ja == ra { q_exact += 1; } else if rex.len() < cli.show * 3 { rex.push(json!({"question": s["text"], "java_only": ja.difference(&ra).collect::<Vec<_>>(), "rs_only": ra.difference(&ja).collect::<Vec<_>>()})); }
            for w in &universe { ans.add(ja.contains(w), ra.contains(w)); }
        }
        eng.flush_cache();
        report["reasoning"] = json!({
            "reasoner": eng.reasoner_note, "seconds": t1.elapsed().as_secs_f64(),
            "consistent": {"java": meta["consistent"], "rs": consistent},
            "types": types.j(), "instances": insts.j(), "super": sup.j(), "sub": sub.j(), "satisfiable": sat.j(),
            "answers": ans.j(), "questions_exact": format!("{q_exact}/{q_n}"), "examples": rex,
        });
    }
    // ---------- predictive parser
    if let Some(pp) = &cli.parser {
        let o = lines(pp);
        let t2 = std::time::Instant::now();
        let (mut n, mut abs_eq, mut con_eq, mut cnt_eq, mut ref_eq) = (0, 0, 0, 0, 0);
        let mut complete = Conf::default();
        let mut ex: Vec<Value> = vec![];
        let mut by_sent: BTreeMap<(u64, u64), Vec<&Value>> = BTreeMap::new();
        for x in o.iter().filter(|x| x["kind"] == "options") { by_sent.entry((x["element"].as_u64().unwrap(), x["idx"].as_u64().unwrap())).or_default().push(x); }
        for ((id, idx), recs) in by_sent {
            let e = &eng.wiki.elements[&id];
            let sents: Vec<String> = e.article.iter().filter_map(|st| match st { Statement::Sentence { serialized, .. } => Some(serialized.clone()), _ => None }).collect();
            let Some(ser) = sents.get(idx as usize) else { continue };
            let tes = eng.tokenize(ser)?;
            let toks: Vec<String> = eng.wiki.editor_elements(&tes).iter().map(|t| eng.wiki.original_text(t)).collect();
            let mut p = eng.parser();
            for r in recs {
                let k = r["k"].as_u64().unwrap() as usize;
                while p.tokens().len() < k { let t = toks[p.tokens().len()].clone(); p.add_token(&t); }
                n += 1;
                let mut ra: Vec<String> = p.abstract_option_strings(); ra.sort(); ra.dedup();
                let mut ja: Vec<String> = r["abstract"].as_array().unwrap().iter().map(|x| canonical_vars(x.as_str().unwrap())).collect(); ja.sort(); ja.dedup();
                if ra == ja { abs_eq += 1; } else if ex.len() < cli.show { ex.push(json!({"k": k, "tokens": &toks[..k], "java_only": ja.iter().filter(|x| !ra.contains(x)).collect::<Vec<_>>(), "rs_only": ra.iter().filter(|x| !ja.contains(x)).collect::<Vec<_>>()})); }
                let mut con: Vec<String> = p.next_token_options().con.iter().map(|c| format!("{}|{}", c.word, c.category.as_ref().map(|x| x.name().to_string()).unwrap_or_else(|| "null".into()))).collect();
                con.sort(); con.dedup();
                let mut h = Sha256::new(); for c in &con { h.update(c.as_bytes()); h.update(b"\n"); }
                let sha = format!("{:x}", h.finalize());
                if sha == r["concrete_sha"].as_str().unwrap_or("") { con_eq += 1; }
                if con.len() as u64 == r["n_concrete"].as_u64().unwrap_or(0) { cnt_eq += 1; }
                complete.add(r["complete"].as_bool().unwrap_or(false), p.is_complete());
                if p.reference() == r["ref"].as_i64().unwrap_or(-1) { ref_eq += 1; }
            }
        }
        report["parser"] = json!({"prefixes": n, "abstract_options_identical": abs_eq, "concrete_options_identical": con_eq,
            "concrete_count_identical": cnt_eq, "reference_identical": ref_eq, "complete": complete.j(), "seconds": t2.elapsed().as_secs_f64(), "examples": ex});
    }
    let s = serde_json::to_string_pretty(&report)?;
    if let Some(r) = &cli.report { std::fs::write(r, &s)?; }
    // compact summary: every binary decision with its MCC, then the identity counters
    fn walk(v: &Value, path: &str, out: &mut Vec<String>) {
        if let Some(m) = v.as_object() {
            if m.contains_key("mcc") {
                out.push(format!("  {path:<32} mcc={:<8} agreement={:.5}  tp={} tn={} fp={} fn={}", m["mcc"], m["agreement"].as_f64().unwrap_or(0.0), m["tp"], m["tn"], m["fp"], m["fn"]));
                return;
            }
            for (k, x) in m { if k != "examples" { walk(x, &if path.is_empty() { k.clone() } else { format!("{path}.{k}") }, out); } }
        }
    }
    let mut lines = vec![];
    walk(&report, "", &mut lines);
    println!("{} / {}: {} elements, loaded in {:.1}s", cli.wiki, cli.profile, report["elements"], report["load_seconds"].as_f64().unwrap_or(0.0));
    for l in lines { println!("{l}"); }
    let g = |a: &str, b: &str| report[a][b].as_u64().map(|x| x.to_string()).unwrap_or_else(|| "-".into());
    if report.get("sentences").is_some() {
        println!("  sentences: {}  text identical: {}  editor tokens identical: {}", g("sentences", "n"), g("sentences", "text_identical"), g("sentences", "parser_tokens_identical"));
    }
    if report.get("parser").is_some() {
        println!("  parser prefixes: {}  abstract options identical: {}  concrete options identical: {}  reference identical: {}",
            g("parser", "prefixes"), g("parser", "abstract_options_identical"), g("parser", "concrete_options_identical"), g("parser", "reference_identical"));
    }
    if cli.report.is_none() { println!("{s}"); }
    Ok(())
}
