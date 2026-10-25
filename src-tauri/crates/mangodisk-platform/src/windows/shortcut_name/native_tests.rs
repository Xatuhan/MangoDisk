use super::*;
use crate::PlatformSystemSettingChangeRequest;
use winreg::types::ToRegValue;

fn raw_value(path: &str, name: &str) -> Option<RegValue<'static>> {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(path)
        .ok()
        .and_then(|key| key.get_raw_value(name).ok())
}

struct Restore {
    original: Option<RegValue<'static>>,
    key_existed: bool,
}
impl Drop for Restore {
    fn drop(&mut self) {
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let result = (|| -> io::Result<()> {
            let (key, _) = root.create_subkey(REGISTRY_PATH)?;
            if let Some(original) = &self.original {
                key.set_raw_value(VALUE_NAME, original)?;
            } else if let Err(error) = key.delete_value(VALUE_NAME) {
                if error.kind() != io::ErrorKind::NotFound {
                    return Err(error);
                }
            }
            let info = key.query_info()?;
            if !self.key_existed && info.sub_keys == 0 && info.values == 0 {
                drop(key);
                root.delete_subkey(REGISTRY_PATH)?;
            }
            Ok(())
        })();
        assert!(
            result.is_ok(),
            "shortcut naming fixture restoration failed: {result:?}"
        );
    }
}

#[test]
#[ignore = "temporarily changes the current user's naming template and exactly restores it"]
fn actual_shortcut_name_roundtrip_preserves_native_values_and_rejects_races() {
    let root = RegKey::predef(HKEY_CURRENT_USER);
    let original = raw_value(REGISTRY_PATH, VALUE_NAME);
    let copy = raw_value(REGISTRY_PATH, "CopyNameTemplate");
    let link = raw_value(
        r"Software\Microsoft\Windows\CurrentVersion\Explorer",
        "link",
    );
    let guard = Restore {
        original: original.as_ref().map(|value| RegValue {
            bytes: value.bytes.clone(),
            vtype: value.vtype.clone(),
        }),
        key_existed: root.open_subkey(REGISTRY_PATH).is_ok(),
    };
    let (key, _) = root.create_subkey(REGISTRY_PATH).unwrap();
    let optimized = PlatformSystemSettingValue::Text("%s.lnk".into());
    let change = |expected_value, desired_value| {
        super::super::system_settings::change(&PlatformSystemSettingChangeRequest {
            setting_id: SETTING_ID.into(),
            expected_value,
            desired_value,
        })
    };
    for raw in [
        None,
        Some("".to_reg_value()),
        Some(" %s - \u{81ea}\u{5b9a}\u{4e49} \u{1f96d}.lnk ".to_reg_value()),
    ] {
        if let Some(raw) = &raw {
            key.set_raw_value(VALUE_NAME, raw).unwrap();
        } else {
            let _ = key.delete_value(VALUE_NAME);
        }
        let before = read().unwrap();
        assert!(change(before.clone(), optimized.clone()).unwrap().verified);
        assert_eq!(read().unwrap(), optimized);
        let repeated = change(optimized.clone(), optimized.clone()).unwrap();
        assert!(repeated.verified && !repeated.changed);
        assert!(change(optimized.clone(), before).unwrap().verified);
        assert_eq!(raw_value(REGISTRY_PATH, VALUE_NAME), raw);
    }
    key.set_value(VALUE_NAME, &"%s - external.lnk").unwrap();
    assert_eq!(
        change(PlatformSystemSettingValue::Missing, optimized.clone())
            .unwrap_err()
            .code(),
        PlatformErrorCode::ItemChanged
    );
    assert_eq!(
        read().unwrap(),
        PlatformSystemSettingValue::Text("%s - external.lnk".into())
    );
    for raw in [
        "x".repeat(1_025).to_reg_value(),
        "\u{81ea}".repeat(342).to_reg_value(),
        RegValue {
            vtype: REG_BINARY,
            bytes: vec![1, 2, 3].into(),
        },
        RegValue {
            vtype: REG_EXPAND_SZ,
            bytes: "%s - external.lnk".to_reg_value().bytes,
        },
        RegValue {
            vtype: REG_SZ,
            bytes: vec![0, 216, 0, 0].into(),
        },
    ] {
        key.set_raw_value(VALUE_NAME, &raw).unwrap();
        assert!(read().is_err());
        assert!(change(PlatformSystemSettingValue::Missing, optimized.clone()).is_err());
        assert_eq!(raw_value(REGISTRY_PATH, VALUE_NAME), Some(raw));
    }
    assert_eq!(raw_value(REGISTRY_PATH, "CopyNameTemplate"), copy);
    assert_eq!(
        raw_value(
            r"Software\Microsoft\Windows\CurrentVersion\Explorer",
            "link"
        ),
        link
    );
    drop(key);
    drop(guard);
    assert_eq!(raw_value(REGISTRY_PATH, VALUE_NAME), original);
}
