use std::sync::Arc;

use discord_context_mcp::app_state::AppState;
use discord_context_mcp::config::Config;
use discord_context_mcp::db::open_db;

fn test_state() -> (tempfile::TempDir, tokio::runtime::Runtime, AppState) {
    let tmp = tempfile::tempdir().expect("temp dir");
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let config = Config {
        data_dir: tmp.path().to_path_buf(),
        log_dir: tmp.path().to_path_buf(),
        port: 0,
        autostart: false,
        gateway_enabled: false,
    };
    let db = Arc::new(open_db(tmp.path()).expect("db"));
    let state = AppState::new(config, db, false, rt.handle().clone());
    (tmp, rt, state)
}

#[test]
fn show_native_window_without_a_handle_is_a_no_op() {
    let (_tmp, _rt, state) = test_state();

    assert!(!state.show_native_window());
    state.set_window_hwnd(0);
    assert!(!state.show_native_window());

    #[cfg(not(windows))]
    {
        // No native window API on other platforms.
        state.set_window_hwnd(42);
        assert!(!state.show_native_window());
    }
}

#[cfg(windows)]
#[test]
fn show_window_by_hwnd_rejects_zero() {
    assert!(!discord_context_mcp::app_state::show_window_by_hwnd(0));
}

#[cfg(windows)]
#[test]
fn show_window_by_hwnd_unhides_a_real_window() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, IsWindowVisible, CW_USEDEFAULT, WS_OVERLAPPEDWINDOW,
    };

    // The built-in STATIC class needs no registration and no extra crate feature.
    let class_name: Vec<u16> = "STATIC\0".encode_utf16().collect();

    // Create a real top-level window without WS_VISIBLE, so it starts hidden.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            std::ptr::null(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    assert!(!hwnd.is_null(), "CreateWindowExW failed");
    assert_eq!(unsafe { IsWindowVisible(hwnd) }, 0, "window must start hidden");

    let (_tmp, _rt, state) = test_state();
    state.set_window_hwnd(hwnd as isize);
    assert_eq!(state.window_hwnd(), hwnd as isize);

    assert!(discord_context_mcp::app_state::show_window_by_hwnd(hwnd as isize));
    assert_ne!(unsafe { IsWindowVisible(hwnd) }, 0, "window must be visible");
    assert!(state.show_native_window(), "AppState must delegate to ShowWindow");

    unsafe { DestroyWindow(hwnd) };
    assert!(
        !discord_context_mcp::app_state::show_window_by_hwnd(hwnd as isize),
        "a destroyed window handle must be rejected"
    );
}
