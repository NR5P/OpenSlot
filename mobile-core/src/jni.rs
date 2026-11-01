//! JNI entry points for the Android app's `io.github.nr5p.openslot.Native` object, through
//! the JNI function table directly (as XARA's own Android bindings do). Text crosses as
//! UTF-8 byte arrays, so call signs and chat keep every character (JNI's own strings are
//! "modified UTF-8").

use std::ffi::{CString, c_char, c_void};
use std::ptr::null_mut;

type Env = *mut *const *const c_void;
type JObject = *mut c_void;

// Slots in JNINativeInterface (jni.h).
const NEW_STRING_UTF: usize = 167;
const GET_ARRAY_LENGTH: usize = 171;
const NEW_BYTE_ARRAY: usize = 176;
const GET_BYTE_ARRAY_REGION: usize = 200;
const SET_BYTE_ARRAY_REGION: usize = 208;

/// # Safety
/// `env` must be a live JNIEnv and `F` the slot's C signature.
unsafe fn slot<F: Copy>(env: Env, i: usize) -> F {
    // SAFETY: guaranteed by the caller; every slot holds a function pointer.
    unsafe { std::mem::transmute_copy(&*(*env).add(i)) }
}

/// # Safety
/// `env` must be a live JNIEnv and `array` a byte[] (or null).
unsafe fn text(env: Env, array: JObject) -> String {
    if array.is_null() {
        return String::new();
    }
    // SAFETY: the slots' signatures are as named in jni.h.
    unsafe {
        let len = slot::<unsafe extern "C" fn(Env, JObject) -> i32>(env, GET_ARRAY_LENGTH)(env, array);
        let mut buf = vec![0u8; len.max(0) as usize];
        slot::<unsafe extern "C" fn(Env, JObject, i32, i32, *mut u8)>(env, GET_BYTE_ARRAY_REGION)(
            env,
            array,
            0,
            len,
            buf.as_mut_ptr(),
        );
        String::from_utf8_lossy(&buf).into_owned()
    }
}

/// # Safety
/// `env` must be a live JNIEnv.
unsafe fn bytes(env: Env, s: Option<String>) -> JObject {
    let Some(s) = s else { return null_mut() };
    // SAFETY: the slots' signatures are as named in jni.h.
    unsafe {
        let array = slot::<unsafe extern "C" fn(Env, i32) -> JObject>(env, NEW_BYTE_ARRAY)(env, s.len() as i32);
        if !array.is_null() {
            slot::<unsafe extern "C" fn(Env, JObject, i32, i32, *const u8)>(env, SET_BYTE_ARRAY_REGION)(
                env,
                array,
                0,
                s.len() as i32,
                s.as_ptr(),
            );
        }
        array
    }
}

/// Null for Ok, else the error's text.
fn err(r: Result<(), String>) -> Option<String> {
    r.err()
}

#[unsafe(no_mangle)]
pub extern "C" fn Java_io_github_nr5p_openslot_Native_version(env: Env, _: JObject) -> JObject {
    let c = CString::new(env!("CARGO_PKG_VERSION")).unwrap_or_default();
    // SAFETY: the JVM passes a live env; NewStringUTF(env, const char*).
    unsafe { slot::<unsafe extern "C" fn(Env, *const c_char) -> JObject>(env, NEW_STRING_UTF)(env, c.as_ptr()) }
}

#[unsafe(no_mangle)]
pub extern "C" fn Java_io_github_nr5p_openslot_Native_start(env: Env, _: JObject, config: JObject) -> JObject {
    // SAFETY: the JVM passes a live env and the byte[] the Kotlin signature declares.
    unsafe { bytes(env, err(crate::start(&text(env, config)))) }
}

#[unsafe(no_mangle)]
pub extern "C" fn Java_io_github_nr5p_openslot_Native_stop(_: Env, _: JObject) {
    crate::stop();
}

#[unsafe(no_mangle)]
pub extern "C" fn Java_io_github_nr5p_openslot_Native_command(env: Env, _: JObject, json: JObject) -> JObject {
    // SAFETY: as above.
    unsafe { bytes(env, err(crate::command(&text(env, json)))) }
}

#[unsafe(no_mangle)]
pub extern "C" fn Java_io_github_nr5p_openslot_Native_query(env: Env, _: JObject, json: JObject) -> JObject {
    // SAFETY: as above.
    unsafe { bytes(env, Some(crate::query(&text(env, json)))) }
}

#[unsafe(no_mangle)]
pub extern "C" fn Java_io_github_nr5p_openslot_Native_nextEvent(env: Env, _: JObject) -> JObject {
    // SAFETY: the JVM passes a live env.
    unsafe { bytes(env, crate::next_event()) }
}

#[unsafe(no_mangle)]
pub extern "C" fn Java_io_github_nr5p_openslot_Native_modemStart(env: Env, _: JObject, args: JObject) -> JObject {
    // SAFETY: as above.
    unsafe { bytes(env, err(crate::modem_start(&text(env, args)))) }
}

#[unsafe(no_mangle)]
pub extern "C" fn Java_io_github_nr5p_openslot_Native_modemStop(_: Env, _: JObject) -> i32 {
    crate::modem_stop().into()
}

#[unsafe(no_mangle)]
pub extern "C" fn Java_io_github_nr5p_openslot_Native_modemRunning(_: Env, _: JObject) -> u8 {
    crate::modem_running().into()
}

#[unsafe(no_mangle)]
pub extern "C" fn Java_io_github_nr5p_openslot_Native_modemNextLog(env: Env, _: JObject) -> JObject {
    // SAFETY: the JVM passes a live env.
    unsafe { bytes(env, crate::modem_next_log()) }
}
