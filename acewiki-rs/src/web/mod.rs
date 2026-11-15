//! AceWiki web front end: axum + maud + HTMX.
//!
//! The Scryer machine inside [`ape_rs::Ape`] and the `Rc`-based chart parser are not `Send`, so the
//! whole wiki state lives on one worker thread; HTTP handlers send it closures and await the result
//! (the Java original serialises ontology updates the same way, via `synchronized` methods).

use crate::aceeditor::{self, AceEditor, EntryResult};
use crate::chartparser::ChartParser;
use crate::engine::Engine;
use crate::model::{Element, Kind, Statement, TextElement, Words};
use crate::storage::Storage;
use axum::extract::{Form, Path, Query, State as AxState};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::Arc;

pub const HTMX: &str = include_str!("../../static/htmx.min.js");

pub struct WikiState {
    pub eng: Engine,
    pub storage: Storage,
    pub ed: AceEditor,
    pub read_only: bool,
}

type Job = Box<dyn FnOnce(&mut WikiState) + Send>;

#[derive(Clone)]
pub struct Worker { tx: mpsc::Sender<Job> }

impl Worker {
    /// Spawn the worker; `init` runs on the worker thread (so non-`Send` state is built there).
    pub fn spawn(init: impl FnOnce() -> anyhow::Result<WikiState> + Send + 'static) -> anyhow::Result<Worker> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (rtx, rrx) = mpsc::channel::<Result<(), String>>();
        std::thread::Builder::new().name("acewiki-engine".into()).stack_size(256 << 20).spawn(move || {
            let mut st = match init() { Ok(s) => { let _ = rtx.send(Ok(())); s } Err(e) => { let _ = rtx.send(Err(format!("{e:#}"))); return; } };
            for job in rx { job(&mut st); }
        })?;
        rrx.recv()?.map_err(|e| anyhow::anyhow!(e))?;
        Ok(Worker { tx })
    }
    pub async fn run<T: Send + 'static>(&self, f: impl FnOnce(&mut WikiState) -> T + Send + 'static) -> T {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx.send(Box::new(move |s| { let _ = tx.send(f(s)); })).expect("engine worker gone");
        rx.await.expect("engine worker dropped job")
    }
}

pub fn router(w: Worker) -> Router {
    Router::new()
        .route("/", get(|s: AxState<Worker>| async move { page(s, 0).await }))
        .route("/w/{id}", get(|s: AxState<Worker>, Path(id): Path<u64>| async move { page(s, id).await }))
        .route("/index", get(index))
        .route("/search", get(search))
        .route("/editor", get(editor_get).post(editor_post))
        .route("/w/{id}/add", post(add_sentences))
        .route("/w/{id}/comment", post(add_comment))
        .route("/w/{id}/s/{idx}", get(sentence_details))
        .route("/w/{id}/s/{idx}/delete", post(delete_statement))
        .route("/w/{id}/s/{idx}/toggle", post(toggle_sentence))
        .route("/word", get(word_form).post(word_save))
        .route("/w/{id}/delete", post(delete_element))
        .route("/export/{fmt}", get(export))
        .route("/aceeditor", get(ace_editor_page))
        .route("/aceeditor/editor", post(ace_editor_post))
        .route("/aceeditor/analyze", post(ace_editor_analyze))
        .route("/aceeditor/word", post(ace_editor_word))
        .route("/aceeditor/lexicon", get(ace_editor_lexicon))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat))
        .route("/healthz", get(|| async { Json(serde_json::json!({"status":"ok"})) }))
        .route("/static/htmx.min.js", get(|| async { ([(header::CONTENT_TYPE, "application/javascript"), (header::CACHE_CONTROL, "max-age=86400")], HTMX) }))
        .with_state(w)
}

// ------------------------------------------------------------------------------------------------
// layout & helpers
// ------------------------------------------------------------------------------------------------

const CSS: &str = r#"
:root{--fg:#1d2430;--mut:#6a7381;--line:#dfe3ea;--acc:#2b5fb4;--bg:#fbfbfc;--bad:#a3261b;--warn:#8a5a00;--ok:#1f6f43}
*{box-sizing:border-box}body{margin:0;font:15px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif;color:var(--fg);background:var(--bg)}
header{display:flex;gap:18px;align-items:center;padding:10px 22px;border-bottom:1px solid var(--line);background:#fff;position:sticky;top:0;z-index:5}
header .brand{font-weight:650;letter-spacing:.2px}header a{color:var(--fg);text-decoration:none}header nav a{margin-right:14px;color:var(--mut)}
header nav a:hover{color:var(--acc)}header form{margin-left:auto}input,select,textarea{font:inherit;padding:5px 8px;border:1px solid var(--line);border-radius:6px;background:#fff}
main{max-width:980px;margin:0 auto;padding:22px}h1{font-size:26px;margin:4px 0 2px}h2{font-size:15px;text-transform:uppercase;letter-spacing:.6px;color:var(--mut);margin:26px 0 8px;border-bottom:1px solid var(--line);padding-bottom:4px}
a{color:var(--acc)}.kind{color:var(--mut);font-size:13px}.words{color:var(--mut);font-size:13px;margin-bottom:12px}
.st{padding:5px 0;display:flex;gap:10px;align-items:baseline}.st .tools{margin-left:auto;font-size:12px;white-space:nowrap}.st .tools a,.st .tools button{color:var(--mut);background:none;border:0;cursor:pointer;font:inherit;padding:0 3px}
.st.unasserted .txt{color:var(--mut);font-style:italic}.st.notowl .txt{color:var(--warn)}.st.question .txt{font-weight:550}.mark{font-size:11px;padding:0 5px;border-radius:8px;border:1px solid var(--line);color:var(--mut)}
.comment{color:#40464f;background:#f2f4f7;border-radius:6px;padding:6px 10px;margin:6px 0;white-space:pre-wrap}.answer{margin:2px 0 8px 22px;font-size:14px}.answer .none{color:var(--mut)}
button.btn,.btn{display:inline-block;background:var(--acc);color:#fff;border:0;border-radius:6px;padding:6px 12px;cursor:pointer;font:inherit;text-decoration:none}.btn.sec{background:#e8ecf3;color:var(--fg)}.btn:disabled{opacity:.45;cursor:default}
.editor{border:1px solid var(--line);border-radius:10px;background:#fff;padding:12px;margin:10px 0}.editor .text{font-size:17px;min-height:30px;padding:6px 8px;border-bottom:1px dashed var(--line);margin-bottom:8px}
.editor .text .tok{margin-right:4px}.editor .row{display:flex;gap:8px;align-items:center;margin:6px 0}.menus{display:flex;flex-wrap:wrap;gap:10px;max-height:340px;overflow:auto}
.menu{min-width:150px;max-width:220px;border:1px solid var(--line);border-radius:8px}.menu h4{margin:0;padding:4px 8px;font-size:12px;color:#fff;border-radius:7px 7px 0 0}
.menu .items{max-height:260px;overflow:auto;padding:2px 0}.menu button{display:block;width:100%;text-align:left;background:none;border:0;padding:2px 9px;cursor:pointer;font:inherit;font-size:14px}.menu button:hover{background:#eef3fb}
.menu .new{color:var(--acc);font-size:13px}.msg{padding:6px 10px;border-radius:6px;margin:8px 0}.msg.err{background:#fbeaea;color:var(--bad)}.msg.info{background:#eaf3fb}
pre{background:#f4f6f9;border-radius:6px;padding:10px;overflow:auto;font-size:12.5px}.cols{columns:3 220px}.cols a{display:block}table.kv td{padding:2px 10px 2px 0;vertical-align:top}
.flag-ok{color:var(--ok)}.flag-no{color:var(--bad)}textarea{width:100%}.muted{color:var(--mut)}
"#;

fn layout(st: &WikiState, title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8"; meta name="viewport" content="width=device-width,initial-scale=1";
                title { (title) " — AceWiki " (st.eng.wiki.name) }
                style { (PreEscaped(CSS)) }
                script src="/static/htmx.min.js" {}
            }
            body {
                header {
                    a.brand href="/" { "AceWiki · " (st.eng.wiki.name) }
                    nav {
                        a href="/" { "Main page" } a href="/index" { "Index" }
                        @if !st.read_only { a href="/word" { "New word" } }
                        a href="/aceeditor" { "ACE Editor" } a href="/export/owl" { "Export OWL" }
                    }
                    form action="/search" { input name="q" placeholder="Search words"; }
                }
                main { (body) }
            }
        }
    }
}

fn kind_label(k: &Kind) -> &'static str {
    match k { Kind::MainPage => "", Kind::ProperName => "proper name", Kind::Noun => "noun", Kind::NounOf => "of-construct", Kind::TrVerb => "verb", Kind::TrAdj => "transitive adjective" }
}

fn glue(prev: Option<&str>, cur: &str) -> bool {
    let punct = cur.len() == 1 && ".,:;?!".contains(cur);
    prev.is_some() && !punct
}

/// Sentence with links to the articles of its words.
fn sentence_html(eng: &Engine, serialized: &str) -> Markup {
    let Ok(tes) = eng.tokenize(serialized) else { return html! { span.muted { (serialized) } } };
    let disp = eng.wiki.display_tokens(&tes);
    html! {
        @for (i, te) in tes.iter().enumerate() {
            @if glue(if i > 0 { Some(&disp[i - 1]) } else { None }, &disp[i]) { " " }
            @match te {
                TextElement::Onto { id, .. } => a href={ "/w/" (id) } { (disp[i]) },
                TextElement::Plain(_) => (disp[i]),
            }
        }
    }
}

fn element_link(eng: &Engine, id: u64) -> Markup {
    match eng.wiki.elements.get(&id) { Some(e) => html! { a href={ "/w/" (id) } { (e.heading()) } }, None => html! { "?" } }
}

fn status_of(eng: &mut Engine, serialized: &str, integrated: bool) -> (&'static str, &'static str) {
    if Engine::is_question(serialized) { return ("question", ""); }
    if !integrated { return ("unasserted", "unasserted"); }
    match eng.analyze(serialized) { Ok(a) if a.reasonable => ("ok", ""), Ok(a) if a.is_owlswrl => ("notowl", "rule"), _ => ("notowl", "not OWL") }
}

fn uses_element(e: &Element, id: u64) -> bool {
    let pat = format!("<{id},");
    e.article.iter().any(|s| matches!(s, Statement::Sentence { serialized, .. } if serialized.contains(&pat)))
}

fn html_resp(m: Markup) -> Response { Html(m.into_string()).into_response() }
fn hx_redirect(to: &str) -> Response { ([("HX-Redirect", to.to_string())], "").into_response() }
fn err(msg: impl std::fmt::Display) -> Markup { html! { div.msg.err { (msg.to_string()) } } }

// ------------------------------------------------------------------------------------------------
// article pages
// ------------------------------------------------------------------------------------------------

async fn page(AxState(w): AxState<Worker>, id: u64) -> Response {
    w.run(move |st| {
        let Some(e) = st.eng.wiki.elements.get(&id).cloned() else { return (StatusCode::NOT_FOUND, html_resp(layout(st, "Not found", err("No such article")))).into_response() };
        let title = if e.kind == Kind::MainPage { "Main Page".to_string() } else { e.heading() };
        let ro = st.read_only;
        let mut rows = vec![];
        for (idx, s) in e.article.iter().enumerate() {
            match s {
                Statement::Comment(c) => { let ch = comment_html(&st.eng, c); rows.push(html! { div.st { div.comment { (ch) } @if !ro { span.tools { (del_button(id, idx)) } } } }) }
                Statement::Sentence { serialized, integrated } => {
                    let (cls, mark) = status_of(&mut st.eng, serialized, *integrated);
                    let answer = if cls == "question" { st.eng.answer(serialized).ok().flatten() } else { None };
                    let eng = &st.eng;
                    rows.push(html! {
                        div class={ "st " (cls) } {
                            span.txt { (sentence_html(eng, serialized)) }
                            @if !mark.is_empty() { span.mark { (mark) } }
                            span.tools {
                                a href={ "/w/" (id) "/s/" (idx) } { "details" }
                                @if !ro && cls != "question" {
                                    button hx-post={ "/w/" (id) "/s/" (idx) "/toggle" } hx-target="body" { @if *integrated { "retract" } @else { "reassert" } }
                                }
                                @if !ro { (del_button(id, idx)) }
                            }
                        }
                        @if let Some(ans) = answer {
                            div.answer { @if ans.is_empty() { span.none { "(no answer)" } } @else {
                                @for (i, a) in ans.iter().enumerate() { @if i > 0 { ", " } (element_link(eng, *a)) } } }
                        }
                    });
                }
            }
        }
        // reasoning sections (ConceptPage / IndividualPage)
        let mut sections = vec![];
        match e.kind {
            Kind::Noun => {
                let sat = st.eng.is_satisfiable(id);
                let inds = st.eng.individuals_of(id);
                let sup = st.eng.super_concepts(id);
                let sub = st.eng.sub_concepts(id);
                let eng = &st.eng;
                sections.push(html! {
                    @if !sat { div.msg.err { "This concept is unsatisfiable: nothing can be " (e.heading()) "." } }
                    h2 { "Individuals" } (link_list(eng, &inds))
                    h2 { "Hierarchy" }
                    table.kv { tr { td.muted { "super-concepts" } td { (link_list(eng, &sup)) } } tr { td.muted { "sub-concepts" } td { (link_list(eng, &sub)) } } }
                });
            }
            Kind::ProperName => {
                let cs = st.eng.concepts_of(id);
                let eng = &st.eng;
                sections.push(html! { h2 { "Assignments" } (link_list(eng, &cs)) });
            }
            _ => {}
        }
        let refs: Vec<(u64, String)> = if e.kind == Kind::MainPage { vec![] } else {
            st.eng.wiki.elements.values().filter(|o| o.id != id).flat_map(|o| o.article.iter().filter_map(move |s| match s {
                Statement::Sentence { serialized, .. } if serialized.contains(&format!("<{id},")) => Some((o.id, serialized.clone())), _ => None })).collect()
        };
        let eng = &st.eng;
        let words: Vec<String> = e.words.all().into_iter().flatten().collect();
        let body = html! {
            h1 { (title) }
            @if e.kind != Kind::MainPage {
                div.kind { (kind_label(&e.kind)) }
                div.words { "word forms: " (words.join(" · ").replace('_', " "))
                    @if !ro { " · " a href={ "/word?id=" (id) } { "edit" } " · "
                        button.btn.sec style="padding:1px 8px;font-size:12px" hx-post={ "/w/" (id) "/delete" } hx-confirm="Delete this word and its article?" hx-target="body" { "delete" } } }
            }
            div #flash {}
            @if e.article.is_empty() { p.muted { "This article is empty." } }
            @for r in &rows { (r) }
            @if !ro {
                div.row style="display:flex;gap:8px;margin-top:10px" {
                    button.btn hx-get={ "/editor?el=" (id) } hx-target={ "#ed" } { "+ sentence" }
                    button.btn.sec onclick="document.getElementById('cm').style.display='block'" { "+ comment" }
                }
                form #cm style="display:none;margin-top:8px" method="post" action={ "/w/" (id) "/comment" } {
                    textarea name="text" rows="3" {} button.btn { "Add comment" }
                }
                div #ed {}
            }
            @for s in &sections { (s) }
            @if !refs.is_empty() {
                h2 { "References" }
                @for (oid, s) in &refs { div.st { span.txt { (sentence_html(eng, s)) } span.tools { (element_link(eng, *oid)) } } }
            }
            @if e.kind == Kind::MainPage {
                h2 { "Reasoner" } p.muted { (if eng.reasoner_note.is_empty() { "not loaded yet".to_string() } else { eng.reasoner_note.clone() }) }
            }
        };
        html_resp(layout(st, &title, body))
    }).await
}

/// `CommentComponent`: `[[word]]` becomes a wiki link, `http(s)://…` a web link.
fn comment_html(eng: &Engine, c: &str) -> Markup {
    let re = regex::Regex::new(r"\[\[([^\]]+)\]\]|https?://[^\s<>\]]+").unwrap();
    let mut out = String::new();
    let mut last = 0;
    let esc = |s: &str| html! { (s) }.into_string();
    for m in re.find_iter(c) {
        out += &esc(&c[last..m.start()]);
        let t = m.as_str();
        if let Some(w) = t.strip_prefix("[[").and_then(|x| x.strip_suffix("]]")) {
            match eng.wiki.by_word(&w.replace(' ', "_")).or_else(|| eng.wiki.by_word(w)) {
                Some(e) => out += &html! { a href={ "/w/" (e.id) } { (w) } }.into_string(),
                None => out += &html! { span.muted { (w) } }.into_string(),
            }
        } else { out += &html! { a href=(t) rel="noopener" target="_blank" { (t) } }.into_string(); }
        last = m.end();
    }
    out += &esc(&c[last..]);
    PreEscaped(out)
}

fn del_button(id: u64, idx: usize) -> Markup {
    html! { button hx-post={ "/w/" (id) "/s/" (idx) "/delete" } hx-confirm="Delete this statement?" hx-target="body" { "delete" } }
}

fn link_list(eng: &Engine, ids: &[u64]) -> Markup {
    let mut v: Vec<(String, u64)> = ids.iter().filter_map(|i| eng.wiki.elements.get(i).map(|e| (e.heading().to_lowercase(), *i))).collect();
    v.sort();
    html! { @if v.is_empty() { span.muted { "(none)" } } @else { div.cols { @for (_, i) in &v { (element_link(eng, *i)) } } } }
}

async fn index(AxState(w): AxState<Worker>) -> Response {
    w.run(|st| {
        let mut groups: BTreeMap<&'static str, Vec<(String, u64)>> = BTreeMap::new();
        for e in st.eng.wiki.elements.values() {
            if e.kind == Kind::MainPage { continue; }
            groups.entry(kind_label(&e.kind)).or_default().push((e.heading().to_lowercase(), e.id));
        }
        let eng = &st.eng;
        let body = html! {
            h1 { "Index" } p.muted { (eng.wiki.elements.len() - 1) " words" }
            @for (k, v) in groups.iter_mut() {
                @let _ = v.sort();
                h2 { (k) " (" (v.len()) ")" } div.cols { @for (_, id) in v.iter() { (element_link(eng, *id)) } }
            }
        };
        html_resp(layout(st, "Index", body))
    }).await
}

#[derive(Deserialize)]
struct Q { q: Option<String> }
async fn search(AxState(w): AxState<Worker>, Query(q): Query<Q>) -> Response {
    let q = q.q.unwrap_or_default();
    w.run(move |st| {
        let ql = q.to_lowercase().replace(' ', "_");
        let hits: Vec<u64> = st.eng.wiki.elements.values().filter(|e| e.kind != Kind::MainPage &&
            e.words.all().into_iter().flatten().any(|w| w.to_lowercase().contains(&ql))).map(|e| e.id).collect();
        if hits.len() == 1 { return Redirect::to(&format!("/w/{}", hits[0])).into_response(); }
        let eng = &st.eng;
        let body = html! { h1 { "Search: " (q) } (link_list(eng, &hits)) };
        html_resp(layout(st, "Search", body))
    }).await
}

// ------------------------------------------------------------------------------------------------
// predictive editor (PreditorWindow) for AceWiki sentences
// ------------------------------------------------------------------------------------------------

/// Menu group for an AceWiki category (`ACEHandler`'s editor controller set-up).
fn acewiki_menu_group(cat: Option<&str>) -> &'static str {
    match cat {
        Some("propername") => "proper name", Some("noun") => "noun", Some("nounpl") => "plural noun", Some("nounof") => "of-construct",
        Some("verbsg" | "verbinf") => "verb", Some("pverb") => "passive verb", Some("tradj") => "transitive adjective",
        Some("defnoun" | "reference") => "reference", Some("variable") => "new variable", _ => "function word",
    }
}
const ACEWIKI_MENU_ORDER: &[(&str, u32)] = &[("function word", 0), ("proper name", 60), ("noun", 100), ("plural noun", 120), ("of-construct", 140),
    ("transitive adjective", 180), ("verb", 210), ("passive verb", 210), ("new variable", 320), ("reference", 320)];
fn ext_kind(cat: &str) -> Option<&'static str> {
    match cat { "propername" => Some("propername"), "noun" | "nounpl" => Some("noun"), "nounof" => Some("nounof"),
        "verbsg" | "verbinf" | "pverb" => Some("trverb"), "tradj" => Some("tradj"), _ => None }
}
fn hue_color(shift: u32) -> String { format!("hsl({}, 45%, 45%)", (215 + shift) % 360) }

/// Greedy tokenisation of free text driven by the parser's predictions. Accepted tokens are added
/// to `p`; on failure returns the tokens accepted so far and an explanation.
pub fn feed_text(p: &mut ChartParser, text: &str) -> Result<Vec<String>, (Vec<String>, String)> {
    let re = regex::Regex::new(r"[A-Za-z0-9_\-']+|[.,?!:;]").unwrap();
    let raw: Vec<&str> = re.find_iter(text).map(|m| m.as_str()).collect();
    let mut out = vec![];
    let mut i = 0;
    while i < raw.len() {
        let mut done = false;
        for k in (1..=6.min(raw.len() - i)).rev() {
            let ws = &raw[i..i + k];
            if k > 1 && ws.iter().any(|w| w.len() == 1 && ".,?!:;".contains(*w)) { continue; }
            let mut cands = vec![ws.join(" "), ws.join("_")];
            if k > 1 { cands.push(format!("{} {}", ws[0], ws[1..].join("_"))); }
            let lower = |s: &String| { let mut c = s.chars(); c.next().map(|f| f.to_lowercase().collect::<String>() + c.as_str()).unwrap_or_default() };
            let extra: Vec<String> = cands.iter().map(lower).collect();
            cands.extend(extra);
            if let Some(c) = cands.into_iter().find(|c| p.is_possible_next_token(c)) {
                p.add_token(&c); out.push(c); i += k; done = true; break;
            }
        }
        if !done {
            let opts = p.next_token_options().clone();
            let mut sample: Vec<String> = opts.con.iter().map(|c| c.word.clone()).collect();
            sample.sort(); sample.dedup();
            let n = sample.len();
            sample.truncate(12);
            let ctx = if out.is_empty() { "at the beginning".to_string() } else { format!("after \"{}\"", out.join(" ").replace('_', " ")) };
            return Err((out, format!("\"{}\" is not possible {ctx}. Possible next words include: {}{}", raw[i], sample.join(", "), if n > 12 { format!(" … ({n} options)") } else { String::new() })));
        }
    }
    Ok(out)
}

#[derive(Deserialize, Default)]
struct EdForm { el: Option<u64>, tokens: Option<String>, op: Option<String>, tok: Option<String>, filter: Option<String>, replace: Option<usize> }

async fn editor_get(AxState(w): AxState<Worker>, Query(f): Query<EdForm>) -> Response {
    w.run(move |st| {
        let el = f.el.unwrap_or(0);
        let mut tokens = vec![];
        if let Some(r) = f.replace {
            if let Some(Statement::Sentence { serialized, .. }) = st.eng.wiki.elements.get(&el).and_then(|e| e.article.get(r)) {
                if let Ok(tes) = st.eng.tokenize(serialized) { tokens = st.eng.wiki.editor_elements(&tes).iter().map(|t| st.eng.wiki.original_text(t)).collect(); }
            }
        }
        html_resp(acewiki_editor(st, el, tokens, "", None, f.replace))
    }).await
}

async fn editor_post(AxState(w): AxState<Worker>, Form(f): Form<EdForm>) -> Response {
    w.run(move |st| {
        let el = f.el.unwrap_or(0);
        let mut tokens: Vec<String> = f.tokens.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or_default();
        let mut msg = None;
        let mut filter = f.filter.clone().unwrap_or_default();
        match f.op.as_deref() {
            Some("add") => if let Some(t) = f.tok { tokens.push(t); filter.clear(); },
            Some("del") => { tokens.pop(); }
            Some("clear") => tokens.clear(),
            Some("type") => {
                let mut p = st.eng.parser();
                p.add_tokens(&tokens);
                match feed_text(&mut p, &filter) {
                    Ok(t) => { tokens.extend(t); filter.clear(); }
                    Err((t, m)) => { tokens.extend(t); msg = Some(m); }
                }
            }
            _ => {}
        }
        html_resp(acewiki_editor(st, el, tokens, &filter, msg, f.replace))
    }).await
}

fn acewiki_editor(st: &mut WikiState, el: u64, tokens: Vec<String>, filter: &str, msg: Option<String>, replace: Option<usize>) -> Markup {
    let mut p = st.eng.parser();
    let mut ok_tokens = vec![];
    for t in &tokens { if p.is_possible_next_token(t) { p.add_token(t); ok_tokens.push(t.clone()); } else { break; } }
    let complete = p.is_complete() && !ok_tokens.is_empty();
    let opts = p.next_token_options().clone();
    let mut groups: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let fl = filter.to_lowercase();
    for c in &opts.con {
        let shown = c.word.replace('_', " ");
        if !fl.is_empty() && !shown.to_lowercase().starts_with(&fl) && !c.word.to_lowercase().starts_with(&fl) { continue; }
        groups.entry(acewiki_menu_group(c.category.as_ref().map(|x| x.name()))).or_default().push(c.word.clone());
    }
    for v in groups.values_mut() { v.sort_by_key(|w| w.to_lowercase()); v.dedup(); }
    let mut news: BTreeMap<&'static str, &'static str> = BTreeMap::new();
    for a in &opts.abs { if let Some(k) = ext_kind(a.category_name()) { news.insert(acewiki_menu_group(Some(a.category_name())), k); } }
    let tok_json = serde_json::to_string(&ok_tokens).unwrap();
    let tes: Vec<TextElement> = ok_tokens.iter().map(|t| st.eng.wiki.create_text_element(t, &st.eng.grammar.terminals)).collect();
    let disp = st.eng.wiki.display_tokens(&tes);
    let ro = st.read_only;
    html! {
        div.editor #ed hx-target="#ed" hx-swap="outerHTML" {
            div.text {
                @for (i, d) in disp.iter().enumerate() { @if glue(if i > 0 { Some(&disp[i - 1]) } else { None }, d) { " " } span.tok { (d) } }
                @if disp.is_empty() { span.muted { "Start typing or pick a word…" } }
            }
            @if let Some(m) = &msg { (err(m)) }
            form.row hx-post="/editor" {
                input type="hidden" name="el" value=(el); input type="hidden" name="tokens" value=(tok_json);
                @if let Some(r) = replace { input type="hidden" name="replace" value=(r); }
                input type="hidden" name="op" value="type";
                input name="filter" value=(filter) autofocus autocomplete="off" placeholder="type words, Enter to add"
                    hx-post="/editor" hx-trigger="keyup changed delay:250ms" hx-vals=r#"{"op":"filter"}"# style="flex:1";
                button.btn.sec type="button" hx-post="/editor" hx-include="closest form" hx-vals=r#"{"op":"del"}"# { "⌫" }
                button.btn.sec type="button" hx-post="/editor" hx-include="closest form" hx-vals=r#"{"op":"clear"}"# { "clear" }
                button.btn type="button" disabled[!complete || ro] hx-post={ "/w/" (el) "/add" } hx-include="closest form" hx-target="#ed" { "OK" }
                button.btn.sec type="button" onclick="this.closest('.editor').remove()" { "cancel" }
            }
            div.menus {
                @for (g, shift) in ACEWIKI_MENU_ORDER {
                    @if let Some(items) = groups.get(g) {
                        div.menu { h4 style={ "background:" (hue_color(*shift)) } { (g) }
                            div.items {
                                @for w in items.iter().take(400) {
                                    button hx-post="/editor" hx-vals=(serde_json::json!({"el": el, "tokens": tok_json, "op": "add", "tok": w, "replace": replace}).to_string()) { (w.replace('_', " ")) }
                                }
                                @if let Some(k) = news.get(g) { @if !ro { a.new href={ "/word?kind=" (k) "&back=" (el) } target="_blank" { "new…" } } }
                            }
                        }
                    } @else if let Some(k) = news.get(g) {
                        @if !ro { div.menu { h4 style={ "background:" (hue_color(*shift)) } { (g) } div.items { a.new href={ "/word?kind=" (k) "&back=" (el) } target="_blank" { "new…" } } } }
                    }
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct AddForm { tokens: String, choice: Option<String>, replace: Option<usize> }

/// `SentenceEditorHandler`: split into sentences, a→every suggestion, commit with consistency check.
async fn add_sentences(AxState(w): AxState<Worker>, Path(id): Path<u64>, Form(f): Form<AddForm>) -> Response {
    w.run(move |st| {
        if st.read_only { return html_resp(err("This wiki is read-only.")); }
        let tokens: Vec<String> = serde_json::from_str(&f.tokens).unwrap_or_default();
        let mut sents: Vec<Vec<String>> = vec![vec![]];
        for t in &tokens { sents.last_mut().unwrap().push(t.clone()); if t == "." || t == "?" { sents.push(vec![]); } }
        sents.retain(|s| !s.is_empty());
        // AToEverySuggestion (asked for the first sentence that triggers it)
        if f.choice.is_none() {
            for s in &sents {
                if s.len() > 2 && (s[0] == "a" || s[0] == "an" || s[0] == "A" || s[0] == "An") && !s[1].ends_with(" of") && s.last().unwrap() == "." {
                    let n = s[1].replace('_', " ");
                    let tj = serde_json::to_string(&tokens).unwrap();
                    return html_resp(html! { div.editor #ed {
                        div.msg.info { "Your sentence \"a " (n) " ...\" is interpreted as \"there is a " (n) " that ...\". Do you want to say \"every " (n) " ...\"?" }
                        @for c in ["a", "every"] {
                            button.btn style="margin-right:8px" hx-post={ "/w/" (id) "/add" } hx-target="#ed"
                                hx-vals=(serde_json::json!({"tokens": tj, "choice": c, "replace": f.replace}).to_string()) { (c) " ..." }
                        }
                    } });
                }
            }
        }
        let mut new_statements = vec![];
        let mut conflict = false;
        for mut s in sents {
            if f.choice.as_deref() == Some("every") && (s[0] == "a" || s[0] == "an" || s[0] == "A" || s[0] == "An") && !s[1].ends_with(" of") && s.last().map(|x| x == ".").unwrap_or(false) { s[0] = "every".into(); }
            let tes: Vec<TextElement> = s.iter().map(|t| st.eng.wiki.create_text_element(t, &st.eng.grammar.terminals)).collect();
            let ser = st.eng.wiki.serialize(&tes);
            let a = match st.eng.analyze(&ser) { Ok(a) => a, Err(e) => return html_resp(err(format!("APE error: {e:#}"))) };
            let integrated = if !Engine::is_question(&ser) && a.reasonable {
                match st.eng.would_be_consistent(&ser) { Ok(true) => true, _ => { conflict = true; false } }
            } else { true };
            new_statements.push(Statement::Sentence { serialized: ser, integrated });
        }
        let Some(e) = st.eng.wiki.elements.get_mut(&id) else { return html_resp(err("No such article")) };
        match f.replace {
            Some(r) if r < e.article.len() => { e.article.splice(r..r + 1, new_statements); }
            _ => e.article.extend(new_statements),
        }
        let e = e.clone();
        if let Err(x) = st.storage.save_element(&st.eng.wiki.name, &e) { return html_resp(err(format!("save failed: {x:#}"))); }
        st.eng.invalidate();
        st.eng.flush_cache();
        if conflict {
            return html_resp(html! { div #ed { div.msg.err { "Conflict: the sentence is in conflict with the current knowledge. For that reason, it cannot be added to the knowledge base." }
                a.btn href={ "/w/" (id) } { "OK" } } });
        }
        hx_redirect(&format!("/w/{id}"))
    }).await
}

#[derive(Deserialize)]
struct TextForm { text: String }
async fn add_comment(AxState(w): AxState<Worker>, Path(id): Path<u64>, Form(f): Form<TextForm>) -> Response {
    w.run(move |st| {
        if st.read_only || f.text.trim().is_empty() { return Redirect::to(&format!("/w/{id}")).into_response(); }
        if let Some(e) = st.eng.wiki.elements.get_mut(&id) {
            e.article.push(Statement::Comment(f.text.replace("\r\n", "\n")));
            let e = e.clone();
            let _ = st.storage.save_element(&st.eng.wiki.name, &e);
        }
        Redirect::to(&format!("/w/{id}")).into_response()
    }).await
}

async fn delete_statement(AxState(w): AxState<Worker>, Path((id, idx)): Path<(u64, usize)>) -> Response {
    w.run(move |st| {
        if !st.read_only { if let Some(e) = st.eng.wiki.elements.get_mut(&id) {
            if idx < e.article.len() { e.article.remove(idx); }
            let e = e.clone();
            let _ = st.storage.save_element(&st.eng.wiki.name, &e);
            st.eng.invalidate();
        } }
        hx_redirect(&format!("/w/{id}"))
    }).await
}

/// retract / reassert (`Ontology.retractSentence` / `reassert` with consistency check).
async fn toggle_sentence(AxState(w): AxState<Worker>, Path((id, idx)): Path<(u64, usize)>) -> Response {
    w.run(move |st| {
        if st.read_only { return hx_redirect(&format!("/w/{id}")); }
        let Some(Statement::Sentence { serialized, integrated }) = st.eng.wiki.elements.get(&id).and_then(|e| e.article.get(idx)).cloned() else { return hx_redirect(&format!("/w/{id}")) };
        let new = if integrated { false } else { st.eng.would_be_consistent(&serialized).unwrap_or(false) };
        if !integrated && !new {
            let body = html! { div.msg.err { "Conflict: the sentence is in conflict with the current knowledge. For that reason, it cannot be added to the knowledge base." } a href={ "/w/" (id) } { "back" } };
            return html_resp(layout(st, "Conflict", body));
        }
        if let Some(Statement::Sentence { integrated, .. }) = st.eng.wiki.elements.get_mut(&id).and_then(|e| e.article.get_mut(idx)) { *integrated = new; }
        let e = st.eng.wiki.elements[&id].clone();
        let _ = st.storage.save_element(&st.eng.wiki.name, &e);
        st.eng.invalidate();
        hx_redirect(&format!("/w/{id}"))
    }).await
}

async fn sentence_details(AxState(w): AxState<Worker>, Path((id, idx)): Path<(u64, usize)>) -> Response {
    w.run(move |st| {
        let Some(Statement::Sentence { serialized, integrated }) = st.eng.wiki.elements.get(&id).and_then(|e| e.article.get(idx)).cloned() else {
            return (StatusCode::NOT_FOUND, "no such sentence").into_response() };
        let a = match st.eng.analyze(&serialized) { Ok(a) => a, Err(e) => return html_resp(layout(st, "Error", err(format!("{e:#}")))) };
        let yn = |b: bool| if b { html! { span.flag-ok { "yes" } } } else { html! { span.flag-no { "no" } } };
        let eng = &st.eng;
        let body = html! {
            p { a href={ "/w/" (id) } { "← " (eng.wiki.elements[&id].heading()) } }
            h1 style="font-size:20px" { (sentence_html(eng, &serialized)) }
            table.kv {
                tr { td.muted { "integrated" } td { (yn(integrated)) } }
                tr { td.muted { "OWL/SWRL" } td { (yn(a.is_owlswrl)) } } tr { td.muted { "OWL" } td { (yn(a.is_owl)) } }
                tr { td.muted { "reasonable" } td { (yn(a.reasonable || (Engine::is_question(&serialized) && a.integrable))) } }
                @if !a.profile_violations.is_empty() { tr { td.muted { "profile" } td { (a.profile_violations.join("; ")) } } }
            }
            h2 { "Paraphrase" } p { (a.paraphrase) }
            @if !a.messages.is_empty() { h2 { "Messages" } @for m in &a.messages { div.muted { (m.0) " · " (m.1) ": " (m.2) } } }
            h2 { "Syntax tree" } pre { (a.syntaxpp) }
            h2 { "DRS" } pre { (a.drspp) }
            h2 { "OWL (functional-style)" } pre { (a.owlfsspp) }
            h2 { "Serialized" } pre { (serialized) }
            h2 { "APE input" } pre { (a.text) "\n\n" (a.lexicon.replace(". ", ".\n")) }
        };
        html_resp(layout(st, "Sentence", body))
    }).await
}

// ------------------------------------------------------------------------------------------------
// lexicon editor (*Changer classes)
// ------------------------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct WordQ { kind: Option<String>, id: Option<u64>, back: Option<u64> }
async fn word_form(AxState(w): AxState<Worker>, Query(q): Query<WordQ>) -> Response {
    w.run(move |st| html_resp(word_page(st, q.kind.as_deref(), q.id, q.back, None, &BTreeMap::new()))).await
}

fn word_fields(kind: &str) -> Vec<(&'static str, &'static str, bool)> {
    match kind {
        "propername" => vec![("w0", "proper name", true), ("def0", "… used with \"the\"", false), ("w2", "abbreviation", false), ("def2", "… used with \"the\"", false)],
        "noun" => vec![("w0", "singular", true), ("w1", "plural", true)],
        "nounof" => vec![("w0", "noun (without \"of\")", true)],
        "trverb" => vec![("w0", "third singular", true), ("w1", "bare infinitive", true), ("w2", "past participle", false)],
        "tradj" => vec![("w0", "transitive adjective (incl. preposition)", true)],
        _ => vec![],
    }
}

fn word_page(st: &WikiState, kind: Option<&str>, id: Option<u64>, back: Option<u64>, msg: Option<String>, vals: &BTreeMap<String, String>) -> Markup {
    let existing = id.and_then(|i| st.eng.wiki.elements.get(&i));
    let kind = existing.map(|e| e.kind.internal()).or(kind);
    let mut vals = vals.clone();
    if let (Some(e), true) = (existing, vals.is_empty()) {
        match &e.words {
            Words::ProperName { word, word_def, abbrev, abbrev_def } => {
                vals.insert("w0".into(), word.replace('_', " ")); if *word_def { vals.insert("def0".into(), "on".into()); }
                if let Some(a) = abbrev { vals.insert("w2".into(), a.replace('_', " ")); } if *abbrev_def { vals.insert("def2".into(), "on".into()); }
            }
            Words::Noun { sg, pl } => { vals.insert("w0".into(), sg.replace('_', " ")); vals.insert("w1".into(), pl.replace('_', " ")); }
            Words::NounOf { word } => { vals.insert("w0".into(), word.trim_end_matches(" of").replace('_', " ")); }
            Words::TrVerb { third, inf, passive } => { vals.insert("w0".into(), third.replace('_', " ")); vals.insert("w1".into(), inf.replace('_', " "));
                if let Some(p) = passive { vals.insert("w2".into(), p.trim_end_matches(" by").replace('_', " ")); } }
            Words::TrAdj { word } => { vals.insert("w0".into(), word.replace('_', " ")); }
            Words::Main(_) => {}
        }
    }
    let body = html! {
        h1 { @if existing.is_some() { "Edit word" } @else { "New word" } }
        @if let Some(m) = &msg { (err(m)) }
        @if let Some(k) = kind {
            form method="post" action="/word" {
                input type="hidden" name="kind" value=(k);
                @if let Some(i) = id { input type="hidden" name="id" value=(i); }
                @if let Some(b) = back { input type="hidden" name="back" value=(b); }
                table.kv {
                    @for (name, label, req) in word_fields(k) {
                        tr { td { (label) @if req { " *" } } td {
                            @if name.starts_with("def") { input type="checkbox" name=(name) checked[vals.contains_key(name)]; }
                            @else { input name=(name) value=(vals.get(name).cloned().unwrap_or_default()); }
                        } }
                    }
                }
                p { button.btn { "OK" } }
            }
        } @else {
            p { "Word type:" }
            @for (k, l) in [("propername", "proper name"), ("noun", "noun"), ("nounof", "of-construct"), ("trverb", "transitive verb"), ("tradj", "transitive adjective")] {
                a.btn.sec style="margin:0 8px 8px 0" href={ "/word?kind=" (k) } { (l) }
            }
        }
    };
    layout(st, "Word", body)
}

/// Word validation as in the Java `*Changer` classes / `ACEOWLLexicon`.
fn norm_word(s: &str) -> String {
    let r = regex::Regex::new(r"(\s|_)+").unwrap().replace_all(s.trim(), "_").into_owned();
    r.trim_matches('_').to_string()
}

async fn word_save(AxState(w): AxState<Worker>, Form(f): Form<BTreeMap<String, String>>) -> Response {
    w.run(move |st| {
        if st.read_only { return Redirect::to("/").into_response(); }
        let kind_s = f.get("kind").cloned().unwrap_or_default();
        let Some(kind) = Kind::parse(&kind_s) else { return html_resp(word_page(st, None, None, None, Some("unknown word type".into()), &f)) };
        let id: Option<u64> = f.get("id").and_then(|x| x.parse().ok());
        let back: Option<u64> = f.get("back").and_then(|x| x.parse().ok());
        let g = |k: &str| norm_word(f.get(k).map(String::as_str).unwrap_or(""));
        let fail = |st: &WikiState, m: String| html_resp(word_page(st, Some(&kind_s), id, back, Some(m), &f));
        for (name, label, req) in word_fields(&kind_s) {
            if !name.starts_with("def") && req && g(name).is_empty() { return fail(st, format!("A required word form is missing: {label}.")); }
        }
        let valid = regex::Regex::new(r"^[a-zA-Z][a-zA-Z0-9_-]*$").unwrap();
        let words = match kind {
            Kind::ProperName => {
                let ab = g("w2");
                Words::ProperName { word: g("w0"), word_def: f.contains_key("def0"), abbrev: if ab.is_empty() || ab == g("w0") { None } else { Some(ab) }, abbrev_def: f.contains_key("def2") }
            }
            Kind::Noun => Words::Noun { sg: g("w0"), pl: g("w1") },
            Kind::NounOf => Words::NounOf { word: format!("{} of", g("w0")) },
            Kind::TrVerb => { let p = g("w2"); Words::TrVerb { third: g("w0"), inf: g("w1"), passive: if p.is_empty() { None } else { Some(format!("{p} by")) } } }
            Kind::TrAdj => Words::TrAdj { word: g("w0") },
            Kind::MainPage => return fail(st, "invalid".into()),
        };
        // checks: valid form, not a function word, distinct, not used by another word
        let forms: Vec<String> = words.all().into_iter().flatten().collect();
        let mut seen = std::collections::BTreeSet::new();
        for x in words.headwords().iter().chain(forms.iter()) {
            let bare = x.trim_start_matches("the ").trim_end_matches(" of").trim_end_matches(" by");
            if !valid.is_match(bare) { return fail(st, format!("This is not a valid word: '{bare}'")); }
            if st.eng.grammar.terminals.contains(bare) || st.eng.grammar.terminals.contains(&bare.to_lowercase()) {
                return fail(st, format!("This is a function word that cannot be redefined: '{bare}'"));
            }
        }
        for x in &forms { seen.insert(x.clone()); }
        if matches!(kind, Kind::Noun | Kind::TrVerb) && seen.len() < forms.iter().filter(|x| !x.is_empty()).collect::<std::collections::BTreeSet<_>>().len() {
            return fail(st, "All word forms must be distinct.".into());
        }
        if kind == Kind::Noun && g("w0") == g("w1") { return fail(st, "Singular and plural form must be different.".into()); }
        if kind == Kind::TrVerb && g("w0") == g("w1") { return fail(st, "Third singular and infinitive must be different.".into()); }
        for x in &forms {
            if let Some(o) = st.eng.wiki.by_word(x) { if Some(o.id) != id { return fail(st, format!("The word '{}' is already used for another word.", x.replace('_', " "))); } }
        }
        let new_id = match id {
            Some(i) => { if let Some(e) = st.eng.wiki.elements.get_mut(&i) { e.words = words; } i }
            None => { let i = st.eng.wiki.next_id(); st.eng.wiki.elements.insert(i, Element { id: i, kind, words, article: vec![] }); i }
        };
        st.eng.wiki.reindex();
        st.eng.invalidate();
        let e = st.eng.wiki.elements[&new_id].clone();
        if let Err(x) = st.storage.save_element(&st.eng.wiki.name, &e) { return fail(st, format!("save failed: {x:#}")); }
        Redirect::to(&format!("/w/{}", back.filter(|_| id.is_none()).map(|_| new_id).unwrap_or(new_id))).into_response()
    }).await
}

async fn delete_element(AxState(w): AxState<Worker>, Path(id): Path<u64>) -> Response {
    w.run(move |st| {
        if st.read_only || id == 0 { return hx_redirect("/"); }
        if st.eng.wiki.elements.values().any(|o| o.id != id && uses_element(o, id)) {
            let body = html! { (err("This word cannot be deleted, because other articles refer to it.")) a href={ "/w/" (id) } { "back" } };
            return html_resp(layout(st, "Delete", body));
        }
        st.eng.wiki.elements.remove(&id);
        st.eng.wiki.reindex();
        st.eng.invalidate();
        let _ = st.storage.delete_element(&st.eng.wiki.name, id);
        hx_redirect("/index")
    }).await
}

// ------------------------------------------------------------------------------------------------
// exports (OntologyExporter implementations)
// ------------------------------------------------------------------------------------------------

async fn export(AxState(w): AxState<Worker>, Path(fmt): Path<String>) -> Response {
    w.run(move |st| {
        let name = st.eng.wiki.name.clone();
        let (ct, ext, body): (&str, &str, String) = match fmt.as_str() {
            "owl" => { let ax = st.eng.ontology_axioms(); ("application/owl+xml", "owl", crate::owl::owlxml_document(&st.eng.uri(), &ax)) }
            "ace" => {
                let mut s = String::new();
                let ids: Vec<u64> = st.eng.wiki.elements.keys().copied().collect();
                for id in ids {
                    let e = st.eng.wiki.elements[&id].clone();
                    if e.article.is_empty() { continue; }
                    s += &format!("# {}\n\n", if e.kind == Kind::MainPage { "Main Page".into() } else { e.heading() });
                    for x in &e.article {
                        match x {
                            Statement::Sentence { serialized, .. } => if let Ok(tes) = st.eng.tokenize(serialized) { s += &st.eng.wiki.sentence_text(&tes); s.push('\n'); },
                            Statement::Comment(c) => { s += &format!("% {}\n", c.replace('\n', "\n% ")); }
                        }
                    }
                    s.push('\n');
                }
                ("text/plain; charset=utf-8", "ace.txt", s)
            }
            "lexicon" => {
                let mut s = String::new();
                for e in st.eng.wiki.elements.values() { for l in e.words.lexicon_entries() { s += &l; s.push('\n'); } }
                ("text/plain; charset=utf-8", "lex.pl", s)
            }
            "acewikidata" => {
                let mut s = String::new();
                for e in st.eng.wiki.elements.values() { s += &format!("{}\n{}\n", e.id, crate::storage::serialize_element(e)); }
                ("text/plain; charset=utf-8", "acewikidata", s)
            }
            _ => return (StatusCode::NOT_FOUND, "formats: owl, ace, lexicon, acewikidata").into_response(),
        };
        let mut h = HeaderMap::new();
        h.insert(header::CONTENT_TYPE, ct.parse().unwrap());
        h.insert(header::CONTENT_DISPOSITION, format!("inline; filename=\"{name}.{ext}\"").parse().unwrap());
        (h, body).into_response()
    }).await
}

// ------------------------------------------------------------------------------------------------
// ACE Editor application
// ------------------------------------------------------------------------------------------------

async fn ace_editor_page(AxState(w): AxState<Worker>) -> Response {
    w.run(|st| {
        let entries = st.ed.entries.clone();
        let body = html! {
            h1 { "ACE Editor" }
            p.muted { "Predictive editor for full ACE texts over the editor lexicon (" (st.ed.lexicon.words.len()) " entries). "
                a href="/aceeditor/lexicon" { "lexicon file" } }
            (ace_editor_fragment(st, vec![], "", None))
            h2 { "Text" }
            div #entries { @for e in &entries { (ace_result_stub(e)) } }
        };
        html_resp(layout(st, "ACE Editor", body))
    }).await
}

fn ace_result_stub(text: &str) -> Markup {
    html! { div.st { span.txt { (text) } span.tools { button hx-post="/aceeditor/analyze" hx-vals=(serde_json::json!({"text": text}).to_string()) hx-target="next .res" { "analyze" } } } div.res {} }
}

fn ace_editor_fragment(st: &mut WikiState, tokens: Vec<String>, filter: &str, msg: Option<String>) -> Markup {
    let mut p = st.ed.parser();
    let mut ok = vec![];
    for t in &tokens { if p.is_possible_next_token(t) { p.add_token(t); ok.push(t.clone()); } else { break; } }
    let complete = p.is_complete() && !ok.is_empty();
    let opts = p.next_token_options().clone();
    let mut groups: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let fl = filter.to_lowercase();
    for c in &opts.con {
        if !fl.is_empty() && !c.word.to_lowercase().starts_with(&fl) { continue; }
        let v = groups.entry(aceeditor::menu_group(c.category.as_ref().map(|x| x.name()))).or_default();
        if !v.contains(&c.word) { v.push(c.word.clone()); }
    }
    for v in groups.values_mut() { v.sort_by_key(|w| w.to_lowercase()); }
    let mut news: BTreeMap<&'static str, Vec<&str>> = BTreeMap::new();
    for a in &opts.abs { let n = a.category_name(); if aceeditor::EXT_CATS.contains(&n) { news.entry(aceeditor::menu_group(Some(n))).or_default().push(aceeditor::EXT_CATS[aceeditor::EXT_CATS.iter().position(|x| *x == n).unwrap()]); } }
    let tj = serde_json::to_string(&ok).unwrap();
    let text = st.ed.text_of(&ok);
    html! {
        div.editor #aed hx-target="#aed" hx-swap="outerHTML" {
            div.text { @if text.is_empty() { span.muted { "Start typing an ACE text…" } } @else { (text) } }
            @if let Some(m) = &msg { (err(m)) }
            form.row hx-post="/aceeditor/editor" {
                input type="hidden" name="tokens" value=(tj); input type="hidden" name="op" value="type";
                input name="filter" value=(filter) autofocus autocomplete="off" placeholder="type words, Enter to add"
                    hx-post="/aceeditor/editor" hx-trigger="keyup changed delay:250ms" hx-vals=r#"{"op":"filter"}"# style="flex:1";
                button.btn.sec type="button" hx-post="/aceeditor/editor" hx-include="closest form" hx-vals=r#"{"op":"del"}"# { "⌫" }
                button.btn.sec type="button" hx-post="/aceeditor/editor" hx-include="closest form" hx-vals=r#"{"op":"clear"}"# { "clear" }
                button.btn type="button" disabled[!complete] hx-post="/aceeditor/editor" hx-include="closest form" hx-vals=r#"{"op":"commit"}"# { "Add to text" }
            }
            div.menus {
                @for (gi, g) in aceeditor::MENU_ORDER.iter().enumerate() {
                    @if groups.contains_key(g) || news.contains_key(g) {
                        div.menu { h4 style={ "background:" (hue_color(gi as u32 * 30)) } { (g) }
                            div.items {
                                @for w in groups.get(g).into_iter().flatten().take(400) {
                                    button hx-post="/aceeditor/editor" hx-vals=(serde_json::json!({"tokens": tj, "op": "add", "tok": w}).to_string()) { (w) }
                                }
                                @if let Some(cats) = news.get(g) {
                                    details { summary.new { "new…" }
                                        form hx-post="/aceeditor/word" {
                                            input type="hidden" name="tokens" value=(tj);
                                            select name="cat" { @for c in cats { option { (c) } } }
                                            input name="form" placeholder="word form" size="10";
                                            input name="lemma" placeholder="lemma (opt.)" size="10";
                                            button.btn.sec { "add" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Deserialize, Default)]
struct AedForm { tokens: Option<String>, op: Option<String>, tok: Option<String>, filter: Option<String> }
async fn ace_editor_post(AxState(w): AxState<Worker>, Form(f): Form<AedForm>) -> Response {
    w.run(move |st| {
        let mut tokens: Vec<String> = f.tokens.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or_default();
        let mut filter = f.filter.clone().unwrap_or_default();
        let mut msg = None;
        match f.op.as_deref() {
            Some("add") => if let Some(t) = f.tok { tokens.push(t); filter.clear(); },
            Some("del") => { tokens.pop(); }
            Some("clear") => tokens.clear(),
            Some("type") => {
                let mut p = st.ed.parser(); p.add_tokens(&tokens);
                match feed_text(&mut p, &filter) { Ok(t) => { tokens.extend(t); filter.clear(); } Err((t, m)) => { tokens.extend(t); msg = Some(m); } }
            }
            Some("commit") => {
                let text = st.ed.text_of(&tokens);
                st.ed.entries.push(text.clone());
                let frag = ace_editor_fragment(st, vec![], "", None);
                return html_resp(html! { (frag) div #entries hx-swap-oob="beforeend" { (ace_result_stub(&text)) } });
            }
            _ => {}
        }
        html_resp(ace_editor_fragment(st, tokens, &filter, msg))
    }).await
}

#[derive(Deserialize)]
struct AedWord { tokens: Option<String>, cat: String, form: String, lemma: Option<String> }
async fn ace_editor_word(AxState(w): AxState<Worker>, Form(f): Form<AedWord>) -> Response {
    w.run(move |st| {
        let tokens: Vec<String> = f.tokens.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or_default();
        let form = norm_word(&f.form);
        let lemma = f.lemma.as_deref().map(norm_word).filter(|x| !x.is_empty()).unwrap_or_else(|| form.clone());
        let valid = regex::Regex::new(r"^[a-zA-Z][a-zA-Z0-9_-]*$").unwrap();
        let msg = if !valid.is_match(&form) || !valid.is_match(&lemma) { Some(format!("This is not a valid word: '{form}'")) }
            else if st.ed.grammar.terminals.contains(&form) { Some(format!("This is a function word that cannot be redefined: '{form}'")) }
            else if st.ed.lexicon.by_text(&form).next().is_some() { Some(format!("This word is already used in the lexicon: '{form}'")) }
            else {
                let q = |s: &str| crate::model::prolog_quote(s);
                let pn = match f.cat.as_str() { "prop_sg" => "pn_sg", "propdef_sg" => "pndef_sg", c => c };
                let entry = match pn {
                    "noun_sg" | "noun_pl" | "pn_sg" | "pndef_sg" => format!("{pn}({}, {}, neutr).", q(&form), q(&lemma)),
                    "adj_tr" | "adj_tr_comp" => format!("{pn}({}, {}, '').", q(&form), q(&lemma)),
                    _ => format!("{pn}({}, {}).", q(&form), q(&lemma)),
                };
                if st.ed.lexicon.add_line(&entry) { st.ed.lexicon_changed(); let _ = st.ed.save_lexicon(); None } else { Some(format!("invalid entry {entry}")) }
            };
        html_resp(ace_editor_fragment(st, tokens, "", msg))
    }).await
}

async fn ace_editor_analyze(AxState(w): AxState<Worker>, Form(f): Form<TextForm>) -> Response {
    w.run(move |st| {
        let WikiState { eng, ed, .. } = st;
        let r = ed.analyze(eng.ape(), &f.text);
        match r {
            Ok(r) => html_resp(ace_result_html(&r)),
            Err(e) => html_resp(err(format!("{e:#}"))),
        }
    }).await
}

fn ace_result_html(r: &EntryResult) -> Markup {
    html! {
        div style="margin:0 0 14px 18px" {
            @if r.error { (err("APE could not parse this text.")) }
            @for m in &r.messages { div.muted { (m.0) " · " (m.1) ": " (m.2) } }
            @for (k, label) in aceeditor::OUTPUTS {
                @if let Some((_, v)) = r.outputs.iter().find(|x| x.0 == *k) { @if !v.trim().is_empty() { details open[*k == "paraphrase"] { summary { (label) } pre { (v) } } } }
            }
        }
    }
}

async fn ace_editor_lexicon(AxState(w): AxState<Worker>) -> Response {
    w.run(|st| ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], st.ed.lexicon.file_content()).into_response()).await
}

// ------------------------------------------------------------------------------------------------
// OpenAI-compatible /v1/chat/completions
// ------------------------------------------------------------------------------------------------

pub const MODELS: &[(&str, &str)] = &[
    ("acewiki-ask", "Answer ACE questions (and check ACE statements) against the wiki's knowledge base"),
    ("acewiki-check", "Analyse an ACE sentence in the wiki vocabulary: paraphrase, OWL, profile and consistency, without storing it"),
    ("aceeditor-parse", "Parse a full ACE text with the ACE Editor lexicon: paraphrase, DRS, FOL, OWL"),
];

async fn models() -> Json<serde_json::Value> {
    Json(serde_json::json!({"object": "list", "data": MODELS.iter().map(|(id, d)| serde_json::json!({"id": id, "object": "model", "created": 0, "owned_by": "acewiki-rs", "description": d})).collect::<Vec<_>>()}))
}

fn last_user_text(v: &serde_json::Value) -> Option<String> {
    let msgs = v.get("messages")?.as_array()?;
    let m = msgs.iter().rev().find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))?;
    match m.get("content")? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(parts) => Some(parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n")),
        _ => None,
    }
}

/// Run one model request on the worker; returns the assistant text.
pub fn chat_answer(st: &mut WikiState, model: &str, text: &str) -> String {
    if model == "aceeditor-parse" {
        let WikiState { eng, ed, .. } = st;
        let r = ed.analyze(eng.ape(), text);
        return match r {
            Ok(r) => {
                let g = |k: &str| r.outputs.iter().find(|x| x.0 == k).map(|x| x.1.trim().to_string()).unwrap_or_default();
                if r.error { format!("APE could not parse the text.\n{}", r.messages.iter().map(|m| format!("- {}: {}", m.1, m.2)).collect::<Vec<_>>().join("\n")) }
                else { format!("Paraphrase:\n{}\n\nDRS:\n{}\n\nFOL:\n{}\n\nOWL:\n{}", g("paraphrase"), g("drspp"), g("fol"), g("owlfsspp")) }
            }
            Err(e) => format!("error: {e:#}"),
        };
    }
    let mut out = vec![];
    for sentence in split_sentences(text) {
        let mut p = st.eng.parser();
        let tokens = match feed_text(&mut p, &sentence) {
            Ok(t) if p.is_complete() => t,
            Ok(t) => { out.push(format!("\"{}\": incomplete sentence — it must end with \".\" or \"?\" and follow the AceWiki grammar (after \"{}\").", sentence.trim(), t.join(" ").replace('_', " "))); continue; }
            Err((_, m)) => { out.push(format!("\"{}\": {m}", sentence.trim())); continue; }
        };
        let tes: Vec<TextElement> = tokens.iter().map(|t| st.eng.wiki.create_text_element(t, &st.eng.grammar.terminals)).collect();
        let ser = st.eng.wiki.serialize(&tes);
        let pretty = st.eng.wiki.sentence_text(&tes).replace('_', " ");
        let a = match st.eng.analyze(&ser) { Ok(a) => a, Err(e) => { out.push(format!("\"{pretty}\": APE error {e:#}")); continue; } };
        if Engine::is_question(&ser) {
            match st.eng.answer(&ser) {
                Ok(Some(ids)) if !ids.is_empty() => {
                    let mut names: Vec<String> = ids.iter().filter_map(|i| st.eng.wiki.elements.get(i)).map(|e| e.heading()).collect();
                    names.sort_by_key(|x| x.to_lowercase());
                    out.push(format!("{pretty}\n{}", names.join(", ")));
                }
                _ => out.push(format!("{pretty}\n(no answer)")),
            }
            if model == "acewiki-check" { out.push(format!("Paraphrase: {}", a.paraphrase.trim())); }
        } else {
            let consistent = if a.reasonable { st.eng.would_be_consistent(&ser).unwrap_or(false) } else { true };
            let mut s = format!("{pretty}\nParaphrase: {}\nOWL: {}  reasonable: {}  consistent with the wiki: {}", a.paraphrase.trim(), if a.is_owl { "yes" } else if a.is_owlswrl { "SWRL" } else { "no" }, a.reasonable, consistent);
            if model == "acewiki-check" {
                if !a.owlfsspp.trim().is_empty() { s += &format!("\n\n{}", a.owlfsspp.trim()); }
                for m in &a.messages { s += &format!("\n- {} {}: {}", m.0, m.1, m.2); }
            }
            out.push(s);
        }
    }
    if out.is_empty() { "Please send one or more ACE sentences or questions (ending with \".\" or \"?\").".into() } else { out.join("\n\n") }
}

fn split_sentences(t: &str) -> Vec<String> {
    let mut v = vec![]; let mut cur = String::new();
    for ch in t.chars() { cur.push(ch); if ch == '.' || ch == '?' { v.push(std::mem::take(&mut cur)); } }
    if !cur.trim().is_empty() { v.push(cur); }
    v
}

async fn chat(AxState(w): AxState<Worker>, Json(v): Json<serde_json::Value>) -> Response {
    let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("acewiki-ask").to_string();
    if !MODELS.iter().any(|m| m.0 == model) {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": {"message": format!("model '{model}' not found; available: {}", MODELS.iter().map(|m| m.0).collect::<Vec<_>>().join(", ")), "type": "invalid_request_error", "code": "model_not_found"}}))).into_response();
    }
    let Some(text) = last_user_text(&v) else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": {"message": "no user message", "type": "invalid_request_error"}}))).into_response();
    };
    let stream = v.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    let m2 = model.clone(); let t2 = text.clone();
    let answer = w.run(move |st| chat_answer(st, &m2, &t2)).await;
    let created = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let id = format!("chatcmpl-{created:x}{:x}", text.len());
    let (pt, ct) = (text.split_whitespace().count(), answer.split_whitespace().count());
    if stream {
        let c1 = serde_json::json!({"id": id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [{"index": 0, "delta": {"role": "assistant", "content": answer}, "finish_reason": null}]});
        let c2 = serde_json::json!({"id": id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]});
        let body = format!("data: {c1}\n\ndata: {c2}\n\ndata: [DONE]\n\n");
        return ([(header::CONTENT_TYPE, "text/event-stream"), (header::CACHE_CONTROL, "no-cache")], body).into_response();
    }
    Json(serde_json::json!({
        "id": id, "object": "chat.completion", "created": created, "model": model,
        "choices": [{"index": 0, "message": {"role": "assistant", "content": answer}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": pt, "completion_tokens": ct, "total_tokens": pt + ct}
    })).into_response()
}

/// Build everything for `acewiki serve`.
pub struct ServeOpts {
    pub data: std::path::PathBuf,
    pub wiki: String,
    pub prolog_dir: std::path::PathBuf,
    pub cfg: crate::engine::Config,
    pub editor_lexicon: Option<std::path::PathBuf>,
    pub read_only: bool,
}

pub fn spawn_worker(o: ServeOpts) -> anyhow::Result<Worker> {
    Worker::spawn(move || {
        let storage = Storage::new(&o.data);
        let wiki = storage.load(&o.wiki)?;
        if !o.data.join(&o.wiki).is_dir() {
            // imported from <wiki>.acewikidata: write the directory form once so later saves are complete
            storage.save_all(&wiki)?;
            eprintln!("imported {} elements from {}.acewikidata", wiki.elements.len(), o.wiki);
        }
        let ape = ape_rs::Ape::new(&o.prolog_dir)?;
        let mut eng = Engine::new(wiki, o.cfg, ape)?;
        eng.set_cache_file(o.data.join(format!("{}.ape-cache.json", o.wiki)));
        let t = std::time::Instant::now();
        eng.load_all()?;
        let consistent = eng.is_consistent();
        eprintln!("loaded wiki '{}' ({} elements) in {:.1}s; consistent={consistent}; reasoner: {}", eng.wiki.name, eng.wiki.elements.len(), t.elapsed().as_secs_f64(), eng.reasoner_note);
        let (src, file) = match &o.editor_lexicon {
            Some(p) => (std::fs::read_to_string(p).unwrap_or_default(), Some(p.clone())),
            None => (include_str!("../../grammar/aceeditor_lexicon.pl").to_string(), None),
        };
        let ed = AceEditor::new(&src, file)?;
        Ok(WikiState { eng, storage, ed, read_only: o.read_only })
    })
}

#[allow(dead_code)]
fn _assert_send() { fn s<T: Send>() {} s::<Worker>(); let _ = Arc::new(0); }
