use crate::applications::uninstall::execution_history::{
    execution_history_record, UninstallHistoryRecordInput,
};
use crate::history::{OperationDetails, OperationOutcome};
use std::path::PathBuf;

use super::*;

#[test]
fn missing_registration_path_requires_an_absolute_absent_path_on_an_online_root() {
    let directory = tempfile::tempdir().expect("fixture directory should be created");
    assert!(!registered_path_is_missing(directory.path()));
    let canonical = directory
        .path()
        .canonicalize()
        .expect("fixture should resolve");
    assert!(registered_path_is_missing(&canonical.join("absent.exe")));
    assert!(!registered_path_is_missing(std::path::Path::new(
        "relative/absent.exe"
    )));
}

#[cfg(unix)]
#[test]
fn dangling_registration_links_are_not_evidence_of_a_removed_application() {
    let directory = tempfile::tempdir().expect("fixture directory should be created");
    let link = directory.path().join("application");
    std::os::unix::fs::symlink(directory.path().join("missing"), &link)
        .expect("fixture link should be created");
    assert!(!registered_path_is_missing(&link));
    assert!(!registered_path_is_missing(&link.join("missing-child.exe")));
}

#[test]
fn catalog_exposes_the_same_redacted_reference_and_typed_reason_as_inventory() {
    let mut application = fixture_application();
    application.uninstall_diagnostic =
        Some(mangodisk_platform::ApplicationUninstallDiagnostic::ExecutableAccessDenied);
    let item = candidate(&application, &ProcessSnapshot::default());
    let json = serde_json::to_value(&item).expect("candidate should serialize");
    assert_eq!(json["uninstallDiagnostic"], "executableAccessDenied");
    assert_eq!(
        json["dates"]["lastUsed"]["timestampMs"],
        1_700_000_000_000_u64
    );
    assert_eq!(
        json["dates"]["installed"]["timestampMs"],
        1_690_000_000_000_u64
    );
    assert_eq!(json["dates"]["lastUsed"]["source"], "platformReported");
    assert!(json.get("lastUsedAtMs").is_none());
    assert_eq!(
        json["applicationId"],
        mangodisk_platform::application_uninstall_diagnostic_id(&application.catalog_identifier)
    );
    assert!(!item
        .application_id
        .contains(&application.primary_identifier));
}

#[test]
fn verified_record_removal_updates_only_the_selected_catalog_row() {
    let mut removed = candidate(&fixture_application(), &ProcessSnapshot::default());
    removed.application_id = "removed".to_string();
    removed.capability = ApplicationUninstallCapability::ViewOnly;
    let mut remaining = removed.clone();
    remaining.application_id = "remaining".to_string();
    remaining.capability = ApplicationUninstallCapability::Ready;
    let mut scan = ApplicationUninstallScanResult {
        schema_version: APPLICATION_UNINSTALL_SCAN_SCHEMA_VERSION,
        scanned_at_ms: 123,
        supported: true,
        execution_supported: true,
        catalog_actionable: true,
        inventory_complete: true,
        catalog_revision: Some("original-revision".to_string()),
        candidates: vec![removed, remaining],
        ready_count: 1,
        blocked_count: 1,
        hidden_count: 0,
        related_directory_count: 0,
        related_path_scan_elapsed_ms: 0,
        elapsed_ms: 1,
    };
    assert_eq!(scan.remove_record("removed"), 1);
    assert_eq!(scan.candidates.len(), 1);
    assert_eq!(scan.candidates[0].application_id, "remaining");
    assert_eq!((scan.ready_count, scan.blocked_count), (1, 0));
    assert_eq!(scan.scanned_at_ms, 123);
    assert_eq!(scan.catalog_revision.as_deref(), Some("original-revision"));
    assert_eq!(scan.remove_record("removed"), 0);
}

#[test]
fn stable_inventory_retry_requires_two_different_complete_revisions() {
    assert!(should_retry_changed_inventory(
        true,
        true,
        Some("before"),
        Some("after")
    ));
    assert!(!should_retry_changed_inventory(
        true,
        true,
        Some("stable"),
        Some("stable")
    ));
    assert!(!should_retry_changed_inventory(
        false,
        true,
        Some("before"),
        Some("after")
    ));
    assert!(!should_retry_changed_inventory(
        true,
        false,
        Some("before"),
        Some("after")
    ));
    assert!(!should_retry_changed_inventory(
        true,
        true,
        None,
        Some("after")
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn partial_macos_inventory_keeps_verified_candidates_actionable() {
    assert!(catalog_is_actionable(false, true, true));
    assert!(!catalog_is_actionable(false, false, true));
    assert!(!catalog_is_actionable(false, true, false));
}

#[cfg(not(target_os = "macos"))]
#[test]
fn partial_non_macos_inventory_remains_non_actionable() {
    assert!(!catalog_is_actionable(false, true, true));
    assert!(catalog_is_actionable(true, true, true));
}

fn fixture_application() -> InstalledApplication {
    InstalledApplication {
        #[cfg(windows)]
        system_signed: false,
        #[cfg(windows)]
        package_executable_path: None,
        uninstall_diagnostic: None,
        catalog_identifier: "macos-bundle:/Applications/Example Editor.app".to_string(),
        source_identities: Vec::new(),
        primary_identifier: "com.example.Editor".to_string(),
        identifiers: vec![
            "Example Editor".to_string(),
            "com.example.Editor".to_string(),
        ],
        name: "Example Editor".to_string(),
        version: Some("1.0".to_string()),
        publisher: Some("Example".to_string()),
        estimated_bytes: 42_000,
        last_used_at_ms: Some(1_700_000_000_000),
        installed_at_ms: Some(1_690_000_000_000),
        icon_path: Some(PathBuf::from("/Applications/Example Editor.app")),
        bundle_path: Some(PathBuf::from("/Applications/Example Editor.app")),
        executable_paths: vec![PathBuf::from(
            "/Applications/Example Editor.app/Contents/MacOS/Example Editor",
        )],
        uninstall_registration: None,
    }
}

fn batch_result_fixture(
    application_id: &str,
    affected_item_count: u64,
    failed_item_count: u64,
    released_bytes: u64,
) -> ApplicationUninstallResult {
    ApplicationUninstallResult {
        plan_id: format!("plan-{application_id}"),
        application_id: application_id.to_string(),
        application_name: Some(application_id.to_string()),
        expected_bytes: 100,
        previewed_bytes: 0,
        released_bytes,
        previewed_item_count: 0,
        affected_item_count,
        failed_item_count,
        released_bytes_is_estimate: false,
        restart_required: false,
        dry_run: false,
        actions: Vec::new(),
        history_saved: affected_item_count > 0,
    }
}

fn history_candidate(application_id: &str, name: &str) -> ApplicationUninstallCandidate {
    let mut candidate = candidate(&fixture_application(), &ProcessSnapshot::default());
    candidate.application_id = application_id.to_string();
    candidate.primary_identifier = format!("identifier.{application_id}");
    candidate.name = name.to_string();
    candidate
}

#[test]
fn identity_keeps_package_names_but_does_not_use_registry_icon_executables() {
    let mut candidate = history_candidate("identity", "Example Application");
    candidate.platform = ApplicationUninstallPlatform::WindowsRegistry;
    candidate.source_identities = vec![ApplicationUninstallSourceIdentity {
        source: ApplicationUninstallInventorySource::WindowsRegistry,
        identifier: "private-registry-key".into(),
    }];
    candidate.executable_paths = vec![PathBuf::from(r"C:\Windows\explorer.exe")];
    let response = ApplicationUninstallService::describe_identity(&candidate);
    let json = serde_json::to_value(response).unwrap();
    assert_eq!(json["metadata"], serde_json::json!({"platform":"windows"}));

    candidate.executable_paths.clear();
    candidate
        .source_identities
        .push(ApplicationUninstallSourceIdentity {
            source: ApplicationUninstallInventorySource::WindowsAppx,
            identifier: "Microsoft.WindowsCalculator_8wekyb3d8bbwe".into(),
        });
    let response = ApplicationUninstallService::describe_identity(&candidate);
    let json = serde_json::to_value(response).unwrap();
    assert_eq!(
        json["metadata"]["packageIdentity"],
        "Microsoft.WindowsCalculator_8wekyb3d8bbwe"
    );
    assert!(!json.to_string().contains("private-registry-key"));
    candidate.source_identities[1].identifier = "x".repeat(1025);
    let response = ApplicationUninstallService::describe_identity(&candidate);
    assert_eq!(
        serde_json::to_value(response).unwrap()["metadata"],
        serde_json::json!({"platform":"windows"})
    );
}

#[cfg(windows)]
#[test]
fn identity_reads_package_version_resources_instead_of_the_first_registry_hint() {
    let windows = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
    let root = tempfile::tempdir().unwrap();
    let manifest_executable = root.path().join("product.exe");
    let registry_executable = root.path().join("registry-uninstaller.exe");
    // Use mandatory OS version resources without requiring an optional installed application.
    // These fixtures are read as metadata only and are never executed.
    std::fs::copy(windows.join("System32/version.dll"), &manifest_executable).unwrap();
    std::fs::copy(windows.join("System32/kernel32.dll"), &registry_executable).unwrap();
    let expected = mangodisk_platform::read_application_identity(&manifest_executable, None)
        .expect("the package fixture has native version resources");
    let unrelated = mangodisk_platform::read_application_identity(&registry_executable, None)
        .expect("the registry fixture has native version resources");
    let expected = serde_json::to_value(expected).unwrap();
    let unrelated = serde_json::to_value(unrelated).unwrap();
    assert_ne!(expected["fileDescription"], unrelated["fileDescription"]);
    let mut candidate = history_candidate("identity", "Example Package");
    candidate.platform = ApplicationUninstallPlatform::WindowsRegistry;
    candidate.source_identities = vec![
        ApplicationUninstallSourceIdentity {
            source: ApplicationUninstallInventorySource::WindowsRegistry,
            identifier: "private-registry-key".into(),
        },
        ApplicationUninstallSourceIdentity {
            source: ApplicationUninstallInventorySource::WindowsAppx,
            identifier: "Example_123".into(),
        },
    ];
    candidate.executable_paths = vec![registry_executable, manifest_executable.clone()];
    candidate.package_executable_path = Some(manifest_executable);
    let response =
        serde_json::to_value(ApplicationUninstallService::describe_identity(&candidate)).unwrap();
    assert_eq!(
        response["metadata"]["fileDescription"],
        expected["fileDescription"]
    );
    assert_eq!(response["metadata"]["packageIdentity"], "Example_123");
    assert!(serde_json::to_value(&candidate)
        .unwrap()
        .get("packageExecutablePath")
        .is_none());

    candidate.package_executable_path = None;
    let response =
        serde_json::to_value(ApplicationUninstallService::describe_identity(&candidate)).unwrap();
    assert_eq!(
        response["metadata"],
        serde_json::json!({"platform":"windows", "packageIdentity":"Example_123"})
    );
}

#[test]
fn close_target_prefers_exact_executable_paths_over_process_names() {
    let mut candidate = history_candidate("application-exact", "Exact Application");
    candidate.running_processes = vec!["SharedHelper".to_string()];
    candidate.executable_paths = vec![PathBuf::from(
        "/Applications/Exact.app/Contents/MacOS/SharedHelper",
    )];

    let target = close_target(&candidate);

    assert!(target.executable_names.is_empty());
    assert_eq!(target.executable_paths, candidate.executable_paths);
}

#[test]
fn close_target_keeps_stable_names_when_no_process_is_running() {
    let mut candidate = history_candidate("application-name", "Example Application");
    candidate.executable_paths.clear();

    let target = close_target(&candidate);

    assert!(target.executable_paths.is_empty());
    assert!(target
        .executable_names
        .iter()
        .any(|name| name == "Example Application"));
    assert!(target
        .executable_names
        .iter()
        .any(|name| name == "identifier.application-name"));
}

#[test]
fn close_result_updates_only_fully_stopped_catalog_targets() {
    let mut stopped = history_candidate("application-stopped", "Stopped Application");
    stopped.capability = ApplicationUninstallCapability::ApplicationRunning;
    stopped.running_processes = vec!["stopped-helper".to_string()];
    let mut remaining = history_candidate("application-remaining", "Remaining Application");
    remaining.capability = ApplicationUninstallCapability::ApplicationRunning;
    remaining.running_processes = vec!["remaining-helper".to_string()];
    let mut failed = history_candidate("application-failed", "Failed Application");
    failed.capability = ApplicationUninstallCapability::ApplicationRunning;
    failed.running_processes = vec!["failed-helper".to_string()];
    let mut scan = ApplicationUninstallScanResult {
        schema_version: APPLICATION_UNINSTALL_SCAN_SCHEMA_VERSION,
        scanned_at_ms: 1,
        supported: true,
        execution_supported: true,
        catalog_actionable: true,
        inventory_complete: true,
        catalog_revision: Some("revision-1".to_string()),
        candidates: vec![stopped, remaining, failed],
        ready_count: 0,
        blocked_count: 3,
        hidden_count: 0,
        related_directory_count: 0,
        related_path_scan_elapsed_ms: 0,
        elapsed_ms: 1,
    };
    let result = crate::ApplicationCloseBatchResult {
        mode: crate::ApplicationCloseMode::Graceful,
        matched_process_count: 2,
        requested_process_count: 2,
        remaining_process_count: 1,
        failed_target_count: 1,
        targets: vec![
            crate::ApplicationCloseTargetResult {
                target_id: "application-stopped".to_string(),
                status: crate::ApplicationCloseTargetStatus::Completed,
                matched_process_count: 1,
                requested_process_count: 1,
                remaining_processes: Vec::new(),
            },
            crate::ApplicationCloseTargetResult {
                target_id: "application-remaining".to_string(),
                status: crate::ApplicationCloseTargetStatus::Completed,
                matched_process_count: 1,
                requested_process_count: 1,
                remaining_processes: vec!["remaining-helper".to_string()],
            },
            crate::ApplicationCloseTargetResult {
                target_id: "application-failed".to_string(),
                status: crate::ApplicationCloseTargetStatus::Failed,
                matched_process_count: 0,
                requested_process_count: 0,
                remaining_processes: Vec::new(),
            },
        ],
        elapsed_ms: 1,
    };

    refresh_catalog_after_close(&mut scan, &result);

    assert!(scan.candidates[0].running_processes.is_empty());
    assert_ne!(
        scan.candidates[0].capability,
        ApplicationUninstallCapability::ApplicationRunning
    );
    assert_eq!(
        scan.candidates[1].capability,
        ApplicationUninstallCapability::ApplicationRunning
    );
    assert_eq!(
        scan.candidates[1].running_processes,
        vec!["remaining-helper"]
    );
    assert_eq!(
        scan.candidates[2].capability,
        ApplicationUninstallCapability::ApplicationRunning
    );
    assert_eq!(scan.candidates[2].running_processes, vec!["failed-helper"]);
    assert_eq!(scan.ready_count + scan.blocked_count, 3);
}

#[test]
fn uninstall_progress_tracks_serial_application_boundaries() {
    let mut snapshots = Vec::new();
    {
        let mut reporter = UninstallExecutionReporter::new(2, |progress| snapshots.push(progress));
        reporter.emit(ApplicationUninstallExecutionStage::Validating, None);

        let completed = batch_result_fixture("application-a", 1, 0, 100);
        reporter.emit(
            ApplicationUninstallExecutionStage::Uninstalling,
            Some(&completed.application_id),
        );
        reporter.record(&completed);
        reporter.emit(
            ApplicationUninstallExecutionStage::Uninstalling,
            Some(&completed.application_id),
        );

        let failed = batch_result_fixture("application-b", 0, 1, 0);
        reporter.emit(
            ApplicationUninstallExecutionStage::Uninstalling,
            Some(&failed.application_id),
        );
        reporter.record(&failed);
        reporter.emit(ApplicationUninstallExecutionStage::Finalizing, None);
    }

    assert_eq!(
        snapshots
            .iter()
            .map(|progress| progress.stage)
            .collect::<Vec<_>>(),
        vec![
            ApplicationUninstallExecutionStage::Validating,
            ApplicationUninstallExecutionStage::Uninstalling,
            ApplicationUninstallExecutionStage::Uninstalling,
            ApplicationUninstallExecutionStage::Uninstalling,
            ApplicationUninstallExecutionStage::Finalizing,
        ]
    );
    assert!(snapshots.iter().all(|progress| {
        progress.completed_application_count <= progress.total_application_count
    }));
    let final_snapshot = snapshots
        .last()
        .expect("final uninstall progress must be emitted");
    assert_eq!(final_snapshot.completed_application_count, 2);
    assert_eq!(final_snapshot.affected_application_count, 1);
    assert_eq!(final_snapshot.failed_application_count, 1);
    assert_eq!(final_snapshot.released_bytes, 100);
    assert_eq!(
        final_snapshot.completed_applications,
        vec![
            ApplicationUninstallExecutionItemResult {
                application_id: "application-a".to_string(),
                status: ApplicationUninstallExecutionItemStatus::Completed,
                released_bytes: 100,
            },
            ApplicationUninstallExecutionItemResult {
                application_id: "application-b".to_string(),
                status: ApplicationUninstallExecutionItemStatus::Failed,
                released_bytes: 0,
            },
        ]
    );
    let wire = serde_json::to_value(final_snapshot)
        .expect("execution progress should serialize for desktop events");
    assert_eq!(
        wire["completedApplications"][0]["applicationId"],
        "application-a"
    );
    assert_eq!(wire["completedApplications"][0]["status"], "completed");
}

#[test]
fn application_identity_is_stable_when_alias_order_changes() {
    let mut application = fixture_application();
    let expected = application_id(&application);
    application.identifiers.reverse();
    assert_eq!(application_id(&application), expected);
}

#[test]
fn application_identity_is_stable_when_inventory_sources_are_enriched() {
    let mut application = fixture_application();
    let expected = application_id(&application);
    application
        .source_identities
        .push(mangodisk_platform::ApplicationSourceIdentity {
            source: ApplicationInventorySource::Winget,
            identifier: "Example.Editor".to_string(),
        });
    application.identifiers.push("Example.Editor".to_string());
    assert_eq!(application_id(&application), expected);
}

#[test]
fn valid_applications_remain_visible_when_inventory_is_incomplete() {
    let applications = vec![fixture_application()];

    let discovered = discovered_candidates(&applications, Some(&ProcessSnapshot::default()));

    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].primary_identifier, "com.example.Editor");
}

#[test]
fn applications_are_hidden_when_process_detection_is_unavailable() {
    let applications = vec![fixture_application()];

    assert!(discovered_candidates(&applications, None).is_empty());
}

#[test]
fn current_application_identifier_is_excluded_from_uninstall_candidates() {
    let mut application = fixture_application();
    application.primary_identifier = crate::APPLICATION_IDENTIFIER.to_string();
    application.identifiers = vec![crate::APPLICATION_IDENTIFIER.to_string()];

    let discovered = discovered_candidates(&[application], Some(&ProcessSnapshot::default()));

    assert!(discovered.is_empty());
}

#[cfg(windows)]
#[test]
fn windows_product_registration_is_excluded_from_uninstall_candidates() {
    let mut application = fixture_application();
    application.primary_identifier = "future-installer-identity".to_string();
    application.identifiers = vec!["future-installer-identity".to_string()];
    application.name = CURRENT_APPLICATION_NAME.to_string();
    application.publisher = Some("Future MangoDisk Publisher".to_string());
    application.source_identities.clear();

    let discovered = discovered_candidates(&[application], Some(&ProcessSnapshot::default()));

    assert!(discovered.is_empty());
}

#[test]
fn ready_catalog_entry_requires_a_primary_uninstall_component() {
    let summary = |kind| ApplicationUninstallComponentSummary {
        component_id: "component".to_string(),
        kind,
        risk: super::super::models::ApplicationUninstallRisk::Rebuildable,
        path: None,
        bytes: 1,
        file_count: 1,
        default_selected: false,
    };

    assert!(!has_primary_component(&[summary(
        ApplicationUninstallComponentKind::Cache,
    )]));
    assert!(has_primary_component(&[summary(
        ApplicationUninstallComponentKind::ApplicationBinary,
    )]));
    assert!(has_primary_component(&[summary(
        ApplicationUninstallComponentKind::NativeInstaller,
    )]));
}

#[cfg(windows)]
#[test]
fn appx_candidate_does_not_expose_the_protected_package_directory() {
    let mut application = fixture_application();
    application.uninstall_registration = Some(ApplicationUninstallRegistration::WindowsAppx {
        package_family_name: "Example_123".to_string(),
        package_full_name: "Example_1.0.0.0_x64__123".to_string(),
        estimated_bytes: 42_000,
    });

    let candidate = candidate(&application, &ProcessSnapshot::default());

    assert!(candidate.application_path.is_none());
    assert_eq!(
        candidate.record_state,
        ApplicationUninstallRecordState::Installed
    );
    assert!(candidate.icon_path.is_some());
}

#[cfg(windows)]
#[test]
fn candidate_does_not_expose_a_stale_registered_directory() {
    let mut application = fixture_application();
    let missing = std::env::temp_dir().join(format!(
        "mangodisk-missing-application-{}",
        std::process::id()
    ));
    application.bundle_path = Some(missing.clone());
    application.executable_paths = vec![missing.join("application.exe")];

    let candidate = candidate(&application, &ProcessSnapshot::default());

    assert!(candidate.application_path.is_none());
    assert_eq!(
        candidate.record_state,
        ApplicationUninstallRecordState::OrphanedRegistration
    );
}

#[cfg(windows)]
#[test]
fn registered_executables_delegate_elevation_to_windows_shell() {
    let registration = |scope| ApplicationUninstallRegistration::WindowsRegistered {
        key_name: "Example".to_string(),
        scope,
        registry_view: mangodisk_platform::WindowsRegistryView::Registry64,
        command_kind: mangodisk_platform::WindowsRegisteredUninstallKind::Executable,
        command_digest: "a".repeat(64),
        estimated_bytes: 42_000,
    };
    let mut application = fixture_application();
    application.uninstall_registration = Some(registration(ApplicationInstallScope::Machine));
    assert_eq!(
        capability(&application, &[]),
        ApplicationUninstallCapability::Ready
    );

    application.uninstall_registration = Some(registration(ApplicationInstallScope::CurrentUser));
    assert_eq!(
        capability(&application, &[]),
        ApplicationUninstallCapability::Ready
    );
}

#[test]
fn batch_result_keeps_successful_applications_when_another_fails() {
    let plans = vec![
            plan::create_reviewed_plan(
                "application-a".to_string(),
                "revision-1".to_string(),
                vec![ApplicationUninstallPlanItem {
                    component_id: "component-a".to_string(),
                    kind: crate::applications::uninstall::models::ApplicationUninstallComponentKind::ApplicationBinary,
                    expected_bytes: 100,
                    expected_file_count: 1,
                    expected_snapshot_fingerprint: "a".repeat(64),
                }],
            )
            .expect("first application plan should be valid"),
            plan::create_reviewed_plan(
                "application-b".to_string(),
                "revision-1".to_string(),
                vec![ApplicationUninstallPlanItem {
                    component_id: "component-b".to_string(),
                    kind: crate::applications::uninstall::models::ApplicationUninstallComponentKind::ApplicationBinary,
                    expected_bytes: 100,
                    expected_file_count: 1,
                    expected_snapshot_fingerprint: "b".repeat(64),
                }],
            )
            .expect("second application plan should be valid"),
        ];
    let batch =
        batch::create_plan("revision-1".to_string(), plans).expect("batch plan should be valid");

    let results = vec![
        batch_result_fixture("application-a", 1, 0, 100),
        batch_result_fixture("application-b", 0, 1, 0),
    ];
    let history = execution_history_record(UninstallHistoryRecordInput {
        operation_id: 42,
        started_at_ms: 10,
        finished_at_ms: 25,
        batch_id: &batch.batch_id,
        expected_bytes: batch.expected_bytes,
        plans: &batch.plans,
        results: &results,
        candidates: &[
            history_candidate("application-a", "Application A"),
            history_candidate("application-b", "Application B"),
        ],
    })
    .expect("one batch history record should contain every selected application");
    assert_eq!(history.selected_item_count, 2);
    assert_eq!(history.affected_item_count, 1);
    assert_eq!(history.failed_item_count, 1);
    let OperationDetails::ApplicationUninstall(details) = history.details else {
        panic!("batch uninstall history must preserve application details");
    };
    assert_eq!(details.applications.len(), 2);
    assert_eq!(details.applications[0].application_name, "Application A");
    assert_eq!(details.applications[1].application_name, "Application B");

    let result = aggregate_batch_result(&batch, false, results);

    assert_eq!(result.selected_application_count, 2);
    assert_eq!(result.affected_application_count, 1);
    assert_eq!(result.failed_application_count, 1);
    assert_eq!(result.affected_item_count, 1);
    assert_eq!(result.failed_item_count, 1);
    assert_eq!(result.released_bytes, 100);
    assert_eq!(result.results.len(), 2);
}

#[test]
fn cancelled_applications_mark_batch_history_without_counting_as_failures() {
    let plans = vec![
        plan::create_reviewed_plan(
            "application-a".to_string(),
            "revision-1".to_string(),
            vec![ApplicationUninstallPlanItem {
                component_id: "component-a".to_string(),
                kind: ApplicationUninstallComponentKind::ApplicationBinary,
                expected_bytes: 100,
                expected_file_count: 1,
                expected_snapshot_fingerprint: "a".repeat(64),
            }],
        )
        .expect("first application plan should be valid"),
        plan::create_reviewed_plan(
            "application-b".to_string(),
            "revision-1".to_string(),
            vec![ApplicationUninstallPlanItem {
                component_id: "component-b".to_string(),
                kind: ApplicationUninstallComponentKind::ApplicationBinary,
                expected_bytes: 100,
                expected_file_count: 1,
                expected_snapshot_fingerprint: "b".repeat(64),
            }],
        )
        .expect("second application plan should be valid"),
    ];
    let batch =
        batch::create_plan("revision-1".to_string(), plans).expect("batch plan should be valid");
    let results = vec![
        batch_result_fixture("application-a", 1, 0, 100),
        preflight::cancel_all(&batch.plans[1], Some("Application B".to_string()), None),
    ];

    let history = execution_history_record(UninstallHistoryRecordInput {
        operation_id: 43,
        started_at_ms: 10,
        finished_at_ms: 25,
        batch_id: &batch.batch_id,
        expected_bytes: batch.expected_bytes,
        plans: &batch.plans,
        results: &results,
        candidates: &[
            history_candidate("application-a", "Application A"),
            history_candidate("application-b", "Application B"),
        ],
    })
    .expect("cancelled batch history should retain every selected application");
    let result = aggregate_batch_result(&batch, false, results);

    assert_eq!(history.outcome, OperationOutcome::Cancelled);
    assert_eq!(history.failed_item_count, 0);
    assert_eq!(result.selected_application_count, 2);
    assert_eq!(result.affected_application_count, 1);
    assert_eq!(result.failed_application_count, 0);
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "permanently uninstalls only the explicitly named disposable macOS bundle"]
fn real_macos_bundle_fixture_completes_the_full_uninstall_workflow() {
    init_real_fixture_logger();
    let application_name = std::env::var("MANGODISK_TEST_MACOS_UNINSTALL_NAME")
        .expect("set MANGODISK_TEST_MACOS_UNINSTALL_NAME to a disposable application bundle");
    assert!(
        !application_name.trim().is_empty(),
        "the disposable application name must not be empty"
    );
    let mut scan = ApplicationUninstallService::scan()
        .expect("the macOS application catalog should be available");
    let application_id = scan
        .candidates
        .iter()
        .find(|candidate| candidate.name == application_name)
        .map(|candidate| candidate.application_id.clone())
        .expect("the disposable application should be present");
    if scan
        .candidates
        .iter()
        .find(|candidate| candidate.application_id == application_id)
        .is_some_and(|candidate| {
            candidate.capability == ApplicationUninstallCapability::ApplicationRunning
        })
    {
        let close_result = ApplicationUninstallService::close_applications_from_catalog(
            ApplicationUninstallCloseRequest {
                application_ids: vec![application_id.clone()],
                mode: crate::ApplicationCloseMode::Force,
            },
            &mut scan,
        )
        .expect("the running disposable application should close");
        assert_eq!(close_result.remaining_process_count, 0);
    }
    let candidate = scan
        .candidates
        .iter()
        .find(|candidate| candidate.application_id == application_id)
        .expect("the disposable application should remain in the catalog snapshot");
    assert_eq!(candidate.capability, ApplicationUninstallCapability::Ready);
    let application_path = candidate
        .application_path
        .as_deref()
        .map(PathBuf::from)
        .expect("the disposable bundle path should be available");
    let component_ids = candidate
        .components
        .iter()
        .filter(|component| component.default_selected)
        .map(|component| component.component_id.clone())
        .collect::<Vec<_>>();
    let preparation = ApplicationUninstallService::prepare_batch_from_catalog(
        &[ApplicationUninstallBatchSelection {
            application_id: application_id.clone(),
            component_ids,
        }],
        &scan,
    )
    .expect("the disposable application should pass uninstall preflight");
    assert_eq!(preparation.preview.failed_application_count, 0);

    let result = ApplicationUninstallService::execute_batch(preparation.plan, false)
        .expect("the disposable application should uninstall");
    assert_eq!(result.affected_application_count, 1, "{result:#?}");
    assert_eq!(result.failed_application_count, 0, "{result:#?}");
    assert!(
        !application_path.exists(),
        "the disposable application bundle should be removed"
    );
    let refreshed =
        ApplicationUninstallService::scan().expect("the macOS application catalog should refresh");
    assert!(
        refreshed
            .candidates
            .iter()
            .all(|candidate| candidate.application_id != application_id),
        "the removed application must disappear from the refreshed catalog"
    );
}

#[cfg(windows)]
#[test]
#[ignore = "uninstalls only the explicitly named Scoop fixture"]
fn real_scoop_fixture_completes_the_full_uninstall_workflow() {
    let package_name = std::env::var("MANGODISK_TEST_SCOOP_UNINSTALL_PACKAGE")
        .expect("set MANGODISK_TEST_SCOOP_UNINSTALL_PACKAGE to a disposable Scoop package");
    real_package_fixture_completes_the_full_uninstall_workflow(
        &package_name,
        ApplicationUninstallInventorySource::Scoop,
        ApplicationUninstallInstallerKind::WindowsScoop,
        ApplicationUninstallExecutionMode::Silent,
        ApplicationUninstallCapability::Ready,
    );
}

#[cfg(windows)]
#[test]
#[ignore = "uninstalls only the explicitly named WinGet fixture"]
fn real_winget_fixture_completes_the_full_uninstall_workflow() {
    let package_name = std::env::var("MANGODISK_TEST_WINGET_UNINSTALL_PACKAGE")
        .expect("set MANGODISK_TEST_WINGET_UNINSTALL_PACKAGE to a disposable WinGet package");
    real_package_fixture_completes_the_full_uninstall_workflow(
        &package_name,
        ApplicationUninstallInventorySource::Winget,
        ApplicationUninstallInstallerKind::WindowsRegistered,
        ApplicationUninstallExecutionMode::ExternalClient,
        ApplicationUninstallCapability::Ready,
    );
}

#[cfg(windows)]
#[test]
#[ignore = "uninstalls only the explicitly named Chocolatey fixture"]
fn real_chocolatey_fixture_completes_the_full_uninstall_workflow() {
    let package_name = std::env::var("MANGODISK_TEST_CHOCOLATEY_UNINSTALL_PACKAGE")
        .expect("set MANGODISK_TEST_CHOCOLATEY_UNINSTALL_PACKAGE to a disposable package");
    real_package_fixture_completes_the_full_uninstall_workflow(
        &package_name,
        ApplicationUninstallInventorySource::Chocolatey,
        ApplicationUninstallInstallerKind::WindowsChocolatey,
        ApplicationUninstallExecutionMode::ExternalClient,
        ApplicationUninstallCapability::RequiresElevation,
    );
}

#[cfg(windows)]
#[test]
#[ignore = "uninstalls only the explicitly named registered application fixture"]
fn real_registered_fixture_completes_the_full_uninstall_workflow() {
    let application_name = std::env::var("MANGODISK_TEST_REGISTERED_UNINSTALL_NAME").expect(
        "set MANGODISK_TEST_REGISTERED_UNINSTALL_NAME to a disposable registered application",
    );
    real_named_fixture_completes_the_full_uninstall_workflow(
        &application_name,
        ApplicationUninstallInstallerKind::WindowsRegistered,
        ApplicationUninstallExecutionMode::Interactive,
        ApplicationUninstallCapability::Ready,
    );
}

#[cfg(windows)]
#[test]
#[ignore = "uninstalls only the explicitly named MSI application fixture"]
fn real_msi_fixture_completes_the_full_uninstall_workflow() {
    let application_name = std::env::var("MANGODISK_TEST_MSI_UNINSTALL_NAME")
        .expect("set MANGODISK_TEST_MSI_UNINSTALL_NAME to a disposable MSI application");
    real_named_fixture_completes_the_full_uninstall_workflow(
        &application_name,
        ApplicationUninstallInstallerKind::WindowsMsi,
        ApplicationUninstallExecutionMode::Silent,
        ApplicationUninstallCapability::RequiresElevation,
    );
}

#[cfg(windows)]
#[test]
#[ignore = "uninstalls only the explicitly named AppX application fixture"]
fn real_appx_fixture_completes_the_full_uninstall_workflow() {
    let application_name = std::env::var("MANGODISK_TEST_APPX_UNINSTALL_NAME")
        .expect("set MANGODISK_TEST_APPX_UNINSTALL_NAME to a disposable AppX application");
    real_named_fixture_completes_the_full_uninstall_workflow(
        &application_name,
        ApplicationUninstallInstallerKind::WindowsAppx,
        ApplicationUninstallExecutionMode::Silent,
        ApplicationUninstallCapability::Ready,
    );
}

#[cfg(windows)]
#[test]
#[ignore = "reads only the explicitly named running application fixture"]
fn real_running_fixture_is_blocked_before_uninstall() {
    let application_name = std::env::var("MANGODISK_TEST_RUNNING_APPLICATION_NAME").expect(
        "set MANGODISK_TEST_RUNNING_APPLICATION_NAME to an application that is currently running",
    );
    init_real_fixture_logger();
    let scan = ApplicationUninstallService::scan()
        .expect("the Windows application catalog should be available");
    let candidate = scan
        .candidates
        .iter()
        .find(|candidate| candidate.name.eq_ignore_ascii_case(&application_name))
        .expect("the running application should be present");

    assert_eq!(
        candidate.capability,
        ApplicationUninstallCapability::ApplicationRunning
    );
    assert!(
        !candidate.running_processes.is_empty(),
        "the blocked candidate should include running-process evidence"
    );
}

#[cfg(windows)]
#[test]
#[ignore = "force-closes and uninstalls only the explicitly named disposable registered fixture"]
fn real_running_registered_fixture_force_closes_and_uninstalls() {
    let application_name = std::env::var("MANGODISK_TEST_RUNNING_REGISTERED_UNINSTALL_NAME")
            .expect(
                "set MANGODISK_TEST_RUNNING_REGISTERED_UNINSTALL_NAME to a disposable registered application",
            );
    init_real_fixture_logger();
    let mut scan = ApplicationUninstallService::scan()
        .expect("the Windows application catalog should be available");
    let candidate = scan
        .candidates
        .iter()
        .find(|candidate| candidate.name.eq_ignore_ascii_case(&application_name))
        .expect("the running disposable application should be present");
    assert_eq!(
        candidate.capability,
        ApplicationUninstallCapability::ApplicationRunning
    );
    let application_id = candidate.application_id.clone();
    let close_result = ApplicationUninstallService::close_applications_from_catalog(
        ApplicationUninstallCloseRequest {
            application_ids: vec![application_id.clone()],
            mode: crate::ApplicationCloseMode::Force,
        },
        &mut scan,
    )
    .expect("the disposable application should force-close");
    assert_eq!(close_result.matched_process_count, 1);
    assert_eq!(close_result.remaining_process_count, 0);
    let stopped_candidate = scan
        .candidates
        .iter()
        .find(|candidate| candidate.application_id == application_id)
        .expect("the stopped candidate should remain in the catalog snapshot");
    assert!(stopped_candidate.running_processes.is_empty());
    assert_eq!(
        stopped_candidate.capability,
        ApplicationUninstallCapability::Ready
    );

    real_named_fixture_completes_the_full_uninstall_workflow(
        &application_name,
        ApplicationUninstallInstallerKind::WindowsRegistered,
        ApplicationUninstallExecutionMode::Interactive,
        ApplicationUninstallCapability::Ready,
    );
}

#[cfg(windows)]
fn real_named_fixture_completes_the_full_uninstall_workflow(
    application_name: &str,
    expected_installer_kind: ApplicationUninstallInstallerKind,
    expected_execution_mode: ApplicationUninstallExecutionMode,
    expected_capability: ApplicationUninstallCapability,
) {
    init_real_fixture_logger();
    assert!(
        !application_name.trim().is_empty(),
        "the disposable application name must not be empty"
    );
    let scan = ApplicationUninstallService::scan()
        .expect("the Windows application catalog should be available");
    let candidate = scan
        .candidates
        .iter()
        .find(|candidate| candidate.name.eq_ignore_ascii_case(application_name))
        .expect("the disposable application should be present");
    assert_eq!(candidate.installer_kind, Some(expected_installer_kind));
    assert_eq!(candidate.execution_mode, Some(expected_execution_mode));
    assert_eq!(candidate.capability, expected_capability);
    let application_id = candidate.application_id.clone();
    let component_ids = candidate
        .components
        .iter()
        .filter(|component| component.default_selected)
        .map(|component| component.component_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        component_ids.len(),
        1,
        "an application should expose one required native component"
    );
    let selection = ApplicationUninstallBatchSelection {
        application_id: application_id.clone(),
        component_ids,
    };
    let preparation = ApplicationUninstallService::prepare_batch_from_catalog(&[selection], &scan)
        .expect("the application uninstall should pass preflight");
    assert_eq!(preparation.preview.failed_application_count, 0);
    assert_eq!(preparation.preview.previewed_application_count, 1);

    let result = ApplicationUninstallService::execute_batch(preparation.plan, false)
        .expect("the application uninstall should execute");
    assert_eq!(
        result.affected_application_count, 1,
        "the native application uninstall should affect one application: {result:#?}"
    );
    assert_eq!(
        result.failed_application_count, 0,
        "the native application uninstall should not fail: {result:#?}"
    );

    let refreshed = ApplicationUninstallService::scan()
        .expect("the Windows application catalog should refresh");
    assert!(
        refreshed
            .candidates
            .iter()
            .all(|candidate| candidate.application_id != application_id),
        "the removed application must disappear from the refreshed catalog"
    );
}

#[cfg(windows)]
fn real_package_fixture_completes_the_full_uninstall_workflow(
    package_name: &str,
    source: ApplicationUninstallInventorySource,
    expected_installer_kind: ApplicationUninstallInstallerKind,
    expected_execution_mode: ApplicationUninstallExecutionMode,
    expected_capability: ApplicationUninstallCapability,
) {
    init_real_fixture_logger();
    assert!(
        !package_name.trim().is_empty(),
        "the disposable package name must not be empty"
    );
    let expected_identifier = match source {
        ApplicationUninstallInventorySource::Scoop => {
            format!("current-user:{package_name}")
        }
        _ => package_name.to_string(),
    };
    let scan = ApplicationUninstallService::scan()
        .expect("the Windows application catalog should be available");
    let candidate = scan
        .candidates
        .iter()
        .find(|candidate| {
            candidate.source_identities.iter().any(|identity| {
                identity.source == source
                    && identity
                        .identifier
                        .eq_ignore_ascii_case(&expected_identifier)
            })
        })
        .expect("the disposable package should be present");
    assert_eq!(candidate.installer_kind, Some(expected_installer_kind));
    assert_eq!(candidate.execution_mode, Some(expected_execution_mode));
    assert_eq!(candidate.capability, expected_capability);
    let component_ids = candidate
        .components
        .iter()
        .filter(|component| component.default_selected)
        .map(|component| component.component_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        component_ids.len(),
        1,
        "a package should expose one required native component"
    );
    let selection = ApplicationUninstallBatchSelection {
        application_id: candidate.application_id.clone(),
        component_ids,
    };
    let preparation = ApplicationUninstallService::prepare_batch_from_catalog(&[selection], &scan)
        .expect("the package uninstall should pass preflight");
    assert_eq!(preparation.preview.failed_application_count, 0);
    assert_eq!(preparation.preview.previewed_application_count, 1);

    let result = ApplicationUninstallService::execute_batch(preparation.plan, false)
        .expect("the package uninstall should execute");
    assert_eq!(
        result.affected_application_count, 1,
        "the native package uninstall should affect one application: {result:#?}"
    );
    assert_eq!(
        result.failed_application_count, 0,
        "the native package uninstall should not fail: {result:#?}"
    );

    let refreshed = ApplicationUninstallService::scan()
        .expect("the Windows application catalog should refresh");
    assert!(
        refreshed.candidates.iter().all(|candidate| {
            candidate.source_identities.iter().all(|identity| {
                identity.source != source
                    || !identity
                        .identifier
                        .eq_ignore_ascii_case(&expected_identifier)
            })
        }),
        "the removed package must disappear from the refreshed catalog"
    );
}

#[cfg(any(windows, target_os = "macos"))]
fn init_real_fixture_logger() {
    struct FixtureLogger;

    impl log::Log for FixtureLogger {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() <= log::Level::Debug
        }

        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                eprintln!("{} {}: {}", record.level(), record.target(), record.args());
            }
        }

        fn flush(&self) {}
    }

    static LOGGER: FixtureLogger = FixtureLogger;
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Debug);
}

#[cfg(target_os = "macos")]
#[test]
fn system_application_is_never_ready() {
    let mut application = fixture_application();
    application.bundle_path = Some(PathBuf::from("/System/Applications/Mail.app"));
    assert_eq!(
        capability(&application, &[]),
        ApplicationUninstallCapability::ProtectedApplication
    );
}

#[cfg(target_os = "macos")]
#[test]
fn non_writable_bundle_outside_supported_roots_remains_view_only() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let root = std::env::temp_dir().join(format!(
        "mangodisk-uninstall-capability-root-{}",
        std::process::id()
    ));
    let bundle = root.join("Example.app");
    fs::create_dir_all(bundle.join("Contents"))
        .expect("the disposable application bundle should be created");
    fs::set_permissions(&bundle, fs::Permissions::from_mode(0o555))
        .expect("the disposable bundle should become read-only");

    let mut application = fixture_application();
    application.bundle_path = Some(bundle.clone());
    assert_eq!(
        capability(&application, &[]),
        ApplicationUninstallCapability::ViewOnly
    );

    fs::set_permissions(&bundle, fs::Permissions::from_mode(0o755))
        .expect("the disposable bundle should become removable again");
    fs::remove_dir_all(root).expect("the disposable application bundle should be removed");
}

#[cfg(target_os = "macos")]
#[test]
fn non_writable_nested_bundle_outside_supported_roots_remains_view_only() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let root = std::env::temp_dir().join(format!(
        "mangodisk-uninstall-capability-nested-{}",
        std::process::id()
    ));
    let bundle = root.join("Example.app");
    let protected = bundle.join("Contents/Resources/Protected");
    fs::create_dir_all(&protected)
        .expect("the disposable nested application directory should be created");
    fs::set_permissions(&protected, fs::Permissions::from_mode(0o555))
        .expect("the disposable nested directory should become read-only");

    let mut application = fixture_application();
    application.bundle_path = Some(bundle.clone());
    assert_eq!(
        capability(&application, &[]),
        ApplicationUninstallCapability::ViewOnly
    );

    fs::set_permissions(&protected, fs::Permissions::from_mode(0o755))
        .expect("the disposable nested directory should become removable again");
    fs::remove_dir_all(root).expect("the disposable application bundle should be removed");
}

#[cfg(target_os = "macos")]
#[test]
fn supported_application_roots_advertise_elevation_for_non_deletable_bundles() {
    let mut application = fixture_application();
    application.bundle_path = Some(PathBuf::from("/Applications/Example.APP"));
    assert_eq!(
        capability(&application, &[]),
        ApplicationUninstallCapability::RequiresElevation
    );

    application.bundle_path = Some(
        current_platform()
            .user_directories()
            .expect("the test requires macOS user directories")
            .home_directory()
            .join("Applications/Example.app"),
    );
    assert_eq!(
        capability(&application, &[]),
        ApplicationUninstallCapability::RequiresElevation
    );
}

#[cfg(target_os = "macos")]
#[test]
fn application_requiring_elevation_remains_visible_in_the_catalog() {
    let mut candidate = candidate(&fixture_application(), &ProcessSnapshot::default());
    candidate.capability = ApplicationUninstallCapability::RequiresElevation;

    assert!(is_visible_candidate(&candidate));
}
