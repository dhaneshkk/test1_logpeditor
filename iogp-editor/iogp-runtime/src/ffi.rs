//! C ABI. See `include/iogp_runtime.h`. All functions catch panics; strings are UTF-8, NUL-terminated.
//! Returned pointers stay valid until the next call that mutates the same engine (or `iogp_engine_free`).

use crate::fast::{Engine, Kind};
use std::ffi::{c_char, c_int, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

pub struct IogpEngine {
    eng: Engine,
    /// C copies of the last frame's events
    ev: Vec<[CString; 6]>,
    ev_meta: Vec<(c_int, u8, f64)>,
    json: CString,
    err: CString,
}

#[repr(C)]
pub struct IogpEvent {
    /// 1 = raised, 2 = cleared
    pub kind: c_int,
    pub rule: *const c_char,
    pub alert: *const c_char,
    pub lsr: u8,
    /// empty string when the rule has no severity
    pub severity: *const c_char,
    pub subject: *const c_char,
    pub t: f64,
    /// proof lines joined with '\n' (empty for cleared events)
    pub proof: *const c_char,
}

fn cs(s: &str) -> CString { CString::new(s.replace('\0', " ")).unwrap() }
unsafe fn rs<'a>(p: *const c_char) -> Option<&'a str> { if p.is_null() { None } else { CStr::from_ptr(p).to_str().ok() } }

impl IogpEngine {
    fn capture(&mut self) -> usize {
        let evs = self.eng.end_events();
        self.ev.clear(); self.ev_meta.clear();
        for e in evs {
            self.ev.push([cs(&e.rule), cs(&e.alert), cs(e.severity.as_deref().unwrap_or("")), cs(&e.subject), cs(&e.proof.join("\n")), cs(&e.kind)]);
            self.ev_meta.push((if e.kind == "raised" { 1 } else { 2 }, e.lsr, e.t));
        }
        self.ev.len()
    }
}

/// Create an engine from the editor's config.json text. On failure returns NULL and, if `err` is
/// not NULL, sets `*err` to a message that must be released with `iogp_string_free`.
#[no_mangle]
pub unsafe extern "C" fn iogp_engine_new(config_json: *const c_char, err: *mut *mut c_char) -> *mut IogpEngine {
    let r = catch_unwind(|| {
        let Some(txt) = rs(config_json) else { return Err("config_json is NULL or not UTF-8".to_string()) };
        Engine::from_config(txt).map_err(|e| e.to_string())
    });
    let m = match r {
        Ok(Ok(eng)) => return Box::into_raw(Box::new(IogpEngine { eng, ev: vec![], ev_meta: vec![], json: CString::default(), err: CString::default() })),
        Ok(Err(m)) => m,
        Err(_) => "panic while loading config".to_string(),
    };
    if !err.is_null() { *err = cs(&m).into_raw(); }
    ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn iogp_engine_free(e: *mut IogpEngine) { if !e.is_null() { drop(Box::from_raw(e)); } }

#[no_mangle]
pub unsafe extern "C" fn iogp_string_free(s: *mut c_char) { if !s.is_null() { drop(CString::from_raw(s)); } }

/// Start a frame at time `t` (seconds); `camera` may be NULL.
#[no_mangle]
pub unsafe extern "C" fn iogp_frame_begin(e: *mut IogpEngine, t: f64, camera: *const c_char) {
    if let Some(e) = e.as_mut() { let _ = catch_unwind(AssertUnwindSafe(|| e.eng.begin(t, rs(camera)))); }
}

/// Add an object: kind 0 = detected class (e.g. "person"), 1 = zone (e.g. "height-zone"),
/// 2 = signal (e.g. "permit"). `camera` may be NULL (the frame's camera applies).
/// Returns 0 on success, -1 on bad arguments.
#[no_mangle]
pub unsafe extern "C" fn iogp_frame_object(e: *mut IogpEngine, id: *const c_char, kind: c_int, type_: *const c_char, camera: *const c_char) -> c_int {
    let (Some(e), Some(id), Some(ty)) = (e.as_mut(), rs(id), rs(type_)) else { return -1 };
    let k = match kind { 0 => Kind::Class, 1 => Kind::Zone, 2 => Kind::Signal, _ => return -1 };
    match catch_unwind(AssertUnwindSafe(|| e.eng.object(id, k, ty, rs(camera)))) { Ok(()) => 0, Err(_) => -1 }
}

/// Add a fact, e.g. ("in", "7", "z1"), ("wear", "7", "h3"), ("have", "Moonpool", "PTW-1").
#[no_mangle]
pub unsafe extern "C" fn iogp_frame_fact(e: *mut IogpEngine, rel: *const c_char, a: *const c_char, b: *const c_char) -> c_int {
    let (Some(e), Some(r), Some(a), Some(b)) = (e.as_mut(), rs(rel), rs(a), rs(b)) else { return -1 };
    match catch_unwind(AssertUnwindSafe(|| e.eng.fact(r, a, b))) { Ok(()) => 0, Err(_) => -1 }
}

/// Evaluate the frame. Returns the number of events (read them with `iogp_event_get`), or -1.
#[no_mangle]
pub unsafe extern "C" fn iogp_frame_end(e: *mut IogpEngine) -> c_int {
    let Some(e) = e.as_mut() else { return -1 };
    match catch_unwind(AssertUnwindSafe(|| e.capture())) { Ok(n) => n as c_int, Err(_) => -1 }
}

/// Event `i` of the last frame (0-based). Returns 0, or -1 if out of range.
#[no_mangle]
pub unsafe extern "C" fn iogp_event_get(e: *const IogpEngine, i: usize, out: *mut IogpEvent) -> c_int {
    let (Some(e), Some(out)) = (e.as_ref(), out.as_mut()) else { return -1 };
    let (Some(s), Some(m)) = (e.ev.get(i), e.ev_meta.get(i)) else { return -1 };
    *out = IogpEvent { kind: m.0, rule: s[0].as_ptr(), alert: s[1].as_ptr(), lsr: m.1, severity: s[2].as_ptr(), subject: s[3].as_ptr(), t: m.2, proof: s[4].as_ptr() };
    0
}

/// Convenience: evaluate a frame given as JSON (`{"t":..,"camera":..,"objects":[..],"facts":[..]}`)
/// and return the events as a JSON array. Returns NULL on error (see `iogp_last_error`).
#[no_mangle]
pub unsafe extern "C" fn iogp_step_json(e: *mut IogpEngine, frame_json: *const c_char) -> *const c_char {
    let Some(e) = e.as_mut() else { return ptr::null() };
    let r = catch_unwind(AssertUnwindSafe(|| -> Result<String, String> {
        let txt = rs(frame_json).ok_or("frame_json is NULL or not UTF-8")?;
        let f: crate::Frame = serde_json::from_str(txt).map_err(|x| format!("frame: {x}"))?;
        let evs = e.eng.step(&f);
        serde_json::to_string(&evs).map_err(|x| x.to_string())
    }));
    match r {
        Ok(Ok(s)) => { e.json = cs(&s); e.json.as_ptr() }
        Ok(Err(m)) => { e.err = cs(&m); ptr::null() }
        Err(_) => { e.err = cs("panic during evaluation"); ptr::null() }
    }
}

/// Last error message for this engine ("" if none).
#[no_mangle]
pub unsafe extern "C" fn iogp_last_error(e: *const IogpEngine) -> *const c_char {
    match e.as_ref() { Some(e) => e.err.as_ptr(), None => c"engine is NULL".as_ptr() }
}

/// Number of alerts currently active (raised and not yet cooled down).
#[no_mangle]
pub unsafe extern "C" fn iogp_active_count(e: *const IogpEngine) -> usize { e.as_ref().map(|e| e.eng.active().len()).unwrap_or(0) }
