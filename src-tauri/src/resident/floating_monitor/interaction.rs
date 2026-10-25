//! Windows metric clicks, detail anchors and recoverable window-wide click-through.
use super::LABEL;
use crate::resident::{preferences, runtime::ResidentState};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::Manager;

pub fn toggle_click_through(app: &tauri::AppHandle) -> tauri::Result<()> {
    for attempt in 0..2 {
        let mut requested = app
            .state::<Arc<ResidentState>>()
            .preferences
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        if !requested.enabled
            || requested.windows_display_mode
                == crate::resident::preference_schema::WindowsDisplayMode::Taskbar
        {
            return Ok(());
        }
        requested.floating.click_through = !requested.floating.click_through;
        match preferences::apply(app, requested) {
            Ok(_) => return Ok(()),
            Err(error) if attempt == 0 && error.is_preference_conflict() => continue,
            Err(error) => return Err(tauri::Error::Anyhow(error.into())),
        }
    }
    Ok(())
}

pub(super) fn apply_cursor_events(
    window: &tauri::WebviewWindow,
    applied: &AtomicBool,
    click_through: bool,
) -> tauri::Result<()> {
    if applied.load(Ordering::Relaxed) != click_through {
        if click_through {
            // Never make the window inert before an independent recovery entry exists.
            let tray = window
                .app_handle()
                .tray_by_id(crate::resident::TRAY_ID)
                .ok_or_else(|| {
                    tauri::Error::Io(std::io::Error::other("floating_recovery_tray_missing"))
                })?;
            tray.set_visible(true)?;
        }
        window.set_ignore_cursor_events(click_through)?;
        applied.store(click_through, Ordering::Relaxed);
        log::info!(
            "resident_floating_click_through enabled={click_through} recovery=tray outcome=applied"
        );
    }
    Ok(())
}

pub fn anchor(app: &tauri::AppHandle) -> Option<(f64, f64)> {
    let window = app.get_webview_window(LABEL)?;
    let point = window.outer_position().ok()?;
    let size = window.outer_size().ok()?;
    Some((
        point.x as f64 + size.width as f64 / 2.0,
        point.y as f64 + size.height as f64 / 2.0,
    ))
}

pub fn panel_position(
    app: &tauri::AppHandle,
    work: (f64, f64, f64, f64),
    size: (f64, f64),
    gap: f64,
) -> Option<(f64, f64)> {
    let window = app.get_webview_window(LABEL)?;
    let origin = window.outer_position().ok()?;
    let bounds = window.outer_size().ok()?;
    Some(super::geometry::detail_position(
        super::geometry::Area {
            x: origin.x,
            y: origin.y,
            width: bounds.width,
            height: bounds.height,
        },
        work,
        size,
        gap,
    ))
}

pub fn toggle_page(
    app: &tauri::AppHandle,
    page: crate::resident::panel_page::PanelPage,
) -> tauri::Result<()> {
    if app
        .state::<Arc<ResidentState>>()
        .preferences
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .floating
        .click_through
    {
        return Ok(());
    }
    crate::resident::panel::toggle_page_from(app, LABEL, page)
}
