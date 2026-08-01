//! tool-backend — 硬件调试工作台 Rust 后端（cdylib + staticlib）。
//! 通过 C FFI 向 Flutter 前端提供服务。所有跨语言数据交换使用 JSON 字符串。

mod backend;
mod bridge;
pub mod event;

use crate::backend::WorkbenchBackend;
use crate::event::BackendEvent;
use parking_lot::Mutex;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::LazyLock;

static BACKEND: LazyLock<Mutex<Option<Box<WorkbenchBackend>>>> = LazyLock::new(|| Mutex::new(None));

type EventCallback = extern "C" fn(*const c_char, usize);
static EVENT_CB: LazyLock<Mutex<Option<(EventCallback, usize)>>> =
    LazyLock::new(|| Mutex::new(None));

#[unsafe(no_mangle)]
pub extern "C" fn wb_set_event_callback(cb: Option<EventCallback>, user_data: usize) {
    let mut guard = EVENT_CB.lock();
    *guard = cb.map(|c| (c, user_data));
}

fn send_event(event: &BackendEvent) {
    // Do not hold the callback registry lock while entering Dart.  Keeping the
    // critical section this small prevents a future re-entrant callback from
    // deadlocking registration or shutdown.
    let callback = *EVENT_CB.lock();
    if let Some((cb, user_data)) = callback
        && let Ok(json) = serde_json::to_string(event)
        && let Ok(c_str) = CString::new(json)
    {
        cb(c_str.as_ptr(), user_data);
    }
}

// ── 生命周期 ──

#[unsafe(no_mangle)]
/// # Safety
///
/// `app_dir` must point to a valid, NUL-terminated UTF-8 string for the
/// duration of this call.
pub unsafe extern "C" fn wb_create(app_dir: *const c_char) -> i32 {
    if app_dir.is_null() {
        return -1;
    }
    let c_str = unsafe { CStr::from_ptr(app_dir) };
    let dir_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    let mut guard = BACKEND.lock();
    *guard = Some(Box::new(WorkbenchBackend::new(PathBuf::from(dir_str))));
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn wb_destroy() {
    let mut guard = BACKEND.lock();
    if let Some(mut backend) = guard.take() {
        backend.destroy();
    }
}

/// 轮询事件 — 先释放锁再调用回调，避免 re-entrant 死锁。
#[unsafe(no_mangle)]
pub extern "C" fn wb_poll_events() {
    let events = {
        let mut guard = BACKEND.lock();
        match guard.as_mut() {
            Some(backend) => backend.poll_events(64),
            None => return,
        }
    };
    for event in &events {
        send_event(event);
    }
}

// ── 命令 ──

#[unsafe(no_mangle)]
/// # Safety
///
/// `cmd` and, when non-null, `params_json` must point to valid,
/// NUL-terminated UTF-8 strings for the duration of this call.
pub unsafe extern "C" fn wb_cmd(cmd: *const c_char, params_json: *const c_char) -> *mut c_char {
    if cmd.is_null() {
        return CString::new(r#"{"error":"cmd is null"}"#)
            .unwrap_or_default()
            .into_raw();
    }
    let cmd_str = unsafe { CStr::from_ptr(cmd) }
        .to_str()
        .unwrap_or("")
        .to_owned();
    let params: serde_json::Value = if params_json.is_null() {
        serde_json::Value::Null
    } else {
        let s = unsafe { CStr::from_ptr(params_json) }
            .to_str()
            .unwrap_or("null");
        serde_json::from_str(s).unwrap_or(serde_json::Value::Null)
    };
    // 先释放锁再发送回调，避免 re-entrant 死锁
    let result = {
        let mut guard = BACKEND.lock();
        match guard.as_mut() {
            Some(backend) => backend.handle_command(&cmd_str, &params),
            None => Err("后端未初始化".to_owned()),
        }
    };
    let json = match result {
        Ok(v) => serde_json::to_string(&v).unwrap_or_else(|_| "null".to_owned()),
        Err(e) => serde_json::json!({ "error": e }).to_string(),
    };
    CString::new(json).unwrap_or_default().into_raw()
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `s` must be either null or a pointer returned by this library from
/// `wb_cmd`, `wb_get_ports`, `wb_get_plugins`, or `wb_get_status`, and it may
/// be passed exactly once.
pub unsafe extern "C" fn wb_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

// ── 查询快捷方式 ──

#[unsafe(no_mangle)]
pub extern "C" fn wb_get_ports() -> *mut c_char {
    let guard = BACKEND.lock();
    let json = match guard.as_ref() {
        Some(backend) => {
            serde_json::to_string(&backend.get_ports_json()).unwrap_or_else(|_| "[]".to_owned())
        }
        None => "[]".to_owned(),
    };
    CString::new(json).unwrap_or_default().into_raw()
}

#[unsafe(no_mangle)]
pub extern "C" fn wb_get_plugins() -> *mut c_char {
    let guard = BACKEND.lock();
    let json = match guard.as_ref() {
        Some(backend) => {
            serde_json::to_string(&backend.get_plugins_json()).unwrap_or_else(|_| "[]".to_owned())
        }
        None => "[]".to_owned(),
    };
    CString::new(json).unwrap_or_default().into_raw()
}

#[unsafe(no_mangle)]
pub extern "C" fn wb_get_status() -> *mut c_char {
    let guard = BACKEND.lock();
    let json = match guard.as_ref() {
        Some(backend) => {
            serde_json::to_string(&backend.get_status_json()).unwrap_or_else(|_| "{}".to_owned())
        }
        None => "{}".to_owned(),
    };
    CString::new(json).unwrap_or_default().into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_create_destroy() {
        let dir = CString::new(".").unwrap();
        assert_eq!(unsafe { wb_create(dir.as_ptr()) }, 0);
        wb_destroy();
    }

    #[test]
    fn test_cmd_null_safe() {
        let result = unsafe { wb_cmd(std::ptr::null(), std::ptr::null()) };
        assert!(!result.is_null());
        let s = unsafe { CStr::from_ptr(result) }.to_str().unwrap();
        assert!(s.contains("error"));
        unsafe { wb_free_string(result) };
    }

    #[test]
    fn test_get_ports() {
        let dir = CString::new(".").unwrap();
        unsafe { wb_create(dir.as_ptr()) };
        let cmd = CString::new("get_ports").unwrap();
        let result = unsafe { wb_cmd(cmd.as_ptr(), std::ptr::null()) };
        assert!(!result.is_null());
        unsafe { wb_free_string(result) };
        wb_destroy();
    }
}
