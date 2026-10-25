//! Keep tray-icon's non-atomic Rc handles inside the AppKit main-thread boundary.
use objc2::MainThreadMarker;
use std::sync::mpsc;

/// Capture only application handles and owned data. Acquire and release every
/// native tray handle inside the operation; return only ordinary result data.
/// Callers must release display locks before dispatching. Preference writers
/// may hold their serialization gate, which main-thread operations never take.
pub(super) fn call<T: Send + 'static>(
    app: &tauri::AppHandle,
    operation: impl FnOnce(&tauri::AppHandle, MainThreadMarker) -> tauri::Result<T> + Send + 'static,
) -> tauri::Result<T> {
    if let Some(mtm) = MainThreadMarker::new() {
        return operation(app, mtm);
    }
    let target = app.clone();
    let (completed, result) = mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let mtm = MainThreadMarker::new().expect("tray operations run on the main thread");
        let _ = completed.send(operation(&target, mtm));
    })?;
    result
        .recv()
        .map_err(|_| tauri::Error::Io(std::io::Error::other("tray_main_thread_response_closed")))?
}

pub(in crate::resident) fn rect(
    app: &tauri::AppHandle,
    id: &str,
) -> tauri::Result<Option<tauri::Rect>> {
    let id = id.to_owned();
    call(app, move |app, _mtm| match app.tray_by_id(&id) {
        Some(tray) => tray.rect(),
        None => Ok(None),
    })
}

pub(in crate::resident) struct TrayGeometry {
    pub bounds: tauri::Rect,
    pub scale_factor: f64,
}

pub(in crate::resident) fn geometry(
    app: &tauri::AppHandle,
    id: &str,
) -> tauri::Result<Option<TrayGeometry>> {
    let id = id.to_owned();
    call(app, move |app, _mtm| {
        let Some(tray) = app.tray_by_id(&id) else {
            return Ok(None);
        };
        tray.with_inner_tray_icon(|tray| {
            let mtm = MainThreadMarker::new().expect("tray geometry runs on the main thread");
            let window = tray.ns_status_item()?.button(mtm)?.window()?;
            let bounds = tray.rect()?;
            Some(TrayGeometry {
                bounds: tauri::Rect {
                    position: bounds.position.into(),
                    size: bounds.size.into(),
                },
                scale_factor: window.backingScaleFactor(),
            })
        })
    })
}
