//! C ABI: bad arguments are rejected, never crash; errors are reported.
use iogp_runtime::ffi::*;
use std::ffi::{CStr, CString};
use std::ptr;

#[test]
fn ffi_rejects_bad_input_and_reports_errors() {
    unsafe {
        let mut err: *mut std::ffi::c_char = ptr::null_mut();
        assert!(iogp_engine_new(ptr::null(), &mut err).is_null());
        assert!(CStr::from_ptr(err).to_str().unwrap().contains("NULL"));
        iogp_string_free(err);
        let bad = CString::new("{\"schema\": \"nope\"}").unwrap();
        let mut err: *mut std::ffi::c_char = ptr::null_mut();
        assert!(iogp_engine_new(bad.as_ptr(), &mut err).is_null());
        assert!(CStr::from_ptr(err).to_str().unwrap().contains("schema"));
        iogp_string_free(err);
        // NULL engine everywhere
        iogp_frame_begin(ptr::null_mut(), 0.0, ptr::null());
        assert_eq!(iogp_frame_object(ptr::null_mut(), ptr::null(), 0, ptr::null(), ptr::null()), -1);
        assert_eq!(iogp_frame_end(ptr::null_mut()), -1);
        assert!(iogp_step_json(ptr::null_mut(), ptr::null()).is_null());
        iogp_engine_free(ptr::null_mut());
        // a real engine: bad kind, bad JSON, event index out of range
        let cfg = CString::new(include_str!("fixtures/config.json")).unwrap();
        let e = iogp_engine_new(cfg.as_ptr(), ptr::null_mut());
        assert!(!e.is_null());
        let id = CString::new("7").unwrap();
        let ty = CString::new("person").unwrap();
        iogp_frame_begin(e, 0.0, ptr::null());
        assert_eq!(iogp_frame_object(e, id.as_ptr(), 9, ty.as_ptr(), ptr::null()), -1);
        assert_eq!(iogp_frame_object(e, id.as_ptr(), 0, ty.as_ptr(), ptr::null()), 0);
        assert_eq!(iogp_frame_end(e), 0);
        let mut ev = std::mem::zeroed::<IogpEvent>();
        assert_eq!(iogp_event_get(e, 0, &mut ev), -1);
        let js = CString::new("{not json").unwrap();
        assert!(iogp_step_json(e, js.as_ptr()).is_null());
        assert!(CStr::from_ptr(iogp_last_error(e)).to_str().unwrap().starts_with("frame:"));
        iogp_engine_free(e);
    }
}
