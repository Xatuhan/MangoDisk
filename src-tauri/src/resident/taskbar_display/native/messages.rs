//! Win32 dispatch borrows the native state only for handled messages.
use super::super::{draw, shell_events};
use super::{Window, UPDATE};
use crate::resident::panel;
use std::{cell::RefCell, ptr, sync::atomic::Ordering};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    UI::{Controls::WM_MOUSELEAVE, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

pub(super) unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<Window>;
    if raw.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    if message == WM_NCDESTROY {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        PostQuitMessage(0);
        return 0;
    }
    // A popup menu can dispatch a wake while update() still holds its borrow.
    // Release the coalescing slot even then; the timer remains the safe fallback.
    if message == shell_events::WAKE {
        shell_events::dispatched();
    }
    // Parent-driven hiding need not pass through our logical visibility state.
    // Ignore synchronous notifications from our own ShowWindow calls.
    if message == WM_SHOWWINDOW {
        if let Ok(window) = (*raw).try_borrow() {
            log::info!(
                "resident_taskbar_external_visibility show={} reason={lparam} expected={:?} foreground={:?}",
                wparam != 0,
                window.visibility,
                GetForegroundWindow(),
            );
        }
    }
    // Default processing can synchronously reenter this callback. Borrow state
    // only for handled messages. Handle right release here so DefWindowProc
    // cannot forward our context menu to the Explorer parent.
    if !matches!(
        message,
        UPDATE
            | WM_TIMER
            | shell_events::WAKE
            | WM_PAINT
            | WM_MOUSEMOVE
            | WM_MOUSELEAVE
            | WM_LBUTTONUP
            | WM_RBUTTONUP
            | WM_CONTEXTMENU
    ) {
        return match message {
            WM_ERASEBKGND => 1,
            WM_MOUSEACTIVATE => MA_NOACTIVATE as _,
            WM_CLOSE => {
                ShowWindow(hwnd, SW_HIDE);
                0
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        };
    }
    // ShowWindow/SetWindowPos and popup menus may synchronously reenter this
    // callback. Never manufacture overlapping mutable references to window state.
    let Ok(mut window) = (*raw).try_borrow_mut() else {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    };
    match message {
        UPDATE | WM_TIMER | shell_events::WAKE => {
            if message == UPDATE {
                if window.service.requested.load(Ordering::Relaxed) {
                    SetTimer(hwnd, 1, 100, None);
                } else {
                    KillTimer(hwnd, 1);
                }
            }
            window.update(hwnd, message != WM_TIMER);
            0
        }
        WM_PAINT => {
            if window.background {
                draw::paint(
                    hwnd,
                    &window.columns,
                    &window.surface,
                    window.dpi,
                    window.color,
                    window.hover,
                );
            } else {
                // UpdateLayeredWindow owns the pixels; validate WM_PAINT without
                // trying to BitBlt to a layered window's redirected surface.
                ValidateRect(hwnd, ptr::null());
                window.paint_transparent(hwnd);
            }
            0
        }
        WM_MOUSEMOVE => {
            let mut tracking = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            TrackMouseEvent(&mut tracking);
            window.update_hover(hwnd);
            0
        }
        WM_MOUSELEAVE => {
            window.hover = None;
            InvalidateRect(hwnd, ptr::null(), 0);
            let app = window.service.app.clone();
            // Match notification-area leave handling after an abandoned press.
            // Dispatch outside this native callback to avoid WebView reentrancy.
            tauri::async_runtime::spawn(async move { panel::tray_pointer_left(&app) });
            0
        }
        WM_LBUTTONUP => {
            window.update_hover(hwnd);
            if let Some(column) = window.hover.and_then(|i| window.columns.get(i)) {
                let app = window.service.app.clone();
                let id = column.id;
                tauri::async_runtime::spawn(async move {
                    if let Err(error) =
                        panel::toggle_from(&app, &format!("taskbar-{}", id.tray_id()), id.page())
                    {
                        crate::resident::diagnostics::Failure::record(
                            "taskbar_panel_entry",
                            &error,
                        );
                    }
                });
            }
            0
        }
        WM_RBUTTONUP | WM_CONTEXTMENU => {
            window.menu(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}
