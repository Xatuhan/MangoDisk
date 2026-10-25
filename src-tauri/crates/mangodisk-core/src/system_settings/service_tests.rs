use super::recovery::{
    quarantine_invalid_recovery, INVALID_RECOVERY_FILE_NAME, MAX_RECOVERY_ITEMS,
    RECOVERY_FILE_NAME, RECOVERY_SCHEMA_VERSION,
};
use mangodisk_platform::PlatformSystemSettingSnapshot;
use std::fs;

use super::*;

fn pending_item(
    setting_id: &str,
    target: SystemSettingTargetState,
    expected: i64,
    desired: i64,
) -> PendingChangeItem {
    PendingChangeItem {
        public: SystemSettingChangePlanItem {
            setting_id: setting_id.to_string(),
            category: super::super::SystemSettingCategory::Performance,
            target,
            requires_restart: false,
            requires_elevation: false,
        },
        expected_value: PlatformSystemSettingValue::Integer(expected),
        desired_value: PlatformSystemSettingValue::Integer(desired),
        recovery_value: PlatformSystemSettingValue::Integer(expected),
    }
}

fn recovery_document(items: Vec<RecoveryItem>) -> RecoveryDocument {
    RecoveryDocument {
        schema_version: RECOVERY_SCHEMA_VERSION,
        recovery_id: "recovery".to_string(),
        created_at_ms: 1,
        items,
    }
}

fn recovery_item(setting_id: &str, original: i64, optimized: i64) -> RecoveryItem {
    RecoveryItem {
        setting_id: setting_id.to_string(),
        original_value: PlatformSystemSettingValue::Integer(original),
        optimized_value: PlatformSystemSettingValue::Integer(optimized),
    }
}

fn native_fixture(
    setting_id: &str,
    status: SystemSettingStatus,
    restore_available: bool,
    diagnostic: Option<PlatformSystemSettingDiagnosticCode>,
    requires_restart: bool,
) -> CatalogNativeItem {
    let optimized = status == SystemSettingStatus::Optimized;
    let value = i64::from(optimized);
    CatalogNativeItem {
        public: SystemSettingItem {
            setting_id: setting_id.to_string(),
            category: super::super::SystemSettingCategory::Performance,
            selection_kind: SystemSettingSelectionKind::Custom,
            risk_level: super::super::SystemSettingRiskLevel::Standard,
            status,
            selected_by_default: false,
            restore_available,
            requires_restart,
            requires_elevation: false,
            diagnostic,
        },
        current_value: PlatformSystemSettingValue::Integer(value),
        effective_value: PlatformSystemSettingValue::Integer(value),
        disabled_value: PlatformSystemSettingValue::Integer(0),
        recommended_value: PlatformSystemSettingValue::Integer(1),
    }
}

fn catalog_session_fixture(native_items: Vec<CatalogNativeItem>) -> CatalogSession {
    let items = native_items
        .iter()
        .map(|item| item.public.clone())
        .collect::<Vec<_>>();
    CatalogSession {
        public: SystemSettingsCatalog {
            schema_version: SYSTEM_SETTINGS_CATALOG_SCHEMA_VERSION,
            scan_id: "scan-plan-fixture".to_string(),
            catalog_revision: catalog_revision(&native_items),
            platform: current_platform_kind(),
            scanned_at_ms: 1,
            summary: summary(&items),
            items,
            elapsed_ms: 1,
            recovery_available: false,
        },
        native_items,
    }
}

#[test]
fn shortcut_overlay_restore_uses_the_durable_native_baseline() {
    let id = "windows.explorer.hide-shortcut-arrows";
    let mut item = native_fixture(id, SystemSettingStatus::Optimized, true, None, false);
    item.current_value = PlatformSystemSettingValue::Snapshot(PlatformSystemSettingSnapshot::Text(
        "owned-icon".into(),
    ));
    item.effective_value = PlatformSystemSettingValue::Boolean(true);
    item.recommended_value = PlatformSystemSettingValue::Boolean(true);
    item.disabled_value = PlatformSystemSettingValue::Boolean(false);
    let recovery = RecoveryItem {
        setting_id: id.into(),
        original_value: PlatformSystemSettingValue::Missing,
        optimized_value: PlatformSystemSettingValue::Boolean(true),
    };
    assert_eq!(
        desired_value_for_target(&item, SystemSettingTargetState::Default, Some(&recovery)),
        Some(PlatformSystemSettingValue::Missing)
    );
    assert_eq!(
        desired_value_for_target(&item, SystemSettingTargetState::Default, None),
        None
    );
}

#[test]
fn shortcut_name_restore_preserves_missing_empty_and_custom_templates() {
    let id = "windows.explorer.remove-shortcut-suffix";
    let optimized = PlatformSystemSettingValue::Text("%s.lnk".into());
    for original in [
        PlatformSystemSettingValue::Missing,
        PlatformSystemSettingValue::Text(String::new()),
        PlatformSystemSettingValue::Text(" %s - \u{81ea}\u{5b9a}\u{4e49} \u{1f96d}.lnk ".into()),
    ] {
        let native = mangodisk_platform::PlatformSystemSettingState {
            setting_id: id.into(),
            value: original.clone(),
            effective_value: original.clone(),
            requires_elevation: false,
            diagnostic: None,
        };
        let mut items = catalog_native_items(SystemSettingsPlatform::Windows, vec![native])
            .expect("shortcut naming catalog should load");
        let item = items
            .iter_mut()
            .find(|item| item.public.setting_id == id)
            .unwrap();
        assert!(!item.public.selected_by_default);
        assert_eq!(
            item.public.selection_kind,
            SystemSettingSelectionKind::Custom
        );
        item.current_value = optimized.clone();
        item.effective_value = optimized.clone();
        let document = recovery_document(vec![RecoveryItem {
            setting_id: id.into(),
            original_value: original.clone(),
            optimized_value: optimized.clone(),
        }]);
        let persisted: RecoveryDocument = serde_json::from_slice(
            &serde_json::to_vec(&document).expect("recovery should serialize"),
        )
        .expect("recovery should survive restarting the app");
        assert_eq!(
            desired_value_for_target(
                item,
                SystemSettingTargetState::Default,
                persisted.items.first()
            ),
            Some(original)
        );
        item.effective_value =
            PlatformSystemSettingValue::Text("%s - changed elsewhere.lnk".into());
        assert!(!valid_recovery_baseline(&persisted.items[0], item));
    }
}

#[test]
fn shortcut_name_reenable_preserves_the_latest_external_template() {
    let id = "windows.explorer.remove-shortcut-suffix";
    let optimized = PlatformSystemSettingValue::Text("%s.lnk".into());
    let previous = recovery_document(vec![RecoveryItem {
        setting_id: id.into(),
        original_value: PlatformSystemSettingValue::Missing,
        optimized_value: optimized.clone(),
    }]);
    for external in [
        PlatformSystemSettingValue::Missing,
        PlatformSystemSettingValue::Text(String::new()),
        PlatformSystemSettingValue::Text(" %s - external \u{1f96d}.lnk ".into()),
    ] {
        let native = mangodisk_platform::PlatformSystemSettingState {
            setting_id: id.into(),
            value: external.clone(),
            effective_value: external.clone(),
            requires_elevation: false,
            diagnostic: None,
        };
        let items = catalog_native_items(SystemSettingsPlatform::Windows, vec![native])
            .expect("shortcut naming catalog should load");
        let session = catalog_session_fixture(items);
        let selection = SystemSettingsChangeSelection {
            scan_id: session.public.scan_id.clone(),
            items: vec![super::super::SystemSettingChangeSelectionItem {
                setting_id: id.into(),
                target: SystemSettingTargetState::Optimized,
            }],
        };
        let pending = build_change_plan(selection, session, Some(&previous), 2);
        assert_eq!(pending.items.len(), 1);
        let ledger = recovery_intent(Some(&previous), &pending.items);
        assert_eq!(ledger[0].original_value, external);

        let mut restored = native_fixture(id, SystemSettingStatus::Optimized, true, None, true);
        restored.current_value = optimized.clone();
        restored.effective_value = optimized.clone();
        restored.disabled_value = PlatformSystemSettingValue::Missing;
        restored.recommended_value = optimized.clone();
        assert_eq!(
            desired_value_for_target(&restored, SystemSettingTargetState::Default, ledger.first()),
            Some(external)
        );
    }
}

#[test]
fn platform_inventory_rejects_ambiguous_identifiers_and_marks_missing_states() {
    let platform = current_platform_kind();
    let Some(definition) = definitions(platform).first() else {
        return;
    };
    let state = mangodisk_platform::PlatformSystemSettingState {
        setting_id: definition.id.to_string(),
        value: definition.default_value.owned(),
        effective_value: definition.default_value.owned(),
        requires_elevation: definition.requires_elevation,
        diagnostic: None,
    };

    let items = catalog_native_items(platform, vec![state.clone()])
        .expect("a partial native inventory must remain inspectable");
    assert_eq!(items.len(), definitions(platform).len());
    assert_eq!(items[0].public.setting_id, definition.id);
    assert!(items
        .iter()
        .skip(1)
        .all(|item| item.public.status == SystemSettingStatus::Unavailable));

    assert_eq!(
        catalog_native_items(platform, vec![state.clone(), state])
            .err()
            .expect("duplicate states must fail closed")
            .code(),
        crate::shared::CoreErrorCode::Platform
    );
    let unknown = mangodisk_platform::PlatformSystemSettingState {
        setting_id: "system-setting.unknown".to_string(),
        value: PlatformSystemSettingValue::Missing,
        effective_value: PlatformSystemSettingValue::Missing,
        requires_elevation: false,
        diagnostic: None,
    };
    assert_eq!(
        catalog_native_items(platform, vec![unknown])
            .err()
            .expect("unknown states must fail closed")
            .code(),
        crate::shared::CoreErrorCode::Platform
    );
}

#[test]
fn missing_key_repeat_preferences_remain_system_owned_and_recommended() {
    let definition = definitions(SystemSettingsPlatform::Macos)
        .iter()
        .find(|definition| definition.id == "macos.keyboard.fast-key-repeat")
        .expect("the key repeat preference should exist");
    let state = mangodisk_platform::PlatformSystemSettingState {
        setting_id: definition.id.to_string(),
        value: PlatformSystemSettingValue::Missing,
        effective_value: PlatformSystemSettingValue::Missing,
        requires_elevation: false,
        diagnostic: None,
    };

    let item = native_item(definition, Some(&state));

    assert_eq!(item.public.status, SystemSettingStatus::Recommended);
    assert_eq!(item.current_value, PlatformSystemSettingValue::Missing);
    assert_eq!(item.effective_value, PlatformSystemSettingValue::Missing);
    assert_eq!(item.disabled_value, PlatformSystemSettingValue::Missing);
    assert_eq!(
        item.recommended_value,
        PlatformSystemSettingValue::Integer(2)
    );
}

#[test]
fn legacy_recommendation_remains_restorable_after_catalog_changes() {
    let recovery = recovery_document(vec![RecoveryItem {
        setting_id: "macos.keyboard.fast-key-repeat".to_string(),
        original_value: PlatformSystemSettingValue::Missing,
        optimized_value: PlatformSystemSettingValue::Integer(1),
    }]);
    let mut native_items = vec![CatalogNativeItem {
        public: SystemSettingItem {
            setting_id: "macos.keyboard.fast-key-repeat".to_string(),
            category: super::super::SystemSettingCategory::Performance,
            selection_kind: SystemSettingSelectionKind::Custom,
            risk_level: super::super::SystemSettingRiskLevel::Standard,
            status: SystemSettingStatus::Recommended,
            selected_by_default: false,
            restore_available: false,
            requires_restart: false,
            requires_elevation: false,
            diagnostic: None,
        },
        current_value: PlatformSystemSettingValue::Integer(1),
        effective_value: PlatformSystemSettingValue::Integer(1),
        disabled_value: PlatformSystemSettingValue::Missing,
        recommended_value: PlatformSystemSettingValue::Integer(2),
    }];
    assert!(mark_restore_availability(
        &mut native_items,
        Some(&recovery)
    ));
    let session = catalog_session_fixture(native_items);
    let selection = SystemSettingsChangeSelection {
        scan_id: session.public.scan_id.clone(),
        items: vec![super::super::SystemSettingChangeSelectionItem {
            setting_id: "macos.keyboard.fast-key-repeat".to_string(),
            target: SystemSettingTargetState::Default,
        }],
    };

    let pending = build_change_plan(selection, session, Some(&recovery), 5_000);

    assert!(pending.public.skipped_items.is_empty());
    assert_eq!(pending.items.len(), 1);
    assert_eq!(
        pending.items[0].desired_value,
        PlatformSystemSettingValue::Missing
    );
}

#[test]
fn mixed_change_plan_preserves_recovery_and_reports_every_skip_reason() {
    let session = catalog_session_fixture(vec![
        native_fixture(
            "enable",
            SystemSettingStatus::Recommended,
            false,
            None,
            true,
        ),
        native_fixture("restore", SystemSettingStatus::Optimized, true, None, false),
        native_fixture(
            "unsupported",
            SystemSettingStatus::Unavailable,
            false,
            Some(PlatformSystemSettingDiagnosticCode::Unsupported),
            false,
        ),
        native_fixture(
            "already-default",
            SystemSettingStatus::Recommended,
            false,
            None,
            false,
        ),
        native_fixture(
            "already-optimized",
            SystemSettingStatus::Optimized,
            false,
            None,
            false,
        ),
        native_fixture(
            "stale-restore",
            SystemSettingStatus::Optimized,
            true,
            None,
            false,
        ),
    ]);
    let selection_item =
        |setting_id: &str, target| super::super::SystemSettingChangeSelectionItem {
            setting_id: setting_id.to_string(),
            target,
        };
    let selection = SystemSettingsChangeSelection {
        scan_id: session.public.scan_id.clone(),
        items: vec![
            selection_item("enable", SystemSettingTargetState::Optimized),
            selection_item("restore", SystemSettingTargetState::Default),
            selection_item("unsupported", SystemSettingTargetState::Optimized),
            selection_item("already-default", SystemSettingTargetState::Default),
            selection_item("already-optimized", SystemSettingTargetState::Optimized),
            selection_item("stale-restore", SystemSettingTargetState::Default),
            selection_item("missing", SystemSettingTargetState::Optimized),
        ],
    };
    let recovery = recovery_document(vec![recovery_item("restore", 0, 1)]);

    let pending = build_change_plan(selection, session, Some(&recovery), 5_000);

    assert_eq!(pending.public.created_at_ms, 5_000);
    assert_eq!(pending.public.expires_at_ms, 5_000 + CHANGE_PLAN_TTL_MS);
    assert!(pending.public.requires_confirmation);
    assert!(pending.public.requires_restart);
    assert_eq!(pending.items.len(), 2);
    assert_eq!(pending.items[0].public.setting_id, "enable");
    assert_eq!(
        pending.items[0].desired_value,
        PlatformSystemSettingValue::Integer(1)
    );
    assert_eq!(pending.items[1].public.setting_id, "restore");
    assert_eq!(
        pending.items[1].desired_value,
        PlatformSystemSettingValue::Integer(0)
    );
    assert_eq!(
        pending
            .public
            .skipped_items
            .iter()
            .map(|item| (item.setting_id.as_str(), item.reason))
            .collect::<Vec<_>>(),
        vec![
            ("unsupported", SystemSettingChangeSkipReason::Unsupported),
            (
                "already-default",
                SystemSettingChangeSkipReason::AlreadyDefault
            ),
            (
                "already-optimized",
                SystemSettingChangeSkipReason::AlreadyOptimized
            ),
            (
                "stale-restore",
                SystemSettingChangeSkipReason::SettingChanged
            ),
            ("missing", SystemSettingChangeSkipReason::SettingMissing),
        ]
    );
}

#[test]
fn summary_separates_recommended_and_default_selected_items() {
    let items = vec![
        SystemSettingItem {
            setting_id: "one".to_string(),
            category: super::super::SystemSettingCategory::Privacy,
            selection_kind: SystemSettingSelectionKind::OneClick,
            risk_level: super::super::SystemSettingRiskLevel::Standard,
            status: SystemSettingStatus::Recommended,
            selected_by_default: true,
            restore_available: false,
            requires_restart: false,
            requires_elevation: false,
            diagnostic: None,
        },
        SystemSettingItem {
            setting_id: "two".to_string(),
            category: super::super::SystemSettingCategory::Appearance,
            selection_kind: SystemSettingSelectionKind::Custom,
            risk_level: super::super::SystemSettingRiskLevel::Standard,
            status: SystemSettingStatus::Recommended,
            selected_by_default: false,
            restore_available: false,
            requires_restart: false,
            requires_elevation: false,
            diagnostic: None,
        },
    ];
    let summary = summary(&items);
    assert_eq!(summary.recommended_count, 2);
    assert_eq!(summary.selected_count, 1);
}

#[test]
fn native_capability_mismatches_fail_closed() {
    let definition = SettingDefinition {
        id: "windows.test.setting",
        category: super::super::SystemSettingCategory::Privacy,
        selection_kind: SystemSettingSelectionKind::Custom,
        risk_level: super::super::SystemSettingRiskLevel::Standard,
        default_value: super::super::catalog::DefinitionValue::Integer(0),
        disabled_value: None,
        recommended_value: super::super::catalog::DefinitionValue::Integer(1),
        requires_restart: false,
        requires_elevation: false,
    };
    let state = mangodisk_platform::PlatformSystemSettingState {
        setting_id: definition.id.to_string(),
        value: PlatformSystemSettingValue::Integer(0),
        effective_value: PlatformSystemSettingValue::Integer(0),
        requires_elevation: true,
        diagnostic: None,
    };

    let item = native_item(&definition, Some(&state));

    assert_eq!(item.public.status, SystemSettingStatus::Unavailable);
    assert_eq!(
        item.public.diagnostic,
        Some(PlatformSystemSettingDiagnosticCode::InvalidData)
    );
    assert!(item.public.requires_elevation);
}

#[test]
fn restart_notice_requires_a_changed_item_that_needs_restart() {
    let pending_items = vec![
        PendingChangeItem {
            public: SystemSettingChangePlanItem {
                setting_id: "restart".to_string(),
                category: super::super::SystemSettingCategory::Productivity,
                target: SystemSettingTargetState::Optimized,
                requires_restart: true,
                requires_elevation: false,
            },
            expected_value: PlatformSystemSettingValue::Integer(0),
            desired_value: PlatformSystemSettingValue::Integer(1),
            recovery_value: PlatformSystemSettingValue::Integer(0),
        },
        PendingChangeItem {
            public: SystemSettingChangePlanItem {
                setting_id: "immediate".to_string(),
                category: super::super::SystemSettingCategory::Appearance,
                target: SystemSettingTargetState::Optimized,
                requires_restart: false,
                requires_elevation: false,
            },
            expected_value: PlatformSystemSettingValue::Integer(0),
            desired_value: PlatformSystemSettingValue::Integer(1),
            recovery_value: PlatformSystemSettingValue::Integer(0),
        },
    ];
    let failed_restart = vec![
        change_result(
            "restart".to_string(),
            SystemSettingChangeOutcomeStatus::Failed,
            false,
            Some(SystemSettingChangeFailureReason::PlatformFailure),
        ),
        change_result(
            "immediate".to_string(),
            SystemSettingChangeOutcomeStatus::Changed,
            true,
            None,
        ),
    ];
    assert!(!changed_items_require_restart(
        &pending_items,
        &failed_restart
    ));

    let changed_restart = vec![change_result(
        "restart".to_string(),
        SystemSettingChangeOutcomeStatus::Changed,
        true,
        None,
    )];
    assert!(changed_items_require_restart(
        &pending_items,
        &changed_restart
    ));
}

#[test]
fn user_cancelled_platform_errors_preserve_cancellation_semantics() {
    assert_eq!(
        failure_reason(PlatformErrorCode::UserCancelled),
        SystemSettingChangeFailureReason::UserCancelled
    );
}

#[test]
fn recovery_intent_preserves_prior_baselines_across_batches() {
    let previous = recovery_document(vec![recovery_item("existing", 0, 1)]);
    let pending = vec![pending_item(
        "new",
        SystemSettingTargetState::Optimized,
        4,
        8,
    )];

    assert_eq!(
        recovery_intent(Some(&previous), &pending),
        vec![recovery_item("existing", 0, 1), recovery_item("new", 4, 8)]
    );
}

#[test]
fn policy_updates_preserve_a_matching_native_snapshot_baseline() {
    let id = "windows.test.snapshot";
    let original = PlatformSystemSettingValue::Snapshot(PlatformSystemSettingSnapshot::IntegerMap(
        BTreeMap::from([("native".into(), None)]),
    ));
    let previous = recovery_document(vec![RecoveryItem {
        setting_id: id.into(),
        original_value: original.clone(),
        optimized_value: PlatformSystemSettingValue::Integer(1),
    }]);
    let mut item = native_fixture(id, SystemSettingStatus::Recommended, true, None, false);
    item.current_value = PlatformSystemSettingValue::Snapshot(
        PlatformSystemSettingSnapshot::IntegerMap(BTreeMap::from([("native".into(), Some(1))])),
    );
    item.effective_value = PlatformSystemSettingValue::Integer(1);
    item.recommended_value = PlatformSystemSettingValue::Integer(2);
    let session = catalog_session_fixture(vec![item]);
    let selection = SystemSettingsChangeSelection {
        scan_id: session.public.scan_id.clone(),
        items: vec![super::super::SystemSettingChangeSelectionItem {
            setting_id: id.into(),
            target: SystemSettingTargetState::Optimized,
        }],
    };
    let pending = build_change_plan(selection, session, Some(&previous), 2);
    assert_eq!(pending.items.len(), 1);
    assert_eq!(
        recovery_intent(Some(&previous), &pending.items),
        vec![RecoveryItem {
            setting_id: id.into(),
            original_value: original,
            optimized_value: PlatformSystemSettingValue::Integer(2),
        }]
    );
}

#[test]
fn rejected_reenable_keeps_the_previous_recovery_record() {
    let previous = recovery_document(vec![recovery_item("existing", 0, 1)]);
    let pending = vec![pending_item(
        "existing",
        SystemSettingTargetState::Optimized,
        3,
        1,
    )];
    let preflight = recovery_intent(Some(&previous), &pending);
    assert_eq!(preflight, vec![recovery_item("existing", 3, 1)]);
    let results = vec![change_result(
        "existing".into(),
        SystemSettingChangeOutcomeStatus::Failed,
        false,
        Some(SystemSettingChangeFailureReason::SettingChanged),
    )];
    assert_eq!(
        reconciled_recovery_items(
            Some(&previous),
            preflight,
            &pending,
            &results,
            &BTreeSet::new()
        ),
        previous.items
    );
}

#[test]
fn recovery_baseline_must_match_the_current_native_value_and_value_kind() {
    let item = CatalogNativeItem {
        public: SystemSettingItem {
            setting_id: "windows.test.setting".to_string(),
            category: super::super::SystemSettingCategory::Privacy,
            selection_kind: SystemSettingSelectionKind::Custom,
            risk_level: super::super::SystemSettingRiskLevel::Standard,
            status: SystemSettingStatus::Optimized,
            selected_by_default: false,
            restore_available: false,
            requires_restart: false,
            requires_elevation: false,
            diagnostic: None,
        },
        current_value: PlatformSystemSettingValue::Integer(1),
        effective_value: PlatformSystemSettingValue::Integer(1),
        disabled_value: PlatformSystemSettingValue::Integer(0),
        recommended_value: PlatformSystemSettingValue::Integer(1),
    };

    assert!(valid_recovery_baseline(
        &recovery_item(item.public.setting_id.as_str(), 0, 1),
        &item
    ));
    assert!(!valid_recovery_baseline(
        &recovery_item(item.public.setting_id.as_str(), 0, 2),
        &item
    ));
    assert!(!valid_recovery_baseline(
        &RecoveryItem {
            setting_id: item.public.setting_id.clone(),
            original_value: PlatformSystemSettingValue::Text("invalid".to_string()),
            optimized_value: PlatformSystemSettingValue::Integer(1),
        },
        &item
    ));

    let snapshot = PlatformSystemSettingValue::Snapshot(PlatformSystemSettingSnapshot::IntegerMap(
        BTreeMap::from([("native".to_string(), Some(0))]),
    ));
    let snapshot_recovery = RecoveryItem {
        setting_id: item.public.setting_id.clone(),
        original_value: snapshot.clone(),
        optimized_value: PlatformSystemSettingValue::Integer(1),
    };
    assert!(!valid_recovery_baseline(&snapshot_recovery, &item));

    let snapshot_item = CatalogNativeItem {
        current_value: snapshot,
        ..item
    };
    assert!(valid_recovery_baseline(&snapshot_recovery, &snapshot_item));
}

#[test]
fn recovery_availability_requires_a_matching_durable_baseline() {
    let mut items = vec![CatalogNativeItem {
        public: SystemSettingItem {
            setting_id: "windows.test.setting".to_string(),
            category: super::super::SystemSettingCategory::Performance,
            selection_kind: SystemSettingSelectionKind::Custom,
            risk_level: super::super::SystemSettingRiskLevel::Standard,
            status: SystemSettingStatus::Optimized,
            selected_by_default: false,
            restore_available: false,
            requires_restart: false,
            requires_elevation: false,
            diagnostic: None,
        },
        current_value: PlatformSystemSettingValue::Integer(1),
        effective_value: PlatformSystemSettingValue::Integer(1),
        disabled_value: PlatformSystemSettingValue::Integer(0),
        recommended_value: PlatformSystemSettingValue::Integer(1),
    }];
    let matching = recovery_document(vec![recovery_item("windows.test.setting", 0, 1)]);

    assert!(mark_restore_availability(&mut items, Some(&matching)));
    assert!(items[0].public.restore_available);

    items[0].public.status = SystemSettingStatus::Recommended;
    items[0].recommended_value = PlatformSystemSettingValue::Integer(2);
    assert!(mark_restore_availability(&mut items, Some(&matching)));
    assert!(items[0].public.restore_available);

    let stale = recovery_document(vec![recovery_item("windows.test.setting", 0, 2)]);
    assert!(!mark_restore_availability(&mut items, Some(&stale)));
    assert!(!items[0].public.restore_available);

    items[0].public.status = SystemSettingStatus::Unavailable;
    assert!(!mark_restore_availability(&mut items, Some(&matching)));
    assert!(!items[0].public.restore_available);
}

#[test]
fn explicit_recovery_never_falls_back_to_the_catalog_default() {
    let mut item = CatalogNativeItem {
        public: SystemSettingItem {
            setting_id: "windows.test.setting".to_string(),
            category: super::super::SystemSettingCategory::Performance,
            selection_kind: SystemSettingSelectionKind::Custom,
            risk_level: super::super::SystemSettingRiskLevel::Standard,
            status: SystemSettingStatus::Optimized,
            selected_by_default: false,
            restore_available: true,
            requires_restart: false,
            requires_elevation: false,
            diagnostic: None,
        },
        current_value: PlatformSystemSettingValue::Integer(1),
        effective_value: PlatformSystemSettingValue::Integer(1),
        disabled_value: PlatformSystemSettingValue::Integer(0),
        recommended_value: PlatformSystemSettingValue::Integer(1),
    };

    assert_eq!(
        desired_value_for_target(&item, SystemSettingTargetState::Default, None),
        None
    );

    item.public.restore_available = false;
    assert_eq!(
        desired_value_for_target(&item, SystemSettingTargetState::Default, None),
        Some(PlatformSystemSettingValue::Integer(0))
    );
}

#[test]
fn successful_restore_removes_only_its_baseline() {
    let previous = recovery_document(vec![
        recovery_item("keep", 0, 1),
        recovery_item("restore", 2, 3),
    ]);
    let pending = vec![pending_item(
        "restore",
        SystemSettingTargetState::Default,
        3,
        2,
    )];
    let results = vec![change_result(
        "restore".to_string(),
        SystemSettingChangeOutcomeStatus::Changed,
        true,
        None,
    )];
    let preflight = recovery_intent(Some(&previous), &pending);

    assert_eq!(
        reconciled_recovery_items(
            Some(&previous),
            preflight,
            &pending,
            &results,
            &BTreeSet::new(),
        ),
        vec![recovery_item("keep", 0, 1)]
    );
}

#[test]
fn failed_enable_does_not_create_a_false_recovery_baseline() {
    let pending = vec![pending_item(
        "failed",
        SystemSettingTargetState::Optimized,
        0,
        1,
    )];
    let results = vec![change_result(
        "failed".to_string(),
        SystemSettingChangeOutcomeStatus::Failed,
        false,
        Some(SystemSettingChangeFailureReason::PlatformFailure),
    )];
    let preflight = recovery_intent(None, &pending);

    assert!(
        reconciled_recovery_items(None, preflight, &pending, &results, &BTreeSet::new(),)
            .is_empty()
    );
}

#[test]
fn uncertain_enable_failure_retains_the_preflight_recovery_baseline() {
    let pending = vec![pending_item(
        "uncertain",
        SystemSettingTargetState::Optimized,
        0,
        1,
    )];
    let results = vec![change_result(
        "uncertain".to_string(),
        SystemSettingChangeOutcomeStatus::Failed,
        false,
        Some(SystemSettingChangeFailureReason::VerificationFailed),
    )];
    let preflight = recovery_intent(None, &pending);
    let possibly_changed = BTreeSet::from(["uncertain".to_string()]);

    assert_eq!(
        reconciled_recovery_items(None, preflight, &pending, &results, &possibly_changed,),
        vec![recovery_item("uncertain", 0, 1)]
    );
}

#[test]
fn selection_rejects_duplicate_setting_identifiers() {
    let selection = SystemSettingsChangeSelection {
        scan_id: "scan".to_string(),
        items: vec![
            super::super::SystemSettingChangeSelectionItem {
                setting_id: "duplicate".to_string(),
                target: SystemSettingTargetState::Optimized,
            },
            super::super::SystemSettingChangeSelectionItem {
                setting_id: "duplicate".to_string(),
                target: SystemSettingTargetState::Default,
            },
        ],
    };

    assert!(validate_selection(&selection).is_err());
}

#[test]
fn selection_accepts_the_complete_current_windows_catalog_size() {
    let item_count = definitions(SystemSettingsPlatform::Windows).len();
    let selection = SystemSettingsChangeSelection {
        scan_id: "scan".to_string(),
        items: (0..item_count)
            .map(|index| super::super::SystemSettingChangeSelectionItem {
                setting_id: format!("windows.test.{index}"),
                target: SystemSettingTargetState::Optimized,
            })
            .collect(),
    };

    assert!(item_count > 128, "the regression requires a large catalog");
    assert!(MAX_CHANGE_ITEMS >= item_count);
    assert!(MAX_RECOVERY_ITEMS >= item_count);
    assert!(validate_selection(&selection).is_ok());
}

#[test]
fn invalid_recovery_is_quarantined_instead_of_deleted() {
    let directory = std::env::temp_dir().join(format!(
        "mangodisk-system-settings-recovery-quarantine-{}-{}",
        std::process::id(),
        crate::shared::clock::unix_time_ms()
    ));
    fs::create_dir_all(&directory).expect("the recovery fixture directory must be created");
    let recovery = directory.join(RECOVERY_FILE_NAME);
    let quarantine = directory.join(INVALID_RECOVERY_FILE_NAME);
    fs::write(&recovery, b"invalid recovery")
        .expect("the invalid recovery fixture must be written");

    quarantine_invalid_recovery(&recovery).expect("invalid recovery must be quarantined");

    assert!(!recovery.exists());
    assert_eq!(
        fs::read(&quarantine).expect("the quarantine must remain readable"),
        b"invalid recovery"
    );
    fs::remove_dir_all(directory).expect("the recovery fixture directory must be removed");
}
