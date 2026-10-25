pub(crate) mod ai_runtime;
pub(crate) mod app_updates;
pub(crate) mod application_icon;
pub(crate) mod application_uninstall_catalog;
#[cfg(target_os = "macos")]
pub(crate) mod application_usage;
pub(crate) mod feedback;
pub(crate) mod native_labels;

pub(crate) mod windows_monitoring;

#[cfg(any(windows, test))]
pub(crate) mod application_restart;
