//! One current-user Explorer naming template, with lossless string recovery.
use std::io;
use winreg::{enums::*, RegKey, RegValue};

use crate::{PlatformError, PlatformErrorCode, PlatformResult, PlatformSystemSettingValue};

pub(super) const SETTING_ID: &str = "windows.explorer.remove-shortcut-suffix";
pub(super) const REGISTRY_PATH: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\NamingTemplates";
pub(super) const VALUE_NAME: &str = "ShortcutNameTemplate";
// Core's durable recovery ledger accepts at most 1 KiB of UTF-8 per text value.
// Refuse larger native baselines before any write can make them unrecoverable.
const MAX_TEXT_BYTES: usize = 1_024;

pub(super) fn valid_value(value: &PlatformSystemSettingValue) -> bool {
    match value {
        PlatformSystemSettingValue::Missing => true,
        PlatformSystemSettingValue::Text(text) => {
            !text.contains('\0') && text.len() <= MAX_TEXT_BYTES
        }
        _ => false,
    }
}

fn decode(value: RegValue<'_>) -> PlatformResult<PlatformSystemSettingValue> {
    // Do not coerce REG_EXPAND_SZ, malformed UTF-16 or embedded NULs into a
    // plausible string: the durable recovery record must reproduce the original.
    if value.vtype != REG_SZ
        || value.bytes.len() < 2
        || value.bytes.len() > (MAX_TEXT_BYTES + 1) * 2
        || !value.bytes.len().is_multiple_of(2)
    {
        return Err(PlatformError::new(
            PlatformErrorCode::InvalidData,
            "shortcut name template is not a recoverable REG_SZ",
        ));
    }
    let units = value
        .bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let text = units
        .strip_suffix(&[0])
        .and_then(|units| String::from_utf16(units).ok())
        .filter(|text| !text.contains('\0') && text.len() <= MAX_TEXT_BYTES)
        .ok_or_else(|| {
            PlatformError::new(
                PlatformErrorCode::InvalidData,
                "shortcut name template has invalid encoding or exceeds the recovery limit",
            )
        })?;
    Ok(PlatformSystemSettingValue::Text(text))
}

pub(super) fn read() -> PlatformResult<PlatformSystemSettingValue> {
    let root = RegKey::predef(HKEY_CURRENT_USER);
    let key = match root.open_subkey_with_flags(REGISTRY_PATH, KEY_QUERY_VALUE) {
        Ok(key) => key,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(PlatformSystemSettingValue::Missing);
        }
        Err(error) => return Err(failure("open_for_read", error)),
    };
    match key.get_raw_value(VALUE_NAME) {
        Ok(value) => decode(value),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(PlatformSystemSettingValue::Missing)
        }
        Err(error) => Err(failure("read_value", error)),
    }
}

pub(super) fn write(value: &PlatformSystemSettingValue) -> PlatformResult<()> {
    if !valid_value(value) {
        return Err(PlatformError::new(
            PlatformErrorCode::InvalidData,
            "shortcut name template request is invalid",
        ));
    }
    let root = RegKey::predef(HKEY_CURRENT_USER);
    match value {
        PlatformSystemSettingValue::Text(text) => {
            let (key, _) = root
                .create_subkey_with_flags(REGISTRY_PATH, KEY_SET_VALUE)
                .map_err(|error| failure("open_for_write", error))?;
            key.set_value(VALUE_NAME, text)
                .map_err(|error| failure("write_value", error))?;
        }
        PlatformSystemSettingValue::Missing => {
            let key = match root.open_subkey_with_flags(REGISTRY_PATH, KEY_SET_VALUE) {
                Ok(key) => key,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(failure("open_for_restore", error)),
            };
            match key.delete_value(VALUE_NAME) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(failure("delete_value", error)),
            }
        }
        _ => unreachable!("validated shortcut naming value"),
    }
    // Existing links, CopyNameTemplate, the Explorer `link` value and shortcut
    // file-type registration remain outside this setting's capability boundary.
    log::info!(
        "windows_shortcut_name_written removed={} restart_required=true",
        matches!(value, PlatformSystemSettingValue::Missing)
    );
    Ok(())
}

fn failure(stage: &str, error: io::Error) -> PlatformError {
    log::warn!(
        "windows_shortcut_name_failed stage={stage} value_name={VALUE_NAME} native_code={:?} error_kind={:?}",
        error.raw_os_error(), error.kind()
    );
    if error.kind() == io::ErrorKind::PermissionDenied {
        PlatformError::new(
            PlatformErrorCode::AccessDenied,
            "access shortcut naming preference",
        )
    } else {
        PlatformError::io("access shortcut naming preference", &error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winreg::types::ToRegValue;

    #[test]
    fn original_templates_roundtrip_without_trimming_or_normalization() {
        for text in [
            "",
            "%s.lnk",
            " %s - \u{81ea}\u{5b9a}\u{4e49} \u{1f96d}.lnk ",
            "%s - Shortcut.lnk",
        ] {
            let value = PlatformSystemSettingValue::Text(text.into());
            assert!(valid_value(&value));
            assert_eq!(decode(text.to_reg_value()).unwrap(), value);
        }
    }

    #[test]
    fn templates_outside_durable_recovery_limits_fail_closed() {
        for text in ["x".repeat(1_025), "\u{81ea}".repeat(342)] {
            assert!(decode(text.to_reg_value()).is_err());
            assert!(!valid_value(&PlatformSystemSettingValue::Text(text)));
        }
        for text in ["x".repeat(1_024), "\u{81ea}".repeat(341)] {
            assert_eq!(
                decode(text.to_reg_value()).unwrap(),
                PlatformSystemSettingValue::Text(text)
            );
        }
    }

    #[test]
    fn non_string_and_noncanonical_values_fail_closed() {
        for (vtype, bytes) in [
            (REG_BINARY, vec![0, 0]),
            (REG_EXPAND_SZ, vec![0, 0]),
            (REG_SZ, vec![]),
            (REG_SZ, vec![0]),
            (REG_SZ, vec![65, 0]),
            (REG_SZ, vec![0, 216, 0, 0]),
            (REG_SZ, vec![65, 0, 0, 0, 66, 0, 0, 0]),
            (REG_SZ, vec![0; (MAX_TEXT_BYTES + 1) * 2 + 2]),
        ] {
            assert!(decode(RegValue {
                vtype,
                bytes: bytes.into()
            })
            .is_err());
        }
        assert!(!valid_value(&PlatformSystemSettingValue::Text(
            "a\0b".into()
        )));
        assert!(!valid_value(&PlatformSystemSettingValue::Text(
            "x".repeat(MAX_TEXT_BYTES + 1)
        )));
        assert!(!valid_value(&PlatformSystemSettingValue::Boolean(true)));
        assert!(valid_value(&PlatformSystemSettingValue::Missing));
    }
}

#[cfg(test)]
#[path = "shortcut_name/native_tests.rs"]
mod native_tests;
