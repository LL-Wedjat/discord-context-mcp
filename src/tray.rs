use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Result;
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tokio::sync::mpsc::UnboundedSender;
use tracing::info;
use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};

use crate::app_state::{AppState, TrayAction};

fn create_tray_icon() -> Result<Icon> {
    // 32x32 RGBA icon (Discord blurple square with simple dot)
    let width = 32u32;
    let height = 32u32;
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);

    for y in 0..height {
        for x in 0..width {
            // Draw a rounded square in blurple (#5865F2 = 88, 101, 242)
            let is_border = x == 0 || x == width - 1 || y == 0 || y == height - 1;
            let is_corner = (x <= 1 || x >= width - 2) && (y <= 1 || y >= height - 2);

            if is_corner {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            } else if is_border {
                rgba.extend_from_slice(&[71, 82, 196, 255]);
            } else {
                // Inner white circle / dot in the center
                let dx = x as i32 - 16;
                let dy = y as i32 - 16;
                if dx * dx + dy * dy <= 25 {
                    rgba.extend_from_slice(&[255, 255, 255, 255]);
                } else {
                    rgba.extend_from_slice(&[88, 101, 242, 255]);
                }
            }
        }
    }

    let icon = Icon::from_rgba(rgba, width, height)?;
    Ok(icon)
}

#[cfg(windows)]
pub fn run_tray(state: AppState, event_proxy: UnboundedSender<TrayAction>) -> Result<()> {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };

    let tray_menu = Menu::new();

    let open_item = MenuItem::new("Open Settings", true, None);
    let reconnect_item = MenuItem::new("Reconnect", true, None);
    let quit_item = MenuItem::new("Quit", true, None);

    let open_id = open_item.id().clone();
    let reconnect_id = reconnect_item.id().clone();
    let quit_id = quit_item.id().clone();

    tray_menu.append(&open_item)?;
    tray_menu.append(&reconnect_item)?;
    tray_menu.append(&PredefinedMenuItem::separator())?;
    tray_menu.append(&quit_item)?;

    let icon = create_tray_icon()?;

    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("Discord Context MCP")
        .with_icon(icon)
        .build()?;

    info!("System tray initialized with Win32 message pump");

    let menu_channel = MenuEvent::receiver();
    let tray_channel = TrayIconEvent::receiver();

    while !state.shutdown.load(Ordering::SeqCst) {
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            // Non-blocking peek / pump of Windows messages
            while PeekMessageW(&mut msg, 0 as HWND, 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // Poll menu events
        while let Ok(event) = menu_channel.try_recv() {
            if event.id == open_id {
                let _ = event_proxy.send(TrayAction::OpenSettings);
            } else if event.id == reconnect_id {
                let _ = event_proxy.send(TrayAction::Reconnect);
            } else if event.id == quit_id {
                let _ = event_proxy.send(TrayAction::Quit);
                state.trigger_shutdown();
                break;
            }
        }

        // Poll tray icon events (double click or click to open)
        while let Ok(event) = tray_channel.try_recv() {
            if let TrayIconEvent::DoubleClick { .. } = event {
                let _ = event_proxy.send(TrayAction::OpenSettings);
            }
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    info!("Tray event loop terminating");
    Ok(())
}

#[cfg(not(windows))]
pub fn run_tray(state: AppState, event_proxy: UnboundedSender<TrayAction>) -> Result<()> {
    let tray_menu = Menu::new();

    let open_item = MenuItem::new("Open Settings", true, None);
    let reconnect_item = MenuItem::new("Reconnect", true, None);
    let quit_item = MenuItem::new("Quit", true, None);

    let open_id = open_item.id().clone();
    let reconnect_id = reconnect_item.id().clone();
    let quit_id = quit_item.id().clone();

    tray_menu.append(&open_item)?;
    tray_menu.append(&reconnect_item)?;
    tray_menu.append(&PredefinedMenuItem::separator())?;
    tray_menu.append(&quit_item)?;

    let icon = create_tray_icon()?;

    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("Discord Context MCP")
        .with_icon(icon)
        .build()?;

    info!("System tray initialized");

    let menu_channel = MenuEvent::receiver();
    let tray_channel = TrayIconEvent::receiver();

    while !state.shutdown.load(Ordering::SeqCst) {
        while let Ok(event) = menu_channel.try_recv() {
            if event.id == open_id {
                let _ = event_proxy.send(TrayAction::OpenSettings);
            } else if event.id == reconnect_id {
                let _ = event_proxy.send(TrayAction::Reconnect);
            } else if event.id == quit_id {
                let _ = event_proxy.send(TrayAction::Quit);
                state.trigger_shutdown();
                break;
            }
        }

        while let Ok(event) = tray_channel.try_recv() {
            if let TrayIconEvent::DoubleClick { .. } = event {
                let _ = event_proxy.send(TrayAction::OpenSettings);
            }
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    info!("Tray event loop terminating");
    Ok(())
}
