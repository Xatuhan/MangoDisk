use mangodisk_platform::PlatformResult;

#[cfg(windows)]
pub(crate) fn prepare_startup_environment() -> PlatformResult<tauri::Env> {
    let environment = tauri::Env::default();
    let handoff = mangodisk_platform::prepare_elevated_restart_startup(environment.args_os.clone());
    environment_after_handoff(environment, handoff)
}

fn environment_after_handoff(
    mut environment: tauri::Env,
    handoff: Option<PlatformResult<()>>,
) -> PlatformResult<tauri::Env> {
    if let Some(result) = handoff {
        result?;
        // Tauri's updater and process plugin reuse Env.args_os. A completed handoff must not
        // be replayed against an exited parent during a later ordinary application restart.
        environment.args_os.truncate(1);
    }
    Ok(environment)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mangodisk_platform::PlatformError;

    fn environment(arguments: &[&str]) -> tauri::Env {
        let mut environment = tauri::Env::default();
        environment.args_os = arguments.iter().map(std::ffi::OsString::from).collect();
        environment
    }

    #[test]
    fn subsequent_restart_does_not_replay_completed_elevation_handoff() {
        let original = environment(&[
            "C:\\Mango Disk\\MangoDisk.exe",
            "--mangodisk-admin-restart-v1",
            "7580",
            "1d3814ee1f48c35ca27416c7573a306d",
        ]);
        let ready = environment_after_handoff(original, Some(Ok(()))).unwrap();
        assert_eq!(
            ready.args_os,
            environment(&["C:\\Mango Disk\\MangoDisk.exe"]).args_os
        );
        assert_eq!(ready.args_os.iter().skip(1).count(), 0);
    }

    #[test]
    fn ordinary_startup_keeps_arguments_and_failed_handoff_cannot_start_the_app() {
        let original = environment(&["MangoDisk.exe", "--background"]);
        let ready = environment_after_handoff(original.clone(), None).unwrap();
        assert_eq!(ready.args_os, original.args_os);
        assert!(environment_after_handoff(
            original,
            Some(Err(PlatformError::operation_failed("handoff failed")))
        )
        .is_err());
    }
}
