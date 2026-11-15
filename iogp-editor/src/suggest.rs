//! Auto-suggestion for typing rules: given the text so far (cursor at the end), say which words
//! may come next, filtered by the partly typed word, ranked by an n-gram model learned from the
//! templates and the rules people have written, plus a "ghost" completion (the confident
//! continuation of several words, accepted with Tab) and spelling fixes for a mistyped word.
//!
//! Every suggestion is checked against the predictive grammar, so accepting one can never take
//! the sentence somewhere the compiler will refuse for grammatical reasons.

use crate::catalog::Catalog;
use crate::grammar::{self, RuleGrammar};
use acewiki_rs::chartparser::parser::ChartParser;
use serde::Serialize;
use std::collections::HashMap;

/// Display order of word groups (also used by the menus).
pub const GROUPS: &[&str] = &["function word", "detected object", "zone", "named zone", "camera", "site signal", "position", "observed relation", "number", "time unit", "severity", "alert (IOGP Life-Saving Rule)", "noun"];

#[derive(Clone, Debug, Serialize)]
pub struct Option_ { pub w: String, pub g: &'static str, #[serde(skip_serializing_if = "Option::is_none")] pub fix: Option<String> }

#[derive(Clone, Debug, Serialize)]
pub struct Suggestion {
    /// grammar tokens accepted so far
    pub tokens: Vec<String>,
    /// character offset where the word being typed (or the first unreadable word) starts
    pub partial_start: usize,
    pub partial: String,
    /// end of the first unreadable word (a fix replaces only `partial_start..bad_end`)
    pub bad_end: usize,
    /// the top option is clearly the most likely (only then is a ghost continuation offered
    /// without a typed prefix)
    pub confident: bool,
    /// next-word options, best first, filtered by `partial`
    pub options: Vec<Option_>,
    /// every next word grouped (for the browse menus), unfiltered
    pub groups: Vec<(&'static str, Vec<String>)>,
    /// confident continuation after the best option (text to append after accepting it)
    pub ghost: String,
    pub complete: bool,
    /// the text cannot continue: what is wrong (the partial word fits nothing)
    #[serde(skip_serializing_if = "Option::is_none")] pub error: Option<String>,
}

/// Token n-gram counts learned from accepted rule sentences.
#[derive(Default, Clone)]
pub struct Ngram { uni: HashMap<String, u32>, bi: HashMap<(String, String), u32>, tri: HashMap<(String, String, String), u32> }

impl Ngram {
    pub fn learn(&mut self, tokens: &[String]) {
        let mut seq = vec!["<s>".to_string(), "<s>".to_string()];
        seq.extend(tokens.iter().map(|t| t.to_lowercase()));
        for i in 2..seq.len() {
            *self.uni.entry(seq[i].clone()).or_default() += 1;
            *self.bi.entry((seq[i - 1].clone(), seq[i].clone())).or_default() += 1;
            *self.tri.entry((seq[i - 2].clone(), seq[i - 1].clone(), seq[i].clone())).or_default() += 1;
        }
    }
    fn score(&self, p2: &str, p1: &str, w: &str) -> f64 {
        let w = w.to_lowercase();
        let t = *self.tri.get(&(p2.into(), p1.into(), w.clone())).unwrap_or(&0) as f64;
        let b = *self.bi.get(&(p1.into(), w.clone())).unwrap_or(&0) as f64;
        let u = *self.uni.get(&w).unwrap_or(&0) as f64;
        100.0 * t + 10.0 * b + u
    }
}

/// Learn from sentences (templates, approved rules, valid attempts) — only those the grammar accepts.
pub fn train(g: &RuleGrammar, sentences: impl IntoIterator<Item = String>) -> Ngram {
    let mut m = Ngram::default();
    for s in sentences {
        let mut p = g.parser();
        if let Ok(toks) = acewiki_rs::web::feed_text(&mut p, &s) { if p.is_complete() { m.learn(&toks); } }
    }
    m
}

struct Word { start: usize, text: String }

fn words(text: &str) -> Vec<Word> {
    let re = regex::Regex::new(r"[A-Za-z0-9_\-']+|[.,?!:;]").unwrap();
    re.find_iter(text).map(|m| Word { start: m.start(), text: m.as_str().to_string() }).collect()
}

fn lower_first(s: &str) -> String { let mut c = s.chars(); c.next().map(|f| f.to_lowercase().collect::<String>() + c.as_str()).unwrap_or_default() }

fn lev(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut prev = d[0]; d[0] = i;
        for j in 1..=b.len() { let t = d[j]; d[j] = (d[j] + 1).min(d[j - 1] + 1).min(prev + usize::from(a[i - 1] != b[j - 1])); prev = t; }
    }
    d[b.len()]
}

/// Greedy longest-match of the words onto grammar tokens (multi-word terms like "at least",
/// "does not"). Returns (tokens, index of the first word not consumed).
fn consume(p: &mut ChartParser, ws: &[Word]) -> (Vec<String>, usize) {
    let mut out = vec![];
    let mut i = 0;
    while i < ws.len() {
        let mut done = false;
        for k in (1..=4.min(ws.len() - i)).rev() {
            let seg: Vec<&str> = ws[i..i + k].iter().map(|w| w.text.as_str()).collect();
            if k > 1 && seg.iter().any(|w| w.len() == 1 && ".,?!:;".contains(*w)) { continue; }
            let j = seg.join(" ");
            if let Some(c) = [j.clone(), lower_first(&j)].into_iter().find(|c| p.is_possible_next_token(c)) {
                p.add_token(&c); out.push(c); i += k; done = true; break;
            }
        }
        if !done { break; }
    }
    (out, i)
}

pub fn suggest(g: &RuleGrammar, cat: &Catalog, model: &Ngram, text: &str) -> Suggestion {
    let ws = words(text);
    let trailing_space = text.ends_with(|c: char| c.is_whitespace());
    let mut p = g.parser();
    let (tokens, used) = consume(&mut p, &ws);
    // the unconsumed tail: the word being typed (possibly the start of a multi-word term, "does no")
    let tail: Vec<&Word> = ws[used..].iter().collect();
    let partial_start = tail.first().map(|w| w.start).unwrap_or(text.len());
    let partial = if tail.is_empty() { String::new() } else { text[partial_start..].trim_end().to_string() };
    let complete = p.is_complete() && tail.is_empty() && !tokens.is_empty();

    let opts = p.next_token_options().clone();
    let mut all: Vec<(String, &'static str)> = vec![];
    for c in &opts.con {
        if all.iter().any(|(w, _)| w == &c.word) { continue; }
        all.push((c.word.clone(), grammar::menu_group(c.category.as_ref().map(|x| x.name()), &c.word, cat)));
    }
    let at_start = tokens.is_empty();
    let shown = |w: &str| if at_start { let mut c = w.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() } else { w.to_string() };
    let ctx: Vec<String> = { let mut v = vec!["<s>".to_string(), "<s>".to_string()]; v.extend(tokens.iter().map(|t| t.to_lowercase())); v };
    let (p2, p1) = (ctx[ctx.len() - 2].clone(), ctx[ctx.len() - 1].clone());
    let rank = |w: &str, g: &str| model.score(&p2, &p1, w) - GROUPS.iter().position(|x| *x == g).unwrap_or(99) as f64 * 1e-3;

    let pl = partial.to_lowercase();
    // a multi-word partial must be followed by a space only inside a term ("does " → "does not")
    let mut matched: Vec<Option_> = all.iter()
        .filter(|(w, _)| pl.is_empty() || (w.to_lowercase().starts_with(&pl) && !(trailing_space && w.to_lowercase() == pl)))
        .map(|(w, g)| Option_ { w: shown(w), g, fix: None }).collect();
    matched.sort_by(|a, b| rank(&b.w, b.g).partial_cmp(&rank(&a.w, a.g)).unwrap().then(a.w.cmp(&b.w)));

    let mut error = None;
    if matched.is_empty() && !pl.is_empty() {
        // spelling fixes: options close to what was typed (compare with the same number of characters)
        let bad = tail.first().map(|w| w.text.to_lowercase()).unwrap_or_default();
        let pl = if tail.len() > 1 { bad.clone() } else { pl.clone() };
        let mut fixes: Vec<(usize, Option_)> = all.iter().filter_map(|(w, g)| {
            let wl = w.to_lowercase();
            let cut: String = wl.chars().take(pl.chars().count().max(3)).collect();
            let d = lev(&pl, &wl).min(lev(&pl, &cut) + usize::from(wl.len() > pl.len() && !trailing_space));
            let lim = if pl.chars().count() <= 4 { 1 } else { 2 };
            (d <= lim).then(|| (d, Option_ { w: shown(w), g, fix: tail.first().map(|x| x.text.clone()) }))
        }).collect();
        fixes.sort_by(|a, b| a.0.cmp(&b.0).then(rank(&b.1.w, b.1.g).partial_cmp(&rank(&a.1.w, a.1.g)).unwrap()));
        let first = tail.first().map(|w| w.text.clone()).unwrap_or_default();
        let after = if tokens.is_empty() { "at the start".to_string() } else { format!("after \"{}\"", tokens.last().unwrap()) };
        error = Some(if all.is_empty() {
            format!("the sentence is already complete — remove \"{}\"", text[partial_start..].trim())
        } else if fixes.is_empty() {
            let n = all.len();
            let sample: Vec<String> = { let mut v = all.clone(); v.sort_by(|a, b| rank(&b.0, b.1).partial_cmp(&rank(&a.0, a.1)).unwrap()); v.into_iter().take(6).map(|x| x.0).collect() };
            format!("\"{first}\" cannot come {after}. Try: {}{}", sample.join(", "), if n > 6 { format!(" … ({n} options)") } else { String::new() })
        } else {
            format!("\"{first}\" is not a word {} knows here — did you mean {}?", cat.agent, fixes.iter().take(3).map(|f| format!("\"{}\"", f.1.w)).collect::<Vec<_>>().join(", "))
        });
        matched = if fixes.is_empty() {
            // nothing close: offer every word that fits here as a replacement for the bad one
            let bad_word = tail.first().map(|x| x.text.clone());
            let mut v: Vec<Option_> = all.iter().map(|(w, g)| Option_ { w: shown(w), g, fix: bad_word.clone() }).collect();
            v.sort_by(|a, b| rank(&b.w, b.g).partial_cmp(&rank(&a.w, a.g)).unwrap().then(a.w.cmp(&b.w)));
            v
        } else { fixes.into_iter().map(|x| x.1).collect() };
    }
    matched.truncate(40);
    let bad_end = if error.is_some() { tail.first().map(|w| w.start + w.text.len()).unwrap_or(text.len()) } else { text.len() };
    let confident = error.is_none() && match matched.as_slice() {
        [] => false,
        [_] => true,
        [a, b, ..] => { let (x, y) = (rank(&a.w, a.g), rank(&b.w, b.g)); x >= 20.0 && x >= 2.0 * y.max(0.0) }
    };

    // ghost: after the best option, keep adding the next word while it is the only one or clearly the most likely
    let mut ghost = String::new();
    if let Some(best) = matched.first().filter(|_| error.is_none() && (confident || !partial.is_empty())) {
        let mut q = p.clone();
        let tok = if at_start { lower_first(&best.w) } else { best.w.clone() };
        if q.is_possible_next_token(&tok) {
            q.add_token(&tok);
            let mut hist = ctx.clone(); hist.push(tok.to_lowercase());
            let mut parts: Vec<String> = vec![];
            for _ in 0..10 {
                let next: Vec<String> = { let o = q.next_token_options().clone(); let mut v: Vec<String> = o.con.iter().map(|c| c.word.clone()).collect(); v.sort(); v.dedup(); v };
                if next.is_empty() { break; }
                let (h2, h1) = (hist[hist.len() - 2].clone(), hist[hist.len() - 1].clone());
                let mut sc: Vec<(f64, &String)> = next.iter().map(|w| (model.score(&h2, &h1, w), w)).collect();
                sc.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
                let total: f64 = sc.iter().map(|x| x.0).sum();
                // only continue while the next word is forced or well supported by what people wrote
                let pick = if next.len() == 1 { Some(sc[0].1.clone()) }
                    else if sc[0].0 >= 300.0 && sc[0].0 >= 0.9 * total { Some(sc[0].1.clone()) } else { None };
                let Some(w) = pick else { break };
                q.add_token(&w); hist.push(w.to_lowercase());
                parts.push(w.clone());
                if w == "." { break; }
            }
            ghost = parts.iter().fold(String::new(), |mut s, w| { if w != "." { s.push(' '); } s.push_str(w); s });
        }
    }

    let mut groups: Vec<(&'static str, Vec<String>)> = GROUPS.iter().filter_map(|g| {
        let mut v: Vec<String> = all.iter().filter(|(_, x)| x == g).map(|(w, _)| shown(w)).collect();
        v.sort_by(|a, b| rank(b, g).partial_cmp(&rank(a, g)).unwrap().then(a.cmp(b)));
        (!v.is_empty()).then_some((*g, v))
    }).collect();
    groups.retain(|(_, v)| !v.is_empty());

    Suggestion { tokens, partial_start, partial, bad_end, confident, options: matched, groups, ghost, complete, error }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (Catalog, RuleGrammar, Ngram) {
        let cat = Catalog::builtin();
        let g = RuleGrammar::new(&cat).unwrap();
        let m = train(&g, cat.templates.iter().map(|t| t.text.clone()));
        (cat, g, m)
    }
    #[test]
    fn prefix_ranking_ghost_and_fixes() {
        let (cat, g, m) = setup();
        let s = suggest(&g, &cat, &m, "");
        assert_eq!(s.options[0].w, "If");
        let s = suggest(&g, &cat, &m, "If a worker is in a hei");
        assert_eq!(s.partial, "hei");
        assert_eq!(s.options[0].w, "height-zone");
        // multi-word term typed halfway
        let s = suggest(&g, &cat, &m, "If a worker is in a height-zone and does no");
        assert_eq!(s.partial, "does no", "{s:?}");
        assert!(s.options.iter().any(|o| o.w == "does not"));
        // ghost: after "for at" the rest of the duration phrase is predictable from the templates
        let s = suggest(&g, &cat, &m, "If a worker is in a height-zone and does not wear a harness for at");
        assert_eq!(s.options[0].w, "at least");
        // misspelling
        let s = suggest(&g, &cat, &m, "If a workr");
        assert!(s.error.as_deref().unwrap().contains("did you mean"), "{s:?}");
        assert_eq!(s.options[0].w, "worker");
        assert_eq!(s.options[0].fix.as_deref(), Some("workr"));
        // impossible word
        let s = suggest(&g, &cat, &m, "If a worker wears a permit");
        assert!(s.error.is_some() && s.options.iter().all(|o| o.w != "permit"));
        // complete
        let t = &cat.templates[8].text;
        assert!(suggest(&g, &cat, &m, t).complete);
        let s = suggest(&g, &cat, &m, "If a worker is in a height-zone and does not wear a harness for at least 3 seconds then ");
        assert_eq!(s.options[0].w, "VisionGuard");
        assert!(s.ghost.starts_with(" raises a"), "ghost: {:?}", s.ghost);
        // many zones possible: no guess
        let s = suggest(&g, &cat, &m, "If a worker is in a ");
        assert!(!s.confident && s.ghost.is_empty(), "{s:?}");
        // a typo in the middle: only that word is marked
        let t = "If a workr is in a height-zone";
        let s = suggest(&g, &cat, &m, t);
        assert_eq!(&t[s.partial_start..s.bad_end], "workr");
        assert_eq!(s.options[0].w, "worker");
    }
}
