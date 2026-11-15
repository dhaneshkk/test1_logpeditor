//! Web UI (axum + HTMX) and OpenAI-compatible API.
//!
//! * `/`                    predictive rule editor, live compile panel, rule set with workflow actions
//! * `/rules/{id}`          version history of one rule, with its audit trail
//! * `/review`              approval queue (approvers)
//! * `/audit`               the hash-chained audit log, verified on every view
//! * `/catalog`             what the detectors can see (the vocabulary, named zones, cameras)
//! * `/export/config.json`  APPROVED rules only: detector configuration + FOL, with versions and revision
//! * `/export/rules.p`      TPTP;  `/export/rules.ace` sentences;  `/export/audit.jsonl`;  `/export/attempts.jsonl`
//! * `/v1/models`, `/v1/chat/completions`  OpenAI-compatible (models below)
//! * `/v1/simulate`         run the alert runtime over posted detection frames
//!
//! Identity: with `--users users.json` people log in with a password; without it the editor runs
//! in a flagged demo mode with three password-less identities (author, approver, admin).
//! APE (Scryer) and the chart parser are not `Send`, so they live on one worker thread.

use crate::catalog::Catalog;
use crate::compile::{CompileError, Rule};
use crate::grammar::{self, RuleGrammar};
use crate::runtime::{Engine, Frame};
use crate::store::{self, Role, Status, Store, User, Users};
use crate::{config, parse_lines, Compiler};
use axum::{extract::{Path, Query, State as AxState}, http::{header, HeaderMap, StatusCode}, response::{Html, IntoResponse, Response}, routing::{get, post}, Form, Json, Router};
use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;

const HTMX: &str = include_str!("../../acewiki-rs/static/htmx.min.js");
const COOKIE: &str = "iogp_session";

pub const MODELS: &[(&str, &str)] = &[
    ("iogp-rule-compiler", "controlled-English rules -> JSON detector configuration with time-indexed FOL (TPTP)"),
    ("iogp-rule-explain", "controlled-English rules -> plain-language readback of what will raise each alert"),
    ("iogp-rule-check", "controlled-English rules -> valid / errors with suggestions"),
];

pub struct St {
    pub comp: Compiler,
    pub gram: RuleGrammar,
    pub store: Store,
    compiled: HashMap<(String, u32), Result<Rule, Vec<CompileError>>>,
    users: Option<Users>,
    sessions: HashMap<String, User>,
    attempts: Option<PathBuf>,
    /// n-gram model for suggestion ranking, learned from templates and the rules people wrote
    pub model: crate::suggest::Ngram,
}

impl St {
    /// Re-learn suggestion ranking from templates, every saved version and valid compile attempts.
    pub fn retrain(&mut self) {
        let mut sents: Vec<String> = self.comp.cat.templates.iter().map(|t| t.text.clone()).collect();
        for r in &self.store.rules { for v in &r.versions { sents.push(v.text.clone()); } }
        if let Some(p) = &self.attempts {
            for l in std::fs::read_to_string(p).unwrap_or_default().lines() {
                if let Ok(v) = serde_json::from_str::<Value>(l) { if v["ok"] == true { if let Some(t) = v["text"].as_str() { sents.push(t.to_string()); } } }
            }
        }
        // each distinct sentence counts once, so one person re-checking a sentence cannot dominate
        let mut seen = std::collections::HashSet::new();
        sents.retain(|t| seen.insert(t.trim().to_lowercase()));
        self.model = crate::suggest::train(&self.gram, sents);
    }
    fn cat(&self) -> &Catalog { &self.comp.cat }
    pub fn rule_of(&mut self, id: &str, v: u32) -> Result<Rule, Vec<CompileError>> {
        let key = (id.to_string(), v);
        if let Some(r) = self.compiled.get(&key) { return r.clone(); }
        let text = self.store.get(id).and_then(|r| r.version(v)).map(|x| x.text.clone()).unwrap_or_default();
        let r = self.comp.compile(id, &text);
        self.compiled.insert(key, r.clone());
        r
    }
    /// The approved rules in force (what the detection framework runs).
    pub fn active_rules(&mut self) -> Vec<Rule> {
        let ids: Vec<(String, u32)> = self.store.active().iter().map(|(r, v)| (r.id.clone(), v.v)).collect();
        ids.into_iter().filter_map(|(id, v)| self.rule_of(&id, v).ok()).collect()
    }
    fn user(&self, h: &HeaderMap) -> Option<User> {
        let c = h.get(header::COOKIE)?.to_str().ok()?;
        let tok = c.split(';').map(str::trim).find_map(|kv| kv.strip_prefix(&format!("{COOKIE}=")))?;
        self.sessions.get(tok).cloned()
    }
    fn demo(&self) -> bool { self.users.is_none() }
    /// Every compile attempt of a complete sentence is kept for vocabulary / phrasing analysis.
    fn log_attempt(&self, user: &str, source: &str, text: &str, r: &Result<Rule, Vec<CompileError>>) {
        let Some(p) = &self.attempts else { return };
        let e = json!({"ts": store::now(), "user": user, "source": source, "text": text, "ok": r.is_ok(),
            "errors": r.as_ref().err().map(|es| es.iter().map(|e| json!({"message": e.message, "hint": e.hint})).collect::<Vec<_>>()).unwrap_or_default(),
            "lsr": r.as_ref().ok().map(|x| x.alert.lsr)});
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) { let _ = writeln!(f, "{e}"); }
    }
}

type Job = Box<dyn FnOnce(&mut St) + Send>;
#[derive(Clone)]
pub struct Worker { tx: mpsc::Sender<Job> }
impl Worker {
    pub fn spawn(init: impl FnOnce() -> anyhow::Result<St> + Send + 'static) -> anyhow::Result<Worker> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (rtx, rrx) = mpsc::channel::<Result<(), String>>();
        std::thread::Builder::new().name("iogp-engine".into()).stack_size(256 << 20).spawn(move || {
            let mut st = match init() { Ok(s) => { let _ = rtx.send(Ok(())); s } Err(e) => { let _ = rtx.send(Err(format!("{e:#}"))); return; } };
            for job in rx { job(&mut st); }
        })?;
        rrx.recv()?.map_err(|e| anyhow::anyhow!(e))?;
        Ok(Worker { tx })
    }
    pub async fn run<T: Send + 'static>(&self, f: impl FnOnce(&mut St) -> T + Send + 'static) -> T {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx.send(Box::new(move |s| { let _ = tx.send(f(s)); })).expect("worker gone");
        rx.await.expect("worker dropped job")
    }
}

pub struct ServeOpts { pub cat: Catalog, pub prolog_dir: PathBuf, pub addr: String, pub rules: PathBuf, pub users: Option<PathBuf> }

pub fn spawn_worker(cat: Catalog, prolog_dir: PathBuf, rules_path: PathBuf, users: Option<PathBuf>) -> anyhow::Result<Worker> {
    Worker::spawn(move || {
        let gram = RuleGrammar::new(&cat)?;
        let comp = Compiler::new(cat, ape_rs::Ape::new(&prolog_dir)?);
        let store = Store::open(&rules_path)?;
        let users = match users { Some(p) => Some(Users::load(&p).map_err(|e| anyhow::anyhow!("users file {}: {e}", p.display()))?), None => None };
        let stem = rules_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("rules".into());
        let attempts = Some(rules_path.with_file_name(format!("{stem}.attempts.jsonl")));
        let mut st = St { comp, gram, store, compiled: HashMap::new(), users, sessions: HashMap::new(), attempts, model: Default::default() };
        st.retrain();
        Ok(st)
    })
}

pub fn router(w: Worker) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/login", get(login_page).post(login_post))
        .route("/logout", post(logout))
        .route("/suggest", post(suggest_post))
        .route("/panel", post(panel_post))
        .route("/compile", post(compile_post))
        .route("/rules", post(rule_save))
        .route("/rules/{id}", get(rule_page))
        .route("/rules/{id}/retire", post(rule_retire))
        .route("/rules/{id}/{v}/{action}", post(rule_action))
        .route("/review", get(review_page))
        .route("/audit", get(audit_page))
        .route("/analysis", get(analysis_page))
        .route("/catalog", get(catalog_page))
        .route("/grammar.json", get(grammar_json))
        .route("/export/{file}", get(export))
        .route("/static/htmx.min.js", get(|| async { ([(header::CONTENT_TYPE, "application/javascript")], HTMX) }))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat))
        .route("/v1/simulate", post(simulate))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(w)
}

pub async fn serve(o: ServeOpts) -> anyhow::Result<()> {
    let w = spawn_worker(o.cat, o.prolog_dir, o.rules, o.users.clone())?;
    let l = tokio::net::TcpListener::bind(&o.addr).await?;
    eprintln!("iogp-editor on http://{}  (OpenAI-compatible: http://{}/v1/chat/completions){}", o.addr, o.addr,
        if o.users.is_none() { "\n  DEMO MODE: no --users file, anyone can log in as author-demo / approver-demo / admin-demo" } else { "" });
    axum::serve(l, router(w)).await?;
    Ok(())
}

const CSS: &str = r#"
:root{--ink:#1d2430;--mute:#667085;--line:#e3e6ec;--bg:#f7f8fa;--card:#fff;--acc:#b4441c;--ok:#1f7a4d;--bad:#b42318;--chip:#eef1f6}
*{box-sizing:border-box}body{margin:0;font:15px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif;color:var(--ink);background:var(--bg)}
header{background:#14181f;color:#fff;padding:10px 24px;display:flex;gap:24px;align-items:center}
header b{font-size:16px;letter-spacing:.3px}header a{color:#cfd6e4;text-decoration:none;font-size:14px}header a:hover{color:#fff}
main{max-width:1280px;margin:0 auto;padding:20px 24px 60px}
h1{font-size:22px;margin:4px 0 4px}h2{font-size:17px;margin:28px 0 10px}h4{margin:0;font-size:12px;text-transform:uppercase;letter-spacing:.5px;color:#fff;padding:4px 8px;border-radius:6px 6px 0 0}
.muted{color:var(--mute)}.grid{display:grid;grid-template-columns:minmax(0,1.15fr) minmax(0,1fr);gap:20px}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:16px}
.sentence{font-size:18px;min-height:56px;padding:12px 14px;border:1px solid var(--line);border-radius:8px;background:#fbfbfc;margin-bottom:10px}
.sentence .w{white-space:nowrap}.row{display:flex;gap:8px;align-items:center;margin:8px 0}
input,textarea,select{font:inherit;padding:7px 9px;border:1px solid #cfd4dc;border-radius:7px;background:#fff}textarea{width:100%;min-height:70px}
button,.btn{font:inherit;font-size:14px;padding:6px 12px;border-radius:7px;border:1px solid var(--acc);background:var(--acc);color:#fff;cursor:pointer;text-decoration:none;display:inline-block}
button.sec,.btn.sec{background:#fff;color:var(--ink);border-color:#cfd4dc}button:disabled{opacity:.45;cursor:default}
.menus{display:flex;flex-wrap:wrap;gap:10px;margin-top:10px}.menu{border:1px solid var(--line);border-radius:7px;min-width:150px;max-width:100%}
.menu .items{display:flex;flex-wrap:wrap;gap:4px;padding:6px;max-height:190px;overflow:auto}
.menu .items button{background:var(--chip);color:var(--ink);border:0;padding:3px 8px;font-size:13.5px}.menu .items button:hover{background:#dde3ee}
.tpl{display:flex;flex-direction:column;gap:6px}.tpl button{text-align:left;background:#fff;color:var(--ink);border-color:var(--line);font-size:13.5px}
.tpl button b{color:var(--acc);margin-right:6px}
.msg{padding:8px 10px;border-radius:7px;margin:8px 0}.err{background:#fef3f2;color:var(--bad);border:1px solid #fecdca}.okm{background:#ecfdf3;color:var(--ok);border:1px solid #abefc6}
.readback{font-size:15.5px}.readback li{margin:3px 0}.lsr{display:inline-block;background:var(--acc);color:#fff;border-radius:5px;padding:1px 7px;font-size:12.5px;margin-right:6px}
pre{white-space:pre-wrap;word-break:break-word;background:#10141b;color:#e6edf3;padding:10px 12px;border-radius:8px;overflow:auto;font-size:12.5px;max-height:340px}
details summary{cursor:pointer;color:var(--mute);margin:6px 0}
table{border-collapse:collapse;width:100%}td,th{border-bottom:1px solid var(--line);padding:8px 6px;vertical-align:top;text-align:left;font-size:14px}th{font-size:12px;text-transform:uppercase;color:var(--mute)}
.status{font-weight:600}.status.ok{color:var(--ok)}.status.bad{color:var(--bad)}.small{font-size:13px}
.chip{display:inline-block;border-radius:10px;padding:1px 8px;font-size:12px;font-weight:600}.chip.ok{background:#dcfae6;color:#085d3a}.chip.wait{background:#fef0c7;color:#93370d}.chip.bad{background:#fee4e2;color:#912018}.chip.draft{background:#eef1f6;color:#344054}.chip.old{background:#f2f4f7;color:#98a2b3}
.asp{border-top:1px dashed var(--line);margin-top:8px;padding-top:6px}
.demo{background:#fffaeb;color:#93370d;border-bottom:1px solid #fedf89;padding:6px 24px;font-size:13px}
.ac{position:relative}.ac-wrap{position:relative}
#rule,#ac-mirror{font:17px/1.55 system-ui,-apple-system,Segoe UI,Roboto,sans-serif;padding:12px 14px;border-radius:8px;width:100%;min-height:86px;white-space:pre-wrap;word-wrap:break-word;letter-spacing:normal}
#rule{position:relative;background:transparent;border:1px solid #cfd4dc;resize:vertical;z-index:2;color:var(--ink)}
#rule:focus{outline:2px solid #f3b49b;border-color:var(--acc)}
#ac-mirror{position:absolute;inset:0;border:1px solid transparent;color:transparent;z-index:1;background:#fbfbfc;overflow:hidden}
#ac-mirror .gh{color:#9aa3b2}#ac-mirror .bad{text-decoration:underline wavy #d92d20;text-underline-offset:4px}
#ac-list{list-style:none;margin:4px 0 0;padding:4px;position:absolute;left:0;top:100%;z-index:10;background:#fff;border:1px solid var(--line);border-radius:8px;box-shadow:0 8px 24px rgba(16,24,40,.12);max-height:280px;overflow:auto;min-width:320px;display:none}
#ac-list li{padding:5px 9px;border-radius:6px;cursor:pointer;display:flex;gap:10px;align-items:baseline}
#ac-list li.sel,#ac-list li:hover{background:#fdf0ea}#ac-list li .g{margin-left:auto;font-size:11.5px;color:var(--mute)}
#ac-list li b{color:var(--acc)}#ac-list li .fx{font-size:11.5px;color:#b42318}
#ac-list li.more{color:var(--mute);font-size:12.5px;cursor:default}
#ac-list li.head{background:#fef3f2;color:#b42318;font-size:13px;cursor:default;border-radius:6px;margin-bottom:3px}
#ac-msg:not(:empty){padding:8px 10px;border-radius:7px;margin:8px 0;background:#fef3f2;color:var(--bad);border:1px solid #fecdca}
kbd{font:11.5px ui-monospace,monospace;border:1px solid #cfd4dc;border-bottom-width:2px;border-radius:4px;padding:0 4px;background:#fff}
@media (max-width:900px){.grid{grid-template-columns:1fr}}
"#;

fn layout(title: &str, user: Option<&User>, demo: bool, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" { head {
            meta charset="utf-8"; meta name="viewport" content="width=device-width, initial-scale=1";
            title { (title) " · IOGP rule editor" }
            style { (PreEscaped(CSS)) }
            script src="/static/htmx.min.js" {}
        }
        body {
            header { b { "IOGP Life-Saving Rule alerts" } a href="/" { "Editor" } a href="/review" { "Review" } a href="/audit" { "Audit log" } a href="/analysis" { "Analysis" } a href="/catalog" { "Vocabulary" }
                a href="/export/config.json" { "config.json" } a href="/export/rules.p" { "rules.p (FOL)" } a href="/export/rules.lp" { "rules.lp (ASP)" }
                span style="margin-left:auto" {}
                @if let Some(u) = user {
                    span.small { (u.name) " · " (roles_label(u)) }
                    form hx-post="/logout" style="margin:0" { button.sec style="padding:3px 9px" { "Log out" } }
                }
            }
            @if demo { div.demo { "Demo mode — no users file: anyone can act as any identity. Start with " code { "--users users.json" } " for real sign-off." } }
            main { (body) }
        } }
    }
}
fn roles_label(u: &User) -> String { u.roles.iter().map(|r| match r { Role::Author => "author", Role::Approver => "approver", Role::Admin => "admin" }).collect::<Vec<_>>().join(", ") }
fn page(m: Markup) -> Response { Html(m.into_string()).into_response() }
fn frag(m: Markup) -> Response { Html(m.into_string()).into_response() }
fn to_login() -> Response { ([(header::LOCATION, "/login")], StatusCode::SEE_OTHER).into_response() }
fn hx_redirect(to: &str) -> Response { ([("HX-Redirect", to.to_string())], "").into_response() }
fn enc(s: &str) -> String { s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect() }
fn flash(msg: &str) -> String { format!("/?msg={}", enc(msg)) }

fn lsr_label(r: &Rule) -> String { format!("LSR {} · {}", r.alert.lsr, r.alert.rule) }

fn errors_html(es: &[CompileError]) -> Markup {
    html! { @for e in es { div.msg.err { b { (e.message) } @if !e.hint.is_empty() { br; span.small { (e.hint) } } } } }
}

fn status_chip(s: Status) -> Markup {
    let cls = match s { Status::Approved => "chip ok", Status::Submitted => "chip wait", Status::Rejected => "chip bad", Status::Draft => "chip draft", _ => "chip old" };
    html! { span class=(cls) { (s.label()) } }
}

fn result_html(cat: &Catalog, r: &Result<Rule, Vec<CompileError>>) -> Markup {
    match r {
        Err(es) => html! { h2 { "Not a valid alert rule yet" } (errors_html(es)) },
        Ok(r) => {
            let cfg = serde_json::to_string_pretty(&config::rule_json(r)).unwrap();
            html! {
                div.msg.okm { "Valid rule — " (cat.agent) " can apply it once it is approved." }
                p { span.lsr { (lsr_label(r)) } }
                ul.readback { @for (i, l) in r.readback.iter().enumerate() { @if i == 0 { li style="list-style:none;margin-left:-18px" { b { (l) } } } @else { li { (l) } } } }
                @for w in &r.warnings { div.msg.err.small { (w) } }
                p.small.muted { "APE paraphrase: " (r.paraphrase) }
                details open { summary { "First-order logic (TPTP) — what the detection framework evaluates" } pre { (r.fol) } }
                details { summary { "Detector configuration (JSON)" } pre { (cfg) } }
                details { summary { "APE's own FOL translation" } pre { (r.ape_tptp) } }
            }
        }
    }
}

// ---------------------------------------------------------------- login

async fn login_page(AxState(w): AxState<Worker>, Query(q): Query<MsgQ>) -> Response {
    w.run(move |st| {
        let demo = st.demo();
        let body = html! {
            div.card style="max-width:460px;margin:40px auto" {
                h1 { "Sign in" }
                @if let Some(m) = &q.msg { div.msg.err { (m) } }
                @if demo {
                    p.muted { "Demo mode: pick an identity. Authors write and submit rules; approvers sign them off (never their own)." }
                    @for u in Users::demo() {
                        form method="post" action="/login" class="row" { input type="hidden" name="name" value=(u.name); button style="width:100%" { (u.name) " — " (roles_label(&u)) } }
                    }
                } @else {
                    form method="post" action="/login" {
                        div.row { input name="name" placeholder="user name" autofocus style="flex:1"; }
                        div.row { input name="password" type="password" placeholder="password" style="flex:1"; }
                        div.row { button { "Sign in" } }
                    }
                }
            }
        };
        page(layout("Sign in", None, demo, body))
    }).await
}

#[derive(Deserialize)]
struct LoginForm { name: String, password: Option<String> }
async fn login_post(AxState(w): AxState<Worker>, Form(f): Form<LoginForm>) -> Response {
    w.run(move |st| {
        let user = match &st.users {
            None => Users::demo().into_iter().find(|u| u.name == f.name),
            Some(us) => us.check(&f.name, f.password.as_deref().unwrap_or("")),
        };
        match user {
            Some(u) => {
                let tok = store::random_hex(24);
                st.sessions.insert(tok.clone(), u);
                ([(header::SET_COOKIE, format!("{COOKIE}={tok}; Path=/; HttpOnly; SameSite=Strict")), (header::LOCATION, "/".to_string())], StatusCode::SEE_OTHER).into_response()
            }
            None => ([(header::LOCATION, "/login?msg=wrong%20user%20name%20or%20password")], StatusCode::SEE_OTHER).into_response(),
        }
    }).await
}

async fn logout(AxState(w): AxState<Worker>, h: HeaderMap) -> Response {
    w.run(move |st| {
        if let Some(c) = h.get(header::COOKIE).and_then(|c| c.to_str().ok()) {
            for kv in c.split(';').map(str::trim) { if let Some(t) = kv.strip_prefix(&format!("{COOKIE}=")) { st.sessions.remove(t); } }
        }
        ([(header::SET_COOKIE, format!("{COOKIE}=; Path=/; Max-Age=0")), (header::HeaderName::from_static("hx-redirect"), "/login".to_string())], "").into_response()
    }).await
}

// ---------------------------------------------------------------- editor

const EDITOR_JS: &str = include_str!("editor.js");

fn editor(st: &mut St, user: &User, text: &str, id: &str) -> Markup {
    let cat = st.cat().clone();
    html! {
        div #ed data-id=(id) {
            div.grid {
                div.card {
                    div.ac {
                        div.ac-wrap {
                            div #ac-mirror aria-hidden="true" {}
                            textarea #rule rows="3" spellcheck="false" autocomplete="off" autofocus
                                placeholder="Start typing — e.g. If a worker is in a height-zone …  (Tab accepts the grey suggestion, ↑↓ Enter pick a word, Ctrl+Space shows all)" { (text) }
                        }
                        ul #ac-list role="listbox" {}
                    }
                    div #ac-msg {}
                    div.row.small.muted { span #ac-status { "" } span style="margin-left:auto" {}
                        button.sec type="button" #ac-clear { "Clear" } }
                    details #ac-browse { summary { "Browse the words that can come next" } div #ac-menus .menus {} }
                    div #ac-result {}
                }
                div.card {
                    h2 style="margin-top:0" { "Start from an IOGP Life-Saving Rule" }
                    div.tpl { @for t in &cat.templates {
                        button type="button" data-text=(t.text) .tplbtn { b { (t.lsr) } (t.text) }
                    } }
                    p.small.muted { "Scope to a named zone (" i { "in Rig-Floor-East" } ") or a camera (" i { "a height-zone of CAM-03" } "); exceptions: " i { "… unless the restricted-area has a permit" } ". You can also paste a sentence into the box." }
                    @if !user.can(Role::Author) { p.small.muted { "Your role can check rules but not save them (author role needed)." } }
                }
            }
        }
        script { (PreEscaped(EDITOR_JS)) }
    }
}

#[derive(Deserialize)]
struct SuggestReq { text: String }

async fn suggest_post(AxState(w): AxState<Worker>, Json(r): Json<SuggestReq>) -> Response {
    w.run(move |st| Json(crate::suggest::suggest(&st.gram, &st.comp.cat, &st.model, &r.text)).into_response()).await
}

#[derive(Deserialize)]
struct PanelForm { text: String, id: Option<String> }

/// Compile panel for a complete sentence: readback, FOL, JSON, errors, and the save form.
async fn panel_post(AxState(w): AxState<Worker>, h: HeaderMap, Form(f): Form<PanelForm>) -> Response {
    w.run(move |st| {
        let Some(user) = st.user(&h) else { return hx_redirect("/login") };
        let id = f.id.unwrap_or_default();
        let text = f.text.trim().to_string();
        let r = st.comp.compile(if id.is_empty() { "draft" } else { &id }, &text);
        st.log_attempt(&user.name, "editor", &text, &r);
        let can_save = r.is_ok() && user.can(Role::Author);
        let cat = st.cat().clone();
        frag(html! {
            (result_html(&cat, &r))
            @if user.can(Role::Author) {
                form hx-post="/rules" hx-target="body" hx-swap="outerHTML" {
                    input type="hidden" name="text" value=(text);
                    div.row { label.small style="width:90px" { "Rule id" } input name="id" value=(id) placeholder="auto" size="10"; }
                    div.row { label.small style="width:90px" { "Intent" } input name="intent" placeholder="what this rule should catch, in your own words (optional)" style="flex:1"; }
                    div.row {
                        button.sec name="op" value="draft" disabled[!can_save] { "Save draft" }
                        button name="op" value="submit" disabled[!can_save] { "Save & submit for approval" }
                    }
                }
            }
        })
    }).await
}

#[derive(Deserialize)]
struct IndexQ { edit: Option<String>, msg: Option<String> }
#[derive(Deserialize)]
struct MsgQ { msg: Option<String> }

async fn index(AxState(w): AxState<Worker>, h: HeaderMap, Query(q): Query<IndexQ>) -> Response {
    w.run(move |st| {
        let Some(user) = st.user(&h) else { return to_login() };
        let (id, text) = q.edit.as_ref().and_then(|e| st.store.get(e)).map(|r| (r.id.clone(), r.latest().text.clone())).unwrap_or_default();
        let ed = editor(st, &user, &text, &id);
        let table = rules_table(st, &user);
        let n_active = st.store.active().len();
        let pending = st.store.rules.iter().filter(|r| r.latest().status == Status::Submitted).count();
        let rev = st.store.revision();
        let body = html! {
            h1 { "Alert rules in controlled English" }
            p.muted { "Write each alert as one sentence. The editor only offers words " (st.cat().agent) "'s detectors understand; "
                "APE (Attempto Parsing Engine, via ape-rs) checks the sentence; it is compiled to first-order logic and a detector configuration. "
                "Rules go live only after a second person approves them." }
            @if let Some(m) = &q.msg { div.msg.okm { (m) } }
            (ed)
            h2 { "Rule set " span.muted.small { "(" (n_active) " approved and live · " (pending) " awaiting approval · deployed revision " code { (&rev[..12]) } ")" } }
            div.card { (table) }
        };
        page(layout("Editor", Some(&user), st.demo(), body))
    }).await
}

fn rules_table(st: &mut St, user: &User) -> Markup {
    let rows: Vec<(String, Option<store::Version>, store::Version)> = st.store.rules.iter().map(|r| (r.id.clone(), r.active_version().cloned(), r.latest().clone())).collect();
    let mut compiled: HashMap<String, Result<Rule, Vec<CompileError>>> = HashMap::new();
    for (id, a, l) in &rows { let v = a.as_ref().map(|x| x.v).unwrap_or(l.v); compiled.insert(id.clone(), st.rule_of(id, v)); }
    html! {
        @if rows.is_empty() { p.muted { "No rules yet. Build one above and press " i { "Save & submit for approval" } "." } }
        @else { table {
            tr { th { "id" } th { "rule in force / latest" } th { "status" } th {} }
            @for (id, active, latest) in &rows {
                tr {
                    td { a href={ "/rules/" (id) } { b { (id) } } }
                    td {
                        @match &active { Some(a) => { (a.text) } None => { span.muted { (latest.text) } } }
                        @if let Ok(x) = &compiled[id] { div.small.muted { span.lsr { (lsr_label(x)) } (x.readback[1..].join("; ")) } }
                        @if let Some(a) = &active { @if latest.v != a.v { div.small style="margin-top:4px" { "pending v" (latest.v) ": " i { (latest.text) } } } }
                    }
                    td style="white-space:nowrap" {
                        @if let Some(a) = &active { span.chip.ok { "live v" (a.v) } br; }
                        @if active.as_ref().map(|a| a.v) != Some(latest.v) { "v" (latest.v) " " (status_chip(latest.status)) }
                        @if latest.status == Status::Rejected { div.small style="max-width:220px;white-space:normal" { "“" (latest.note) "” — " (latest.reviewer.clone().unwrap_or_default()) } }
                    }
                    td style="white-space:nowrap" { (actions(id, active.as_ref(), latest, user)) }
                }
            }
        } }
    }
}

fn actions(id: &str, active: Option<&store::Version>, latest: &store::Version, user: &User) -> Markup {
    let can_review = user.can(Role::Approver) && latest.author != user.name;
    html! {
        @if user.can(Role::Author) { a.btn.sec href={ "/?edit=" (id) } { "Edit" } " " }
        @if latest.status == Status::Draft && (latest.author == user.name || user.can(Role::Admin)) {
            button hx-post={ "/rules/" (id) "/" (latest.v) "/submit" } hx-target="body" { "Submit" } " "
        }
        @if latest.status == Status::Submitted && can_review {
            button hx-post={ "/rules/" (id) "/" (latest.v) "/approve" } hx-target="body" hx-prompt="Approval note (optional)" { "Approve" } " "
            button.sec hx-post={ "/rules/" (id) "/" (latest.v) "/reject" } hx-target="body" hx-prompt="Why is it rejected? (the author sees this)" { "Reject" } " "
        }
        @if latest.status == Status::Submitted && user.can(Role::Approver) && latest.author == user.name { span.small.muted { "needs another approver" } }
        @if active.is_some() && user.can(Role::Approver) {
            button.sec hx-post={ "/rules/" (id) "/retire" } hx-target="body" hx-prompt="Why is the rule retired?" { "Retire" }
        }
    }
}

#[derive(Deserialize)]
struct SaveForm { id: Option<String>, text: String, intent: Option<String>, op: Option<String> }

async fn rule_save(AxState(w): AxState<Worker>, h: HeaderMap, Form(f): Form<SaveForm>) -> Response {
    w.run(move |st| {
        let Some(user) = st.user(&h) else { return hx_redirect("/login") };
        let id: String = f.id.unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric() || "_-.".contains(*c)).collect();
        let id = if id.is_empty() { st.store.next_id() } else { id };
        let res = st.comp.compile(&id, &f.text);
        st.log_attempt(&user.name, "save", &f.text, &res);
        if let Err(es) = res { return hx_redirect(&flash(&format!("not saved: {}", es[0].message))); }
        let saved = st.store.save_draft(&user, &id, &f.text, f.intent.as_deref().unwrap_or("").trim());
        if saved.is_ok() { st.retrain(); }
        let msg = match saved {
            Err(e) => format!("not saved: {e}"),
            Ok(v) => {
                if f.op.as_deref() == Some("submit") {
                    match st.store.submit(&user, &id, v) { Ok(()) => format!("{id} v{v} submitted — it goes live when an approver signs it off"), Err(e) => format!("{id} v{v} saved as draft; {e}") }
                } else { format!("{id} v{v} saved as draft") }
            }
        };
        hx_redirect(&flash(&msg))
    }).await
}

async fn rule_action(AxState(w): AxState<Worker>, h: HeaderMap, Path((id, v, action)): Path<(String, u32, String)>) -> Response {
    let note = h.get("HX-Prompt").and_then(|x| x.to_str().ok()).unwrap_or("").to_string();
    w.run(move |st| {
        let Some(user) = st.user(&h) else { return hx_redirect("/login") };
        // approval requires the version to compile with the current catalog
        if action == "approve" { if let Err(es) = st.rule_of(&id, v) { return hx_redirect(&flash(&format!("{id} v{v} cannot be approved: {}", es[0].message))); } }
        let r = match action.as_str() {
            "submit" => st.store.submit(&user, &id, v),
            "approve" => st.store.approve(&user, &id, v, &note),
            "reject" => st.store.reject(&user, &id, v, &note),
            _ => return (StatusCode::NOT_FOUND, "unknown action").into_response(),
        };
        let back = h.get("HX-Current-URL").and_then(|x| x.to_str().ok()).filter(|u| u.contains("/review")).map(|_| "/review").unwrap_or("/");
        let done = match action.as_str() { "submit" => "submitted", "approve" => "approved — now live", _ => "rejected" };
        let msg = match r { Ok(()) => format!("{id} v{v} {done}"), Err(e) => e.to_string() };
        hx_redirect(&format!("{}?msg={}", back, enc(&msg)))
    }).await
}

async fn rule_retire(AxState(w): AxState<Worker>, h: HeaderMap, Path(id): Path<String>) -> Response {
    let note = h.get("HX-Prompt").and_then(|x| x.to_str().ok()).unwrap_or("").to_string();
    w.run(move |st| {
        let Some(user) = st.user(&h) else { return hx_redirect("/login") };
        let msg = match st.store.retire(&user, &id, &note) { Ok(()) => format!("{id} retired"), Err(e) => e.to_string() };
        hx_redirect(&flash(&msg))
    }).await
}

async fn rule_page(AxState(w): AxState<Worker>, h: HeaderMap, Path(id): Path<String>) -> Response {
    w.run(move |st| {
        let Some(user) = st.user(&h) else { return to_login() };
        let Some(rec) = st.store.get(&id).cloned() else { return (StatusCode::NOT_FOUND, "no such rule").into_response() };
        let audit: Vec<store::AuditEntry> = st.store.audit_entries().into_iter().filter(|e| e.rule == id).collect();
        let mut compiled = BTreeMap::new();
        for v in &rec.versions { compiled.insert(v.v, st.rule_of(&id, v.v)); }
        let body = html! {
            h1 { "Rule " (id) }
            p.muted { @match rec.active { Some(v) => { "Version " (v) " is live." } None => { "No version is live." } } }
            @for v in rec.versions.iter().rev() {
                div.card style="margin-bottom:12px" {
                    div.row { b { "v" (v.v) } (status_chip(v.status)) span.small.muted { "written by " (v.author) " · " (v.created)
                        @if let Some(s) = &v.submitted { " · submitted " (s) }
                        @if let Some(r) = &v.reviewer { " · reviewed by " (r) " " (v.reviewed.clone().unwrap_or_default()) } } }
                    p { (v.text) }
                    @if !v.intent.is_empty() { p.small { "Intent: " i { (v.intent) } } }
                    @if !v.note.is_empty() { p.small { "Reviewer note: “" (v.note) "”" } }
                    @match &compiled[&v.v] {
                        Ok(r) => { ul.readback.small { @for l in &r.readback { li { (l) } } } details { summary { "FOL" } pre { (r.fol) } } }
                        Err(es) => { div.small { "does not compile with the current vocabulary:" } (errors_html(es)) }
                    }
                    p.small.muted { "text SHA-256 " code { (v.text_sha256) } }
                    @if rec.latest().v == v.v { (actions(&id, rec.active_version(), v, &user)) }
                }
            }
            h2 { "Audit trail" }
            div.card { (audit_table(&audit)) }
        };
        page(layout(&format!("Rule {id}"), Some(&user), st.demo(), body))
    }).await
}

async fn review_page(AxState(w): AxState<Worker>, h: HeaderMap, Query(q): Query<MsgQ>) -> Response {
    w.run(move |st| {
        let Some(user) = st.user(&h) else { return to_login() };
        let pending: Vec<(String, Option<store::Version>, store::Version)> = st.store.rules.iter().filter(|r| r.latest().status == Status::Submitted)
            .map(|r| (r.id.clone(), r.active_version().cloned(), r.latest().clone())).collect();
        let mut compiled = HashMap::new();
        for (id, _, l) in &pending { compiled.insert(id.clone(), st.rule_of(id, l.v)); }
        let live = st.active_rules();
        let cat = st.cat().clone();
        let mut analysis = HashMap::new();
        for (id, _, _) in &pending { if let Ok(r) = &compiled[id] { analysis.insert(id.clone(), rule_analysis(&cat, r, &live)); } }
        let body = html! {
            h1 { "Awaiting approval" }
            p.muted { "Approvers sign off rules written by someone else. Approving makes the version live in config.json; the decision and its note are recorded in the audit log." }
            @if let Some(m) = &q.msg { div.msg.okm { (m) } }
            @if pending.is_empty() { div.card { p.muted { "Nothing to review." } } }
            @for (id, active, v) in &pending {
                div.card style="margin-bottom:12px" {
                    div.row { a href={ "/rules/" (id) } { b { (id) " v" (v.v) } } span.small.muted { "by " (v.author) " · submitted " (v.submitted.clone().unwrap_or_default()) } }
                    p { (v.text) }
                    @if !v.intent.is_empty() { p.small { "Intent: " i { (v.intent) } } }
                    @if let Some(a) = active { p.small.muted { "replaces live v" (a.v) ": " i { (a.text) } } }
                    @match &compiled[id] { Ok(r) => { ul.readback { @for l in &r.readback { li { (l) } } } details { summary { "FOL" } pre { (r.fol) } } } Err(es) => { (errors_html(es)) } }
                    @if let Some(a) = analysis.get(id) { (a) }
                    (actions(id, active.as_ref(), v, &user))
                }
            }
        };
        page(layout("Review", Some(&user), st.demo(), body))
    }).await
}

fn audit_table(es: &[store::AuditEntry]) -> Markup {
    html! { table.small {
        tr { th { "#" } th { "time (UTC)" } th { "user" } th { "action" } th { "rule" } th { "detail" } th { "hash" } }
        @for e in es.iter().rev() {
            tr { td { (e.seq) } td { (e.ts) } td { (e.user) } td { b { (e.action) } } td { a href={ "/rules/" (e.rule) } { (e.rule) } @if let Some(v) = e.version { " v" (v) } }
                 td style="max-width:420px" { (e.detail) } td { code { (&e.hash[..10]) } } }
        }
    } }
}

async fn audit_page(AxState(w): AxState<Worker>, h: HeaderMap) -> Response {
    w.run(move |st| {
        let Some(user) = st.user(&h) else { return to_login() };
        let check = st.store.audit_file().map(store::verify_audit);
        let es = st.store.audit_entries();
        let body = html! {
            h1 { "Audit log" }
            @match &check {
                Some(Ok((n, head))) => { div.msg.okm { "Chain verified: " (n) " entries, each linked to the previous by SHA-256. Head " code { (head) } } }
                Some(Err(e)) => { div.msg.err { b { "AUDIT LOG BROKEN: " } (e) } }
                None => { div.msg.err { "No audit file (in-memory store)." } }
            }
            p.small.muted { "Download: " a href="/export/audit.jsonl" { "audit.jsonl" } " · verify offline with " code { "iogp audit verify <file>" } }
            div.card { (audit_table(&es)) }
        };
        page(layout("Audit log", Some(&user), st.demo(), body))
    }).await
}

#[derive(Deserialize)]
struct TextForm { text: String }
async fn compile_post(AxState(w): AxState<Worker>, Form(f): Form<TextForm>) -> Response {
    w.run(move |st| { let r = st.comp.compile("draft", &f.text); let c = st.cat().clone(); frag(result_html(&c, &r)) }).await
}

async fn catalog_page(AxState(w): AxState<Worker>, h: HeaderMap) -> Response {
    w.run(move |st| {
        let user = st.user(&h);
        let c = st.cat().clone();
        let body = html! {
            h1 { "Vocabulary" }
            p.muted { "Every content word a rule can use, and what the detection framework observes for it. Edit the catalog JSON (" code { "--catalog" } ") to match the deployed detectors and the zones drawn per camera." }
            div.grid {
                div.card { h2 style="margin-top:0" { "Detected objects" } table { tr { th { "noun" } th { "plural" } th { "detector label" } }
                    @for x in &c.classes { tr { td { b { (x.noun) } } td { (x.plural) } td { code { (x.detector) } } } } } }
                div.card {
                    h2 style="margin-top:0" { "Zones (drawn per camera)" } p { @for z in &c.zones { span.lsr style="background:#3b5b7a" { (z.noun) } " " } }
                    @if !c.zone_instances.is_empty() {
                        h2 { "Named zones" } table { tr { th { "name" } th { "zone type" } th { "camera" } }
                            @for z in &c.zone_instances { tr { td { b { (z.name) } } td { (z.zone) } td { (z.camera.clone().unwrap_or_default()) } } } }
                    }
                    @if !c.cameras.is_empty() {
                        h2 { "Cameras" } p.small.muted { "Scope a zone to a camera: “a height-zone of CAM-03”." }
                        table { @for k in &c.cameras { tr { td { b { (k.name) } } td { (k.description) } } } }
                    }
                    h2 { "Site signals" } p.small.muted { "Exceptions: “… unless the restricted-area has a permit”." } table { @for s in &c.signals { tr { td { b { (s.noun) } } td { (s.source) } } } }
                    h2 { "Positions" } table { @for s in &c.spatial { tr { td { b { "is " (s.prep) } } td { code { (s.detector) } @if let Some(m) = s.default_metres { " (" (m) " m)" } } } } }
                    h2 { "Observed relations" } table { @for r in &c.relations { tr { td { b { (r.third) } } td { code { (r.detector) } } td.small { (r.object_kinds.join(", ")) } } } }
                }
            }
            div.card style="margin-top:20px" { h2 style="margin-top:0" { "Alerts — IOGP Life-Saving Rules" } table { tr { th { "LSR" } th { "alert noun" } th { "rule" } }
                @for a in &c.alerts { tr { td { (a.lsr) } td { b { (a.noun) } } td { (a.rule) } } } }
                p.small.muted { "Severities: " (c.severities.join(", ")) ". Grace " (c.defaults.grace_seconds) " s, cooldown " (c.defaults.cooldown_seconds) " s." } }
        };
        page(layout("Vocabulary", user.as_ref(), st.demo(), body))
    }).await
}

async fn grammar_json(AxState(w): AxState<Worker>) -> Response {
    w.run(|st| Json(grammar::grammar_json(st.cat())).into_response()).await
}

async fn export(AxState(w): AxState<Worker>, Path(file): Path<String>) -> Response {
    w.run(move |st| {
        let rules = st.active_rules();
        match file.as_str() {
            "config.json" => {
                let mut v = config::ruleset_json(st.cat(), &rules);
                v["revision"] = json!(st.store.revision());
                v["generated"] = json!(store::now());
                v["approval"] = json!("only approved versions are exported; see versions and audit");
                if let Some(arr) = v["rules"].as_array_mut() {
                    for r in arr.iter_mut() {
                        let id = r["id"].as_str().unwrap_or("").to_string();
                        if let Some(ver) = st.store.get(&id).and_then(|x| x.active_version()) {
                            r["version"] = json!({"v": ver.v, "author": ver.author, "approved_by": ver.reviewer, "approved_at": ver.reviewed, "text_sha256": ver.text_sha256});
                        }
                    }
                }
                ([(header::CONTENT_TYPE, "application/json")], serde_json::to_string_pretty(&v).unwrap()).into_response()
            }
            "rules.p" => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], format!("% deployed revision {}\n{}", st.store.revision(), config::ruleset_tptp(st.cat(), &rules))).into_response(),
            "rules.lp" => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], format!("% deployed revision {}\n{}", st.store.revision(), crate::asp::ruleset_lp(&rules))).into_response(),
            "rules.ace" => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], st.store.active().iter().map(|(r, v)| format!("{}: {}\n", r.id, v.text)).collect::<String>()).into_response(),
            "audit.jsonl" => ([(header::CONTENT_TYPE, "application/x-ndjson")], st.store.audit_file().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default()).into_response(),
            "attempts.jsonl" => ([(header::CONTENT_TYPE, "application/x-ndjson")], st.attempts.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default()).into_response(),
            _ => (StatusCode::NOT_FOUND, "unknown export").into_response(),
        }
    }).await
}

// ---------------------------------------------------------------- OpenAI-compatible API

async fn models() -> Json<Value> {
    Json(json!({"object": "list", "data": MODELS.iter().map(|(id, d)| json!({"id": id, "object": "model", "created": 0, "owned_by": "iogp-editor", "description": d})).collect::<Vec<_>>()}))
}

fn last_user_text(v: &Value) -> Option<String> {
    let m = v.get("messages")?.as_array()?.iter().rev().find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))?;
    match m.get("content")? {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => Some(parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n")),
        _ => None,
    }
}

/// One model call: the assistant text for a user message of rule sentences.
pub fn chat_answer(st: &mut St, model: &str, text: &str) -> String {
    let items = parse_lines(text);
    if items.is_empty() { return "Send one or more rule sentences, e.g. \"If a worker is in a height-zone and does not wear a harness for at least 3 seconds then VisionGuard raises a critical working-at-height-alert for the worker.\"".into(); }
    let results: Vec<(String, String, Result<Rule, Vec<CompileError>>)> = items.into_iter().map(|(id, s)| { let r = st.comp.compile(&id, &s); st.log_attempt("api", model, &s, &r); (id, s, r) }).collect();
    match model {
        "iogp-rule-compiler" => {
            let ok: Vec<Rule> = results.iter().filter_map(|r| r.2.as_ref().ok().cloned()).collect();
            let mut v = config::ruleset_json(st.cat(), &ok);
            v["errors"] = json!(results.iter().filter_map(|(id, s, r)| r.as_ref().err().map(|es| json!({"id": id, "text": s, "errors": es}))).collect::<Vec<_>>());
            v["tptp"] = json!(config::ruleset_tptp(st.cat(), &ok));
            serde_json::to_string_pretty(&v).unwrap()
        }
        "iogp-rule-explain" => results.iter().map(|(id, s, r)| match r {
            Ok(r) => format!("{id}: {}\n  {}", r.readback[0], r.readback[1..].join("\n  ")),
            Err(es) => format!("{id}: \"{s}\" is not a valid rule:\n  {}", es.iter().map(|e| format!("{} — {}", e.message, e.hint)).collect::<Vec<_>>().join("\n  ")),
        }).collect::<Vec<_>>().join("\n\n"),
        _ => results.iter().map(|(id, _, r)| match r {
            Ok(r) => format!("{id}: valid ({})", lsr_label(r)),
            Err(es) => format!("{id}: invalid\n  {}", es.iter().map(|e| format!("{} — {}", e.message, e.hint)).collect::<Vec<_>>().join("\n  ")),
        }).collect::<Vec<_>>().join("\n"),
    }
}

async fn chat(AxState(w): AxState<Worker>, Json(v): Json<Value>) -> Response {
    let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("iogp-rule-compiler").to_string();
    if !MODELS.iter().any(|m| m.0 == model) {
        return (StatusCode::NOT_FOUND, Json(json!({"error": {"message": format!("model '{model}' not found; available: {}", MODELS.iter().map(|m| m.0).collect::<Vec<_>>().join(", ")), "type": "invalid_request_error", "code": "model_not_found"}}))).into_response();
    }
    let Some(text) = last_user_text(&v) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": {"message": "no user message", "type": "invalid_request_error"}}))).into_response();
    };
    let stream = v.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    let (m2, t2) = (model.clone(), text.clone());
    let answer = w.run(move |st| chat_answer(st, &m2, &t2)).await;
    let created = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let id = format!("chatcmpl-{created:x}{:x}", text.len());
    let (pt, ct) = (text.split_whitespace().count(), answer.split_whitespace().count());
    if stream {
        let c1 = json!({"id": id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [{"index": 0, "delta": {"role": "assistant", "content": answer}, "finish_reason": null}]});
        let c2 = json!({"id": id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]});
        return ([(header::CONTENT_TYPE, "text/event-stream"), (header::CACHE_CONTROL, "no-cache")], format!("data: {c1}\n\ndata: {c2}\n\ndata: [DONE]\n\n")).into_response();
    }
    Json(json!({
        "id": id, "object": "chat.completion", "created": created, "model": model,
        "choices": [{"index": 0, "message": {"role": "assistant", "content": answer}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": pt, "completion_tokens": ct, "total_tokens": pt + ct}
    })).into_response()
}

/// `{"frames": [...], "rules": ["sentence", ...]?}` → alert events. Without `rules` the saved
/// rule set is used. Lets the detection side test a configuration against recorded detections.
async fn simulate(AxState(w): AxState<Worker>, Json(v): Json<Value>) -> Response {
    w.run(move |st| {
        let frames: Vec<Frame> = match serde_json::from_value(v.get("frames").cloned().unwrap_or(json!([]))) {
            Ok(f) => f, Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": {"message": format!("frames: {e}")}}))).into_response(),
        };
        let (rules, errors): (Vec<Rule>, Vec<Value>) = match v.get("rules").and_then(|r| r.as_array()) {
            Some(rs) => {
                let mut ok = vec![]; let mut bad = vec![];
                for (i, r) in rs.iter().enumerate() {
                    let id = format!("r{}", i + 1);
                    match st.comp.compile(&id, r.as_str().unwrap_or("")) { Ok(x) => ok.push(x), Err(es) => bad.push(json!({"id": id, "errors": es})) }
                }
                (ok, bad)
            }
            None => (st.active_rules(), vec![]),
        };
        let (g, c) = (st.cat().defaults.grace_seconds, st.cat().defaults.cooldown_seconds);
        let mut eng = Engine::new(rules, v.get("grace_seconds").and_then(|x| x.as_f64()).unwrap_or(g), v.get("cooldown_seconds").and_then(|x| x.as_f64()).unwrap_or(c));
        let events: Vec<_> = frames.iter().flat_map(|f| eng.step(f)).collect();
        Json(json!({"events": events, "errors": errors})).into_response()
    }).await
}


/// Example scene and overlap with live rules, computed with clingo (offline; never at inference).
fn rule_analysis(cat: &Catalog, r: &Rule, live: &[Rule]) -> Markup {
    let w = crate::asp::witness(cat, r);
    let others: Vec<&Rule> = live.iter().filter(|x| x.id != r.id).collect();
    let mut covered_by = vec![]; let mut covers = vec![];
    if matches!(w, Ok(Some(_))) {
        for o in &others {
            if crate::asp::implies(r, o).unwrap_or(false) { covered_by.push(o.id.clone()); }
            if crate::asp::implies(o, r).unwrap_or(false) { covers.push(o.id.clone()); }
        }
    }
    html! {
        div.asp {
            @match &w {
                Ok(Some(w)) => {
                    b.small { "Example scene that fires it (smallest, over " (w.seconds) " s):" }
                    ul.small { @for l in &w.lines { li { (l) } } }
                }
                Ok(None) => { div.msg.err { b { "This rule can never fire" } " — no scene satisfies all its conditions together." } }
                Err(e) => { p.small.muted { "analysis failed: " (e) } }
            }
            @if !covered_by.is_empty() { div.msg.err.small { "Whenever this rule fires, live rule(s) " b { (covered_by.join(", ")) } " fire too — it adds no new alert situations." } }
            @if !covers.is_empty() { div.msg.okm.small { "It also fires in every situation where live rule(s) " b { (covers.join(", ")) } " fire." } }
        }
    }
}

async fn analysis_page(AxState(w): AxState<Worker>, h: HeaderMap) -> Response {
    w.run(move |st| {
        let Some(user) = st.user(&h) else { return to_login() };
        let live = st.active_rules();
        let cat = st.cat().clone();
        let rv = crate::asp::review(&cat, &live);
        let body = html! {
            h1 { "Analysis of the live rule set" }
            p.muted { "Computed offline with clingo (answer set programming) over the approved rules; inference never runs a solver. "
                "Download the rules as a clingo program: " a href="/export/rules.lp" { "rules.lp" } "." }
            @match &rv {
                Err(e) => { div.msg.err { (e) } }
                Ok(rv) => {
                    div.grid {
                        div.card {
                            h2 style="margin-top:0" { "Coverage" }
                            @if rv.uncovered_lsr.is_empty() { div.msg.okm { "Every IOGP Life-Saving Rule has at least one live rule." } }
                            @for (n, name) in &rv.uncovered_lsr { div.msg.err.small { "No live rule for Life-Saving Rule " (n) " — " (name) } }
                            @if !rv.unused_zones.is_empty() { div.msg.err.small { "Named zones no rule covers: " (rv.unused_zones.join(", ")) } }
                            h2 { "Rules that can never fire" }
                            @if rv.dead.is_empty() { p.muted { "None." } } @else { @for d in &rv.dead { div.msg.err.small { a href={ "/rules/" (d) } { (d) } " — no scene satisfies all its conditions together" } } }
                            h2 { "Rules implied by other rules" }
                            @if rv.implications.is_empty() { p.muted { "None — every rule catches situations no other rule catches." } }
                            @for i in &rv.implications { div.small style="margin:4px 0" { "Whenever " a href={ "/rules/" (i.a) } { b { (i.a) } } " fires, " a href={ "/rules/" (i.b) } { b { (i.b) } } " fires too." } }
                            p.small.muted { "Bounded check: " (rv.bound) "." }
                        }
                        div.card {
                            h2 style="margin-top:0" { "Example scene per rule" }
                            @for w in &rv.witnesses { div style="margin-bottom:10px" { b { a href={ "/rules/" (w.rule) } { (w.rule) } } span.small.muted { " — over " (w.seconds) " s" } ul.small { @for l in &w.lines { li { (l) } } } } }
                        }
                    }
                }
            }
        };
        page(layout("Analysis", Some(&user), st.demo(), body))
    }).await
}
