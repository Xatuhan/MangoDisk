use std::{fs, io, os::windows::fs::OpenOptionsExt, path::Path};

use crate::{PlatformError, PlatformResult};

/// Resolve only the fixed Windows-owned location, never a caller-supplied environment override.
pub fn windows_system_temporary_directory() -> PlatformResult<std::path::PathBuf> {
    super::directories::system_directory().map(|directory| directory.join("SystemTemp"))
}

pub fn windows_process_is_elevated() -> PlatformResult<bool> {
    super::disk_cleanup::current_process_is_elevated().map_err(|code| {
        PlatformError::operation_failed(format!(
            "Windows process elevation lookup failed native_code={code}"
        ))
    })
}

/// Probe deletion access without modifying the file, its ACL, or its attributes.
pub fn probe_windows_file_delete_access(path: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_FLAG_OPEN_REPARSE_POINT};

    fs::OpenOptions::new()
        .access_mode(DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map(|_| ())
}
