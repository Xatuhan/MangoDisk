//! Native resident floating surface; sampling and detail ownership stay in the existing runtime.
#[cfg(windows)]
mod drag;
#[cfg(any(windows, target_os = "macos", target_os = "linux", test))]
mod geometry;
#[cfg(windows)]
mod interaction;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod menu;
#[cfg(any(windows, target_os = "macos", target_os = "linux", test))]
mod menu_preferences;
#[cfg(windows)]
#[path = "windows.rs"]
mod native;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod native;
#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod native;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod placement;
mod sizing;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod window;

use super::{preference_schema::FloatingPreferences, tray_display::display_entry::DisplayEntry};
use serde::Serialize;

pub const LABEL: &str = "floating-monitor";
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
pub const EVENT: &str = "resident-floating-snapshot";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub schema_version: u32,
    pub interactive: bool,
    pub context_menu: bool,
    pub entries: Vec<DisplayEntry>,
    pub preferences: FloatingPreferences,
    pub columns: Option<Vec<f64>>,
}

pub use sizing::LayoutMeasurement;

#[cfg(windows)]
pub use interaction::{anchor, panel_position, toggle_click_through, toggle_page};
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
pub use window::{install, prepare, prewarm, ready, set_layout, show, update, update_title};

#[cfg(windows)]
pub(crate) use window::recreate_webview;

pub fn show_menu(window: tauri::WebviewWindow) -> tauri::Result<()> {
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    if window.label() == LABEL {
        return menu::open(&window);
    }
    let _ = window;
    Ok(())
}

pub fn start_drag(window: tauri::WebviewWindow) -> tauri::Result<()> {
    #[cfg(windows)]
    {
        drag::start(window)
    }
    #[cfg(target_os = "macos")]
    {
        native::start_drag(window)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        // Linux begins movement directly from the GTK button event.
        let _ = window;
        Ok(())
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub fn ready(_app: &tauri::AppHandle) -> Option<Snapshot> {
    None
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub fn set_layout(_app: &tauri::AppHandle, _measurement: LayoutMeasurement) -> bool {
    false
}

#[cfg(not(windows))]
pub fn toggle_page(
    _app: &tauri::AppHandle,
    _page: crate::resident::panel_page::PanelPage,
) -> tauri::Result<()> {
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn passive_preferences(mut preferences: FloatingPreferences) -> FloatingPreferences {
    preferences.always_on_top = true;
    preferences.locked = false;
    preferences.snap_to_edges = false;
    preferences.click_through = false;
    preferences
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passive_surfaces_keep_appearance_without_windows_interactions() {
        let original = FloatingPreferences {
            locked: true,
            click_through: true,
            bold: true,
            background_opacity: 75,
            ..Default::default()
        };
        let passive = passive_preferences(original.clone());
        assert!(passive.always_on_top);
        assert!(!passive.locked);
        assert!(!passive.click_through);
        assert!(!passive.snap_to_edges);
        assert!(passive.bold);
        assert_eq!(passive.background_opacity, 75);
        assert!(original.locked);
    }
}
