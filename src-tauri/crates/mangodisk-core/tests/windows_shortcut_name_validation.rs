#![cfg(target_os = "windows")]

use std::{fs, path::PathBuf, time::SystemTime};

use mangodisk_core::{
    configure_application_paths, ApplicationPaths, SystemSettingChangeFailureReason,
    SystemSettingChangeSelectionItem, SystemSettingTargetState, SystemSettingsChangePlan,
    SystemSettingsChangeSelection, SystemSettingsService,
};
use mangodisk_platform::{
    current_platform, PlatformCancellation, PlatformSystemSettingChangeRequest,
    PlatformSystemSettingValue, SystemSettingsPlatform,
};

const SETTING_ID: &str = "windows.explorer.remove-shortcut-suffix";

struct Fixture {
    original: PlatformSystemSettingValue,
    root: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        write_native(self.original.clone());
        fs::remove_dir_all(&self.root).expect("the isolated settings data must be removed");
    }
}

fn read_native() -> PlatformSystemSettingValue {
    let states = current_platform()
        .scan_system_settings(&[SETTING_ID], &PlatformCancellation::new(|| false))
        .expect("the native naming template must be readable");
    let state = &states[0];
    assert!(
        state.diagnostic.is_none(),
        "the native template must be recoverable"
    );
    state.value.clone()
}

fn write_native(desired_value: PlatformSystemSettingValue) {
    let result = current_platform()
        .change_system_setting(&PlatformSystemSettingChangeRequest {
            setting_id: SETTING_ID.into(),
            expected_value: read_native(),
            desired_value: desired_value.clone(),
        })
        .expect("the isolated native template change must succeed");
    assert!(result.verified);
    assert_eq!(read_native(), desired_value);
}

fn prepare(target: SystemSettingTargetState) -> SystemSettingsChangePlan {
    let catalog = SystemSettingsService::scan().expect("the real settings scan must succeed");
    let item = catalog
        .items
        .iter()
        .find(|item| item.setting_id == SETTING_ID)
        .unwrap();
    assert!(!item.selected_by_default);
    let plan = SystemSettingsService::prepare_change(SystemSettingsChangeSelection {
        scan_id: catalog.scan_id,
        items: vec![SystemSettingChangeSelectionItem {
            setting_id: SETTING_ID.into(),
            target,
        }],
    })
    .expect("the isolated naming plan must prepare");
    assert_eq!(plan.items.len(), 1);
    plan
}

fn apply(target: SystemSettingTargetState) {
    let result = SystemSettingsService::execute_change(prepare(target).plan_id)
        .expect("the isolated naming plan must execute");
    assert_eq!(result.changed_count, 1);
    assert_eq!(result.failed_count, 0);
}

#[test]
#[ignore = "temporarily changes the current user's naming template and restores its original value"]
fn external_templates_survive_reenable_and_preflight_conflicts() {
    let original = read_native();
    let root = std::env::temp_dir().join(format!(
        "mangodisk-shortcut-name-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    fs::create_dir_all(&root).expect("the isolated settings directory must be created");
    let fixture = Fixture { original, root };
    configure_application_paths(
        ApplicationPaths::new(
            fixture.root.join("data"),
            fixture.root.join("cache"),
            fixture.root.join("runtime"),
        )
        .unwrap(),
    )
    .expect("the test must use isolated recovery and history data");

    for external in [
        PlatformSystemSettingValue::Missing,
        PlatformSystemSettingValue::Text(String::new()),
        PlatformSystemSettingValue::Text(" %s - external \u{1f96d}.lnk ".into()),
    ] {
        write_native(PlatformSystemSettingValue::Missing);
        apply(SystemSettingTargetState::Optimized);
        write_native(external.clone());
        apply(SystemSettingTargetState::Optimized);
        // A new scan and plan reload the durable ledger instead of relying on the previous plan.
        apply(SystemSettingTargetState::Default);
        assert_eq!(read_native(), external);
    }

    write_native(PlatformSystemSettingValue::Missing);
    apply(SystemSettingTargetState::Optimized);
    write_native(PlatformSystemSettingValue::Text("%s - first.lnk".into()));
    let plan = prepare(SystemSettingTargetState::Optimized);
    let ledger_path = fixture.root.join("data/system-settings-recovery.json");
    let before: serde_json::Value =
        serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
    let raced = PlatformSystemSettingValue::Text("%s - second.lnk".into());
    write_native(raced.clone());
    let result = SystemSettingsService::execute_change(plan.plan_id).unwrap();
    assert_eq!(result.changed_count, 0);
    assert_eq!(result.failed_count, 1);
    assert_eq!(
        result.items[0].failure_reason,
        Some(SystemSettingChangeFailureReason::SettingChanged)
    );
    assert_eq!(read_native(), raced);
    let after: serde_json::Value =
        serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
    assert_eq!(before["items"], after["items"]);
    drop(fixture);
}
