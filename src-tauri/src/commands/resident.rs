use mangodisk_core::system_resources::metrics::MetricId;
use std::sync::Arc;
use tauri::Manager;

use super::error::{into_command_result, CommandResult};
use crate::resident::{
    self,
    preferences::ResidentPreferences,
    runtime::{ResidentReading, ResidentState},
};

#[tauri::command]
pub fn monitoring_get_reading(
    state: tauri::State<'_, Arc<ResidentState>>,
) -> CommandResult<ResidentReading> {
    into_command_result(
        "monitoring_get_reading",
        state
            .published_reading
            .lock()
            .map(|value| value.clone())
            .map_err(|_| "monitoring state unavailable"),
    )
}

#[tauri::command]
pub fn monitoring_refresh(state: tauri::State<'_, Arc<ResidentState>>) {
    state.refresh_cpu_processes();
}

#[tauri::command]
pub fn resident_get_catalogue(
    state: tauri::State<'_, Arc<ResidentState>>,
) -> CommandResult<ResidentReading> {
    state.request_catalogue();
    into_command_result(
        "resident_get_catalogue",
        state
            .published_reading
            .lock()
            .map(|value| value.clone())
            .map_err(|_| "resource catalogue unavailable"),
    )
}

#[tauri::command]
pub fn resident_get_panel_metric(
    state: tauri::State<'_, Arc<ResidentState>>,
) -> CommandResult<MetricId> {
    into_command_result(
        "resident_get_panel_metric",
        state
            .panel_metric
            .lock()
            .map(|value| *value)
            .map_err(|_| "panel metric unavailable"),
    )
}

#[tauri::command]
pub fn resident_select_metric(app: tauri::AppHandle, metric: MetricId) {
    resident::panel::set_metric(&app, metric);
}

#[tauri::command]
pub fn resident_get_preferences(
    state: tauri::State<'_, Arc<ResidentState>>,
) -> CommandResult<ResidentPreferences> {
    into_command_result(
        "resident_get_preferences",
        state
            .preferences
            .lock()
            .map(|value| value.clone())
            .map_err(|_| "resident preferences unavailable"),
    )
}

#[tauri::command]
pub fn resident_default_preferences() -> ResidentPreferences {
    ResidentPreferences::default()
}

#[tauri::command]
pub async fn resident_save_preferences(
    app: tauri::AppHandle,
    preferences: ResidentPreferences,
) -> CommandResult<ResidentPreferences> {
    into_command_result(
        "resident_save_preferences",
        resident::preferences::apply(&app, preferences),
    )
}

#[tauri::command]
pub async fn resident_open_panel(app: tauri::AppHandle) -> CommandResult<()> {
    into_command_result("resident_open_panel", resident::panel::open(&app))
}

#[tauri::command]
pub fn resident_panel_ready(app: tauri::AppHandle) {
    resident::panel::ready(&app);
}

#[tauri::command]
pub fn resident_hide_panel(app: tauri::AppHandle) {
    resident::panel::hide(&app);
}

#[tauri::command]
pub async fn resident_open_main(
    app: tauri::AppHandle,
    destination: resident::main_window::Destination,
) -> CommandResult<()> {
    into_command_result(
        "resident_open_main",
        resident::main_window::open(&app, destination, "panel"),
    )
}

#[tauri::command]
pub async fn resident_main_ready(
    app: tauri::AppHandle,
) -> CommandResult<Option<resident::main_window::Destination>> {
    into_command_result("resident_main_ready", resident::main_window::ready(&app))
}

#[tauri::command]
pub async fn resident_get_autostart(app: tauri::AppHandle) -> CommandResult<bool> {
    into_command_result("resident_get_autostart", resident::autostart::enabled(&app))
}

#[tauri::command]
pub async fn resident_set_autostart(app: tauri::AppHandle, enabled: bool) -> CommandResult<()> {
    into_command_result(
        "resident_set_autostart",
        resident::autostart::set(&app, enabled),
    )
}

#[tauri::command]
pub fn resident_quit(app: tauri::AppHandle) {
    app.exit(0);
}

#[tauri::command]
pub async fn monitoring_release_memory(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<ResidentState>>,
) -> CommandResult<mangodisk_core::system_resources::release::MemoryReleaseResult> {
    use mangodisk_core::system_resources::release::{MemoryReleaseResult, MemoryReleaseStatus};
    let result = tauri::async_runtime::spawn_blocking(move || {
        resident::memory_release::execute_configured(&app, false, None)
    })
    .await
    .unwrap_or_else(|_| MemoryReleaseResult::status(MemoryReleaseStatus::Failed));
    state.wake();
    Ok(result)
}

#[tauri::command]
pub async fn monitoring_quit_application(
    state: tauri::State<'_, Arc<ResidentState>>,
    application_id: String,
) -> CommandResult<mangodisk_core::ApplicationQuitStatus> {
    let result = super::error::run_blocking("monitoring_quit_application", move || {
        mangodisk_core::request_running_application_quit(&application_id)
    })
    .await;
    state.wake();
    result
}

#[tauri::command]
pub fn resident_get_display_status(
    app: tauri::AppHandle,
) -> resident::taskbar_display::DisplayStatus {
    resident::taskbar_display::status(&app)
}

#[tauri::command]
pub fn memory_release_preferences(
    app: tauri::AppHandle,
) -> CommandResult<mangodisk_core::system_resources::release_policy::ReleasePreferences> {
    into_command_result(
        "memory_release_preferences",
        resident::memory_preferences::get(&app),
    )
}

#[tauri::command]
pub async fn memory_release_save_preferences(
    app: tauri::AppHandle,
    preferences: mangodisk_core::system_resources::release_policy::ReleasePreferences,
) -> CommandResult<mangodisk_core::system_resources::release_policy::ReleasePreferences> {
    super::error::run_blocking("memory_release_save_preferences", move || {
        resident::memory_preferences::save(&app, preferences)
    })
    .await
}

#[tauri::command]
pub async fn memory_release_applications(
) -> CommandResult<Vec<mangodisk_core::system_resources::release_policy::ExcludedApplication>> {
    super::error::run_blocking(
        "memory_release_applications",
        mangodisk_core::system_resources::release_policy::running_applications,
    )
    .await
}

#[tauri::command]
pub async fn memory_release_open_settings(app: tauri::AppHandle) -> CommandResult<()> {
    super::error::run_blocking("memory_release_open_settings", move || {
        resident::memory_window::open(&app)
    })
    .await
}

#[tauri::command]
pub async fn monitoring_reset_network_traffic(
    state: tauri::State<'_, Arc<ResidentState>>,
    interface_id: String,
    started_at_ms: u64,
) -> CommandResult<ResidentReading> {
    let state = state.inner().clone();
    super::error::run_blocking("monitoring_reset_network_traffic", move || {
        resident::network_traffic::reset(&state, &interface_id, started_at_ms)
    })
    .await
}

#[tauri::command]
pub async fn resident_floating_ready(
    window: tauri::WebviewWindow,
) -> CommandResult<Option<resident::floating_monitor::Snapshot>> {
    if window.label() != resident::floating_monitor::LABEL {
        return Ok(None);
    }
    Ok(resident::floating_monitor::ready(window.app_handle()))
}

#[tauri::command]
pub async fn resident_floating_layout(
    window: tauri::WebviewWindow,
    measurement: resident::floating_monitor::LayoutMeasurement,
) -> CommandResult<bool> {
    if window.label() != resident::floating_monitor::LABEL {
        return Ok(false);
    }
    if !measurement.valid() {
        return Err(super::error::CommandError::invalid_input(
            "resident_floating_layout",
        ));
    }
    Ok(resident::floating_monitor::set_layout(
        window.app_handle(),
        measurement,
    ))
}

#[tauri::command]
pub async fn resident_floating_menu(window: tauri::WebviewWindow) -> CommandResult<()> {
    into_command_result(
        "resident_floating_menu",
        resident::floating_monitor::show_menu(window),
    )
}

#[tauri::command]
pub async fn resident_floating_drag(window: tauri::WebviewWindow) -> CommandResult<()> {
    into_command_result(
        "resident_floating_drag",
        resident::floating_monitor::start_drag(window),
    )
}

#[tauri::command]
pub async fn resident_floating_metric(
    window: tauri::WebviewWindow,
    metric: MetricId,
) -> CommandResult<()> {
    if window.label() != resident::floating_monitor::LABEL {
        return Ok(());
    }
    into_command_result(
        "resident_floating_metric",
        resident::floating_monitor::toggle_metric(window.app_handle(), metric),
    )
}
