//! HTTP server replacing SWI's `library(http/thread_httpd)` + `http_dispatch` + `http_cors`.
//!
//! Routes:
//! * `GET|POST /` and `/ape` — the classic APE web service (same parameter names as SWI-APE:
//!   `text`, `solo`, `cdrs=on`, `guess=on`, `ulextext`, ...), query string or form body.
//! * `POST /v1/chat/completions` — OpenAI-compatible facade (see [`crate::openai`]).
//! * `GET /v1/models`, `GET /healthz`.
//!
//! One worker thread owns the APE engine (APE is not thread-safe; the SWI server also ran with
//! `workers(1)`); the accept loop forwards requests over a channel.

use crate::openai::{self, ChatRequest};
use crate::{Ape, ApeError};
use std::io::Read;
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};
use tiny_http::{Header, Method, Request, Response, Server};

type Job = (Vec<(String, String)>, mpsc::Sender<Result<crate::ApeResult, ApeError>>);

pub fn serve(prolog_dir: std::path::PathBuf, addr: &str, time_limit: u32) -> Result<(), Box<dyn std::error::Error>> {
    let (tx, rx) = mpsc::channel::<Job>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    std::thread::Builder::new().name("ape-worker".into()).stack_size(256 << 20).spawn(move || {
        let mut ape = match Ape::new(&prolog_dir) {
            Ok(a) => { let _ = ready_tx.send(Ok(())); a.with_time_limit(time_limit) }
            Err(e) => { let _ = ready_tx.send(Err(e.to_string())); return; }
        };
        for (params, reply) in rx {
            let p: Vec<(&str, &str)> = params.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let _ = reply.send(ape.get_results(&p));
        }
    })?;
    ready_rx.recv()??;
    let server = Server::http(addr).map_err(|e| e.to_string())?;
    eprintln!("ape-rs listening on http://{addr}  (routes: /, /v1/chat/completions, /v1/models, /healthz)");
    for req in server.incoming_requests() {
        if let Err(e) = handle(req, &tx) {
            eprintln!("request error: {e}");
        }
    }
    Ok(())
}

fn hdr(k: &str, v: &str) -> Header { Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap() }

fn cors<R: Read>(r: Response<R>) -> Response<R> {
    r.with_header(hdr("Access-Control-Allow-Origin", "*"))
        .with_header(hdr("Access-Control-Allow-Headers", "Content-Type, Authorization"))
        .with_header(hdr("Access-Control-Allow-Methods", "GET, POST, OPTIONS"))
}

fn json_resp(status: u16, v: &serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    cors(Response::from_string(v.to_string()).with_status_code(status).with_header(hdr("Content-Type", "application/json")))
}

fn run(tx: &mpsc::Sender<Job>, params: Vec<(String, String)>) -> Result<crate::ApeResult, ApeError> {
    let (rtx, rrx) = mpsc::channel();
    tx.send((params, rtx)).map_err(|e| ApeError::Prolog(e.to_string()))?;
    rrx.recv().map_err(|e| ApeError::Prolog(e.to_string()))?
}

fn handle(mut req: Request, tx: &mpsc::Sender<Job>) -> std::io::Result<()> {
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let path = path.to_string();
    let mut body = String::new();
    if *req.method() == Method::Post {
        req.as_reader().read_to_string(&mut body)?;
    }
    if *req.method() == Method::Options {
        return req.respond(cors(Response::empty(204)));
    }
    match (req.method().clone(), path.as_str()) {
        (_, "/healthz") => req.respond(json_resp(200, &serde_json::json!({"status": "ok"}))),
        (Method::Get, "/v1/models") => req.respond(json_resp(200, &openai::models_list())),
        (Method::Post, "/v1/chat/completions") => {
            let resp = chat(tx, &body);
            match resp {
                Ok((stream, v, sse)) if stream => req.respond(cors(
                    Response::from_string(sse).with_header(hdr("Content-Type", "text/event-stream")),
                )).map(|_| drop(v)),
                Ok((_, v, _)) => req.respond(json_resp(200, &v)),
                Err((code, msg)) => req.respond(json_resp(code, &openai::error_body(&msg, "invalid_request_error"))),
            }
        }
        (_, "/") | (_, "/ape") => {
            let mut params: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
            if !body.is_empty() {
                params.extend(url::form_urlencoded::parse(body.as_bytes()).into_owned());
            }
            match run(tx, params) {
                Ok(r) => req.respond(cors(Response::from_string(r.content)
                    .with_header(hdr("Content-Type", &format!("{}; charset=utf-8", r.content_type))))),
                Err(e) => req.respond(cors(Response::from_string(e.to_string()).with_status_code(400))),
            }
        }
        _ => req.respond(json_resp(404, &openai::error_body("not found", "not_found"))),
    }
}

/// Returns (stream?, json, sse-body) or (status, message).
fn chat(tx: &mpsc::Sender<Job>, body: &str) -> Result<(bool, serde_json::Value, String), (u16, String)> {
    let cr: ChatRequest = serde_json::from_str(body).map_err(|e| (400, format!("invalid JSON: {e}")))?;
    let output = openai::model_to_output(cr.model.as_deref()).map_err(|e| (400, e))?;
    let text = cr.messages.iter().rev().find(|m| m.role == "user").map(|m| m.content.clone())
        .ok_or((400, "no user message".to_string()))?;
    let mut params = vec![("text".to_string(), text.clone()), ("solo".to_string(), output.to_string())];
    if let Some(extra) = &cr.ape {
        for (k, v) in extra {
            let v = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
            params.push((k.clone(), v));
        }
    }
    let r = run(tx, params.clone()).map_err(|e| (400, e.to_string()))?;
    // prompt tokens = APE tokens
    let tok = run(tx, vec![("text".into(), text), ("solo".into(), "tokens".into())]).map(|t| t.content).unwrap_or_default();
    let prompt_tokens = tok.matches(',').count() + usize::from(!tok.is_empty());
    let created = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let id = format!("chatcmpl-ape-{created}-{}", r.elapsed.as_nanos() % 1_000_000);
    let model = cr.model.unwrap_or_else(|| "ape".into());
    let v = openai::completion_response(&id, &model, created, &r.content, prompt_tokens);
    let sse = openai::stream_body(&id, &model, created, &r.content);
    Ok((cr.stream, v, sse))
}
