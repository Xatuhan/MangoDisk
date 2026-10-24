use super::error::{run_blocking, CommandResult};
use mangodisk_core::{ApplicationRestartGuard, CoreError};

/// No executable, path, or command-line input is accepted from the WebView.
#[tauri::command]
pub async fn restart_application_as_administrator(app: tauri::AppHandle) -> CommandResult<bool> {
    run_blocking("restart_application_as_administrator", move || {
        let reservation = ApplicationRestartGuard::acquire()?;
        #[cfg(windows)]
        let launched =
            mangodisk_platform::restart_current_application_elevated().map_err(CoreError::from)?;
        #[cfg(not(windows))]
        let launched = {
            let _ = app;
            Err::<bool, _>(CoreError::operation_failed(
                "administrator restart is only available on Windows",
            ))?
        };
        if launched {
            reservation.commit();
            log::info!("administrator_restart_handoff status=ready");
            app.exit(0);
        }
        Ok::<_, CoreError>(launched)
    })
    .await
}
