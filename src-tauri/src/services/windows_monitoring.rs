//! Elevation adapters for the optional Windows sensor capability.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceState {
    NotInstalled,
    Ready,
    Unavailable,
    Incompatible,
    UnsupportedHardware,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub state: ServiceState,
    pub driver_available: bool,
    pub service_installed: bool,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SetupOutcome {
    Ready,
    Removed,
    UnsupportedHardware,
    Busy,
    VerificationFailed,
    UserCancelled,
    InstallationFailed,
    ServiceUnavailable,
    RestartRequired,
    DriverRemovalFailed,
    RemovalFailed,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupResult {
    pub outcome: SetupOutcome,
    pub observation: Observation,
}

/// Hardened local build: the LocalSystem service and PawnIO kernel driver for CPU
/// temperature are never installed. Existing installations can still be removed.
#[cfg(windows)]
const INSTALL_DISABLED: bool = true;

pub fn observe() -> Observation {
    #[cfg(windows)]
    return native::observe();
    #[cfg(not(windows))]
    Observation {
        state: ServiceState::UnsupportedHardware,
        driver_available: false,
        service_installed: false,
    }
}
pub async fn install(app: tauri::AppHandle) -> SetupResult {
    #[cfg(windows)]
    return native::install(app).await;
    #[cfg(not(windows))]
    {
        let _ = app;
        SetupResult {
            outcome: SetupOutcome::UnsupportedHardware,
            observation: observe(),
        }
    }
}
pub async fn uninstall(app: tauri::AppHandle, remove_driver: bool) -> SetupResult {
    #[cfg(windows)]
    return native::uninstall(app, remove_driver).await;
    #[cfg(not(windows))]
    {
        let _ = (app, remove_driver);
        SetupResult {
            outcome: SetupOutcome::UnsupportedHardware,
            observation: observe(),
        }
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use mangodisk_privileged::windows::{client, deployment};
    use mangodisk_privileged_protocol::{CapabilityStatus, Operation, Reply};
    use std::sync::atomic::{AtomicBool, Ordering};

    static BOOTSTRAP: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/resources/privileged/MangoDiskPrivileged.exe"
    ));

    static BUSY: AtomicBool = AtomicBool::new(false);
    struct OperationGuard;
    impl OperationGuard {
        fn acquire() -> Option<Self> {
            BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                .ok()
                .map(|_| Self)
        }
    }
    impl Drop for OperationGuard {
        fn drop(&mut self) {
            BUSY.store(false, Ordering::Release);
        }
    }
    pub(super) fn observe() -> Observation {
        let installation = client::service_installed();
        let service_installed = matches!(installation, Ok(true));
        // Installation is disabled, so present the capability as unavailable. An existing
        // service keeps its real state so the user can still remove it and its driver.
        if super::INSTALL_DISABLED && !service_installed {
            return Observation {
                state: ServiceState::UnsupportedHardware,
                driver_available: false,
                service_installed,
            };
        }
        if !deployment::hardware_supported() {
            return Observation {
                state: ServiceState::UnsupportedHardware,
                driver_available: false,
                service_installed,
            };
        }
        let driver_available = deployment::driver_available();
        let state = match installation {
            Ok(false) => ServiceState::NotInstalled,
            Err(_) => ServiceState::Unavailable,
            Ok(true) => match client::request(Operation::Health) {
                Ok(response) => match response.reply {
                    Reply::Health {
                        cpu_temperature, ..
                    } => {
                        if cpu_temperature == CapabilityStatus::Ready && sensor_ready() {
                            ServiceState::Ready
                        } else {
                            ServiceState::Unavailable
                        }
                    }
                    _ => ServiceState::Incompatible,
                },
                Err(error) => {
                    log::warn!(
                        "windows_monitoring_status_failed error={}",
                        mangodisk_platform::diagnostics::text(&error)
                    );
                    if error.kind() == std::io::ErrorKind::InvalidData {
                        ServiceState::Incompatible
                    } else {
                        ServiceState::Unavailable
                    }
                }
            },
        };
        Observation {
            state,
            driver_available,
            service_installed,
        }
    }
    fn sensor_ready() -> bool {
        let Ok(response) = client::request(Operation::CpuTemperature) else {
            return false;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        matches!(response.reply, Reply::CpuTemperature { status: CapabilityStatus::Ready, reading: Some(reading) } if reading.validate(now).is_ok())
    }
    async fn observe_async() -> Observation {
        tauri::async_runtime::spawn_blocking(observe)
            .await
            .unwrap_or(Observation {
                state: ServiceState::Unavailable,
                driver_available: false,
                service_installed: false,
            })
    }
    pub(super) async fn install(_app: tauri::AppHandle) -> SetupResult {
        // Rejected before staging the helper or requesting UAC.
        if super::INSTALL_DISABLED {
            log::info!("windows_monitoring_install_rejected reason=hardened_local_build");
            return SetupResult {
                outcome: SetupOutcome::UnsupportedHardware,
                observation: observe_async().await,
            };
        }
        let Some(guard) = OperationGuard::acquire() else {
            return SetupResult {
                outcome: SetupOutcome::Busy,
                observation: observe_async().await,
            };
        };
        let initial = observe_async().await;
        if matches!(initial.state, ServiceState::UnsupportedHardware) {
            return SetupResult {
                outcome: SetupOutcome::UnsupportedHardware,
                observation: initial,
            };
        }
        let result = tauri::async_runtime::spawn_blocking(move || {
            // The worker owns the gate even if its async caller is dropped.
            let _guard = guard;
            client::elevate_embedded(BOOTSTRAP, client::SetupAction::InstallCpuTemperature)
        })
        .await;
        let outcome = match result {
            Ok(Ok(0)) => SetupOutcome::Ready,
            Ok(Ok(deployment::REBOOT_EXIT_CODE)) => SetupOutcome::RestartRequired,
            Ok(Ok(deployment::BUSY_EXIT_CODE)) => SetupOutcome::Busy,
            Ok(Ok(deployment::VERIFICATION_EXIT_CODE)) => SetupOutcome::VerificationFailed,
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::InvalidData => {
                log::warn!(
                    "windows_monitoring_bootstrap_verification_failed error={}",
                    mangodisk_platform::diagnostics::text(&error)
                );
                SetupOutcome::VerificationFailed
            }
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::WouldBlock => SetupOutcome::Busy,
            Ok(Err(error)) if error.raw_os_error() == Some(1223) => SetupOutcome::UserCancelled,
            Ok(Err(error)) => {
                log::warn!(
                    "windows_monitoring_install_failed native_code={:?} error={}",
                    error.raw_os_error(),
                    mangodisk_platform::diagnostics::text(&error)
                );
                SetupOutcome::InstallationFailed
            }
            other => {
                log::warn!("windows_monitoring_install_failed result={other:?}");
                SetupOutcome::InstallationFailed
            }
        };
        let observation = observe_async().await;
        let outcome = match outcome {
            SetupOutcome::Ready if !matches!(observation.state, ServiceState::Ready) => {
                SetupOutcome::ServiceUnavailable
            }
            outcome => outcome,
        };
        SetupResult {
            outcome,
            observation,
        }
    }
    pub(super) async fn uninstall(_app: tauri::AppHandle, remove_driver: bool) -> SetupResult {
        let Some(guard) = OperationGuard::acquire() else {
            return SetupResult {
                outcome: SetupOutcome::Busy,
                observation: observe_async().await,
            };
        };
        let result = tauri::async_runtime::spawn_blocking(move || {
            let _guard = guard;
            client::elevate_embedded(BOOTSTRAP, client::SetupAction::Uninstall { remove_driver })
        })
        .await;
        let observation = observe_async().await;
        let outcome = match result {
            Ok(Ok(deployment::BUSY_EXIT_CODE)) => SetupOutcome::Busy,
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::WouldBlock => SetupOutcome::Busy,
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::InvalidData => {
                log::warn!(
                    "windows_monitoring_bootstrap_verification_failed error={}",
                    mangodisk_platform::diagnostics::text(&error)
                );
                SetupOutcome::VerificationFailed
            }
            Ok(Ok(0))
                if !observation.service_installed
                    && (!remove_driver || !observation.driver_available) =>
            {
                SetupOutcome::Removed
            }
            Ok(Ok(deployment::REBOOT_EXIT_CODE)) => SetupOutcome::RestartRequired,
            Ok(Ok(deployment::DRIVER_REMOVAL_EXIT_CODE)) => SetupOutcome::DriverRemovalFailed,
            Ok(Err(error)) if error.raw_os_error() == Some(1223) => SetupOutcome::UserCancelled,
            Ok(Err(error)) => {
                log::warn!(
                    "windows_monitoring_uninstall_failed remove_driver={remove_driver} native_code={:?} error={}",
                    error.raw_os_error(),
                    mangodisk_platform::diagnostics::text(&error)
                );
                SetupOutcome::RemovalFailed
            }
            other => {
                log::warn!("windows_monitoring_uninstall_failed remove_driver={remove_driver} result={other:?}");
                SetupOutcome::RemovalFailed
            }
        };
        SetupResult {
            outcome,
            observation,
        }
    }
}
