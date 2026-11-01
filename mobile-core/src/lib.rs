//! The shared core of the iOS and Android apps; see include/openslot.h for the C interface
//! (Swift) and jni.rs for the Android one. The protocol, session and storage code is the
//! desktop app's (../src), compiled in unchanged.

#![allow(dead_code)] // the shared modules have desktop-only helpers

#[path = "../../src/storage/mod.rs"]
pub mod storage;
#[path = "../../src/vara/mod.rs"]
pub mod vara;
#[path = "../../src/varac/mod.rs"]
pub mod varac;

/// What the shared code asks of the desktop's config.rs.
pub mod config {
    /// The <GPSLOC>/GPS answer: the desktop's with no manual position set (the phones have
    /// no such setting yet).
    pub fn gps_answer() -> String {
        "NO GPS DATA".into()
    }
}

pub mod engine;
#[cfg(target_os = "android")]
mod jni;

use std::ffi::{CStr, CString, c_char};
use std::sync::{Mutex, PoisonError};

use engine::{Command, Config, Engine};

static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);
static MODEM: Mutex<Option<xara_ffi::XaraModem>> = Mutex::new(None);

// ---- the same calls for the C and JNI wrappers, on Rust strings ----

/// Starts the engine (config JSON, see openslot.h); Err says why not.
pub fn start(config_json: &str) -> Result<(), String> {
    let config: Config = serde_json::from_str(config_json).map_err(|e| format!("bad config: {e}"))?;
    let mut slot = ENGINE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(old) = slot.take() {
        old.shutdown();
    }
    *slot = Some(Engine::start(config)?);
    Ok(())
}

pub fn stop() {
    if let Some(e) = ENGINE.lock().unwrap_or_else(PoisonError::into_inner).take() {
        e.shutdown();
    }
}

pub fn command(json: &str) -> Result<(), String> {
    let cmd: Command = serde_json::from_str(json).map_err(|e| format!("bad command {json}: {e}"))?;
    match ENGINE.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
        Some(e) => {
            e.command(cmd);
            Ok(())
        }
        None => Err("not started".into()),
    }
}

pub fn query(json: &str) -> String {
    let q: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    if let Some(v) = engine::frequency_query(&q) {
        return v.to_string();
    }
    let engine = ENGINE.lock().unwrap_or_else(PoisonError::into_inner);
    match engine.as_ref() {
        Some(e) => engine::query(&e.db.lock().unwrap_or_else(PoisonError::into_inner), &q).to_string(),
        None => "null".into(),
    }
}

pub fn next_event() -> Option<String> {
    engine::next_event()
}

/// Starts the built-in XARA modem with xara's command-line options (JSON array of strings).
pub fn modem_start(args_json: &str) -> Result<(), String> {
    let args: Vec<String> = serde_json::from_str(args_json).map_err(|e| format!("bad modem options: {e}"))?;
    let mut slot = MODEM.lock().unwrap_or_else(PoisonError::into_inner);
    if slot.as_ref().is_some_and(|m| m.running()) {
        return Err("the modem is already running".into());
    }
    if let Some(old) = slot.take() {
        old.stop();
    }
    *slot = Some(xara_ffi::start(args).ok_or("the modem is already running")?);
    Ok(())
}

/// Stops the modem (PTT is released first); returns its exit status.
pub fn modem_stop() -> u8 {
    let m = MODEM.lock().unwrap_or_else(PoisonError::into_inner).take();
    m.map_or(0, xara_ffi::XaraModem::stop)
}

pub fn modem_running() -> bool {
    MODEM.lock().unwrap_or_else(PoisonError::into_inner).as_ref().is_some_and(|m| m.running())
}

pub fn modem_next_log() -> Option<String> {
    // The app shows chat itself; drop xara's own copy of received text so it doesn't pile up.
    while xara_ffi::next_received().is_some() {}
    xara_ffi::next_log()
}

// ---- C interface ----

/// # Safety
/// `s` must be NULL or a NUL-terminated string.
unsafe fn arg(s: *const c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    // SAFETY: guaranteed by the caller.
    unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
}

fn out(s: String) -> *mut c_char {
    CString::new(s.replace('\0', "")).unwrap_or_default().into_raw()
}

/// Error text for the last failed call, kept until the next one.
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());

fn result(r: Result<(), String>) -> bool {
    let ok = r.is_ok();
    *LAST_ERROR.lock().unwrap_or_else(PoisonError::into_inner) = r.err().unwrap_or_default();
    ok
}

#[unsafe(no_mangle)]
pub extern "C" fn openslot_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn openslot_last_error() -> *mut c_char {
    out(LAST_ERROR.lock().unwrap_or_else(PoisonError::into_inner).clone())
}

/// # Safety
/// `config_json` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openslot_start(config_json: *const c_char) -> bool {
    // SAFETY: guaranteed by the caller.
    result(start(&unsafe { arg(config_json) }))
}

#[unsafe(no_mangle)]
pub extern "C" fn openslot_stop() {
    stop();
}

/// # Safety
/// `json` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openslot_command(json: *const c_char) -> bool {
    // SAFETY: guaranteed by the caller.
    result(command(&unsafe { arg(json) }))
}

/// # Safety
/// `json` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openslot_query(json: *const c_char) -> *mut c_char {
    // SAFETY: guaranteed by the caller.
    out(query(&unsafe { arg(json) }))
}

#[unsafe(no_mangle)]
pub extern "C" fn openslot_next_event() -> *mut c_char {
    next_event().map_or(std::ptr::null_mut(), out)
}

/// # Safety
/// `s` must come from this library and not have been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openslot_free(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: guaranteed by the caller.
        drop(unsafe { CString::from_raw(s) });
    }
}

/// # Safety
/// `args_json` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openslot_modem_start(args_json: *const c_char) -> bool {
    // SAFETY: guaranteed by the caller.
    result(modem_start(&unsafe { arg(args_json) }))
}

#[unsafe(no_mangle)]
pub extern "C" fn openslot_modem_stop() -> u8 {
    modem_stop()
}

#[unsafe(no_mangle)]
pub extern "C" fn openslot_modem_running() -> bool {
    modem_running()
}

#[unsafe(no_mangle)]
pub extern "C" fn openslot_modem_next_log() -> *mut c_char {
    modem_next_log().map_or(std::ptr::null_mut(), out)
}
