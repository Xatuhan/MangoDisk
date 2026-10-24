use super::*;
use std::{fs::FileTimes, os::windows::fs::OpenOptionsExt, process::Command};

struct DiagnosticLogger;
impl log::Log for DiagnosticLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target().ends_with("windows_system_temp")
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("{}", record.args());
        }
    }
    fn flush(&self) {}
}

fn enable_diagnostics() {
    if log::set_logger(&DiagnosticLogger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

fn write_file(root: &Path, name: &str, contents: &[u8], modified: SystemTime) -> PathBuf {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    path
}

fn stale() -> SystemTime {
    SystemTime::now() - RETENTION - Duration::from_secs(60)
}

#[test]
fn system_temp_cancelled_dispatch_preserves_delete_action_kind() {
    use crate::{
        cleanup::source_selection::SourceSelectionPolicy,
        shared::operation::{test_operation_lock, CoordinatedOperationKind},
        system_inventory::SystemInventorySnapshot,
    };

    let _operation_lock = test_operation_lock();
    let operation = OperationGuard::start(CoordinatedOperationKind::Cleanup).unwrap();
    OperationGuard::cancel(CoordinatedOperationKind::Cleanup);
    let actions = super::super::execute_selected(
        &[CLEANER_ID.to_string()],
        &SystemInventorySnapshot::fixture(Vec::new(), true),
        &[],
        &[],
        &SourceSelectionPolicy::empty(),
        false,
        &operation,
    );
    assert_eq!(actions.len(), 1);
    let action = &actions[0];
    assert_eq!(action.status, CleanupActionStatus::Blocked);
    assert_eq!(action.reason_code, Some(CleanupActionReason::Cancelled));
    assert_eq!(action.affected_item_count, 0);
    assert_eq!(action.action_kind, CleanupActionKind::Delete);
}

#[test]
fn system_temp_excluded_dispatch_preserves_delete_action_kind() {
    use crate::{
        cleanup::{exclusions::CleanupExclusions, source_selection::SourceSelectionPolicy},
        filesystem::ScanExclusionOptions,
        shared::operation::{test_operation_lock, CoordinatedOperationKind},
        system_inventory::SystemInventorySnapshot,
    };
    use mangodisk_platform::{ExcludedNameKind, ScanNameExclusion};

    let _operation_lock = test_operation_lock();
    let operation = OperationGuard::start(CoordinatedOperationKind::Cleanup).unwrap();
    let exclusions = CleanupExclusions::resolve_options(&ScanExclusionOptions {
        paths: Vec::new(),
        names: vec![ScanNameExclusion {
            name: "keep.bin".to_string(),
            kind: ExcludedNameKind::File,
        }],
    })
    .unwrap();
    let actions = super::super::execute_selected_with_progress(
        super::super::CleanerExecutionRequest {
            rule_ids: &[CLEANER_ID.to_string()],
            inventory: &SystemInventorySnapshot::fixture(Vec::new(), true),
            declared_roots: &[],
            project_roots: &[],
            selected_volume_scope: false,
            source_selections: &SourceSelectionPolicy::empty(),
            dry_run: false,
            operation: &operation,
            exclusions: &exclusions,
        },
        |_, _| {},
    );
    assert_eq!(actions.len(), 1);
    let action = &actions[0];
    assert_eq!(action.status, CleanupActionStatus::Blocked);
    assert_eq!(
        action.reason_code,
        Some(CleanupActionReason::NameExclusionsUnsupported)
    );
    assert_eq!(action.affected_item_count, 0);
    assert_eq!(action.action_kind, CleanupActionKind::Delete);
}

#[test]
fn system_temp_keeps_recent_future_readonly_and_nested_recent_files() {
    let fixture = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let old = write_file(fixture.path(), "nested/old.bin", b"old", stale());
    let recent = write_file(fixture.path(), "nested/recent.bin", b"recent", now);
    let future = write_file(fixture.path(), "future.bin", b"future", now + RETENTION);
    let boundary = write_file(fixture.path(), "boundary.bin", b"boundary", now - RETENTION);
    let readonly = write_file(fixture.path(), "readonly.bin", b"readonly", stale());
    let original_permissions = fs::metadata(&readonly).unwrap().permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&readonly, permissions).unwrap();
    let inventory = discover(fixture.path(), now, &|| false).unwrap().unwrap();
    assert_eq!(inventory.candidates.len(), 1);
    assert_eq!(inventory.bytes(), 3);
    let preview = execute_inventory(inventory, true, &|| false);
    assert_eq!(preview.status, CleanupActionStatus::Previewed);
    assert_eq!(preview.released_bytes, 0);
    assert_eq!(preview.affected_item_count, 0);
    assert!(old.exists());
    let inventory = discover(fixture.path(), now, &|| false).unwrap().unwrap();
    let result = execute_inventory(inventory, false, &|| false);
    assert_eq!(result.status, CleanupActionStatus::Completed);
    assert_eq!(result.released_bytes, 3);
    assert_eq!(result.affected_item_count, 1);
    assert!(!old.exists());
    for path in [recent, future, boundary, readonly.clone()] {
        assert!(path.exists());
    }
    assert!(fixture.path().join("nested").is_dir());
    fs::set_permissions(readonly, original_permissions).unwrap();
}

#[test]
fn system_temp_locked_file_produces_partial_result_and_can_be_retried() {
    let fixture = tempfile::tempdir().unwrap();
    let locked = write_file(fixture.path(), "locked.bin", b"locked", stale());
    let removable = write_file(fixture.path(), "removable.bin", b"removed", stale());
    let handle = fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(&locked)
        .unwrap();
    let scan = preview_root(fixture.path(), Instant::now(), &|| false, Some(true));
    assert_eq!(scan.status, ScanItemStatus::Found);
    assert!(!scan.default_selected && !scan.recommended_selected);
    let result = execute_root(fixture.path(), false, &|| false);
    assert_eq!(result.status, CleanupActionStatus::Partial);
    assert_eq!(result.reason_code, Some(CleanupActionReason::ItemsSkipped));
    assert_eq!(result.released_bytes, 7);
    assert_eq!(result.failed_item_count, 1);
    assert_eq!(result.affected_item_count, 1);
    assert!(locked.exists());
    assert!(!removable.exists());
    drop(handle);
    let retry = execute_root(fixture.path(), false, &|| false);
    assert_eq!(retry.status, CleanupActionStatus::Completed);
    assert_eq!(retry.released_bytes, 6);
    assert!(!locked.exists());
}

struct DenyAccess {
    path: PathBuf,
}
impl DenyAccess {
    fn new(path: &Path, rights: &str) -> Self {
        let output = Command::new("icacls.exe")
            .arg(path)
            .args(["/deny", &format!("*S-1-1-0:({rights})")])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "failed to apply fixture ACL: {output:?}"
        );
        Self {
            path: path.to_path_buf(),
        }
    }
}
impl Drop for DenyAccess {
    fn drop(&mut self) {
        if self.path.try_exists().is_ok_and(|exists| !exists) {
            return;
        }
        let output = Command::new("icacls.exe")
            .arg(&self.path)
            .args(["/remove:d", "*S-1-1-0"])
            .output()
            .unwrap();
        if !output.status.success() {
            eprintln!("failed to restore fixture ACL: {output:?}");
            assert!(std::thread::panicking(), "fixture ACL restoration failed");
        }
    }
}

#[test]
fn system_temp_unreadable_directory_blocks_all_mutations() {
    let fixture = tempfile::tempdir().unwrap();
    let old = write_file(fixture.path(), "old.bin", b"old", stale());
    let _denied = DenyAccess::new(fixture.path(), "RD");
    for (elevated, expected) in [
        (Some(false), ScanItemStatus::RequiresElevation),
        (Some(true), ScanItemStatus::Limited),
        (None, ScanItemStatus::Limited),
    ] {
        assert_eq!(
            preview_root(fixture.path(), Instant::now(), &|| false, elevated).status,
            expected
        );
    }
    let result = execute_root(fixture.path(), false, &|| false);
    assert_eq!(result.status, CleanupActionStatus::Failed);
    assert_eq!(
        result.reason_code,
        Some(CleanupActionReason::PreflightFailed)
    );
    assert_eq!(result.released_bytes, 0);
    assert!(old.exists());
}

#[test]
fn system_temp_denied_delete_preserves_file_and_reports_partial() {
    enable_diagnostics();
    let fixture = tempfile::tempdir().unwrap();
    let denied = write_file(fixture.path(), "denied.bin", b"denied", stale());
    let removable = write_file(fixture.path(), "removable.bin", b"removed", stale());
    let _directory_acl = DenyAccess::new(fixture.path(), "DC");
    // Basic D also denies Synchronize and prevents metadata reads without backup privileges.
    // Specific DE isolates deletion denial so the inventory remains readable.
    let _file_acl = DenyAccess::new(&denied, "DE");
    assert_eq!(
        probe_windows_file_delete_access(&denied)
            .unwrap_err()
            .raw_os_error(),
        Some(5)
    );
    let result = execute_root(fixture.path(), false, &|| false);
    assert_eq!(result.status, CleanupActionStatus::Partial);
    assert_eq!(result.failed_item_count, 1);
    assert_eq!(result.released_bytes, 7);
    assert!(denied.exists());
    drop(_file_acl);
    drop(_directory_acl);
    assert_eq!(fs::read(denied).unwrap(), b"denied");
    assert!(!removable.exists());
}

#[test]
fn system_temp_replaced_or_recently_modified_candidate_is_preserved() {
    for replace in [false, true] {
        let fixture = tempfile::tempdir().unwrap();
        let path = write_file(fixture.path(), "candidate.bin", b"original", stale());
        let inventory = discover(fixture.path(), SystemTime::now(), &|| false)
            .unwrap()
            .unwrap();
        let modified = inventory.candidates[0].modified;
        if replace {
            // Keep the original inode alive so even an aggressive file-ID allocator cannot reuse it.
            fs::rename(&path, fixture.path().join("original.bin")).unwrap();
            write_file(fixture.path(), "candidate.bin", b"replaced", modified);
        } else {
            fs::write(&path, b"modified").unwrap();
        }
        let result = execute_inventory(inventory, false, &|| false);
        assert_eq!(result.status, CleanupActionStatus::Failed);
        assert_eq!(result.released_bytes, 0);
        assert!(path.exists());
    }
}

#[test]
fn system_temp_replaced_parent_preserves_the_original_file() {
    let fixture = tempfile::tempdir().unwrap();
    let path = write_file(fixture.path(), "nested/candidate.bin", b"original", stale());
    let inventory = discover(fixture.path(), SystemTime::now(), &|| false)
        .unwrap()
        .unwrap();
    let parent = path.parent().unwrap();
    let original_parent = fixture.path().join("original-parent");
    fs::rename(parent, &original_parent).unwrap();
    fs::create_dir(parent).unwrap();
    // Preserve the file identity and timestamp, changing only the owning directory.
    fs::rename(original_parent.join("candidate.bin"), &path).unwrap();
    let result = execute_inventory(inventory, false, &|| false);
    assert_eq!(result.status, CleanupActionStatus::Failed);
    assert_eq!(result.released_bytes, 0);
    assert_eq!(fs::read(path).unwrap(), b"original");
}

#[test]
fn system_temp_incomplete_deep_inventory_keeps_all_files() {
    let fixture = tempfile::tempdir().unwrap();
    let old = write_file(fixture.path(), "old.bin", b"old", stale());
    let mut deepest = fixture.path().to_path_buf();
    for _ in 0..=MAX_DEPTH {
        deepest.push("d");
    }
    fs::create_dir_all(deepest).unwrap();
    let preview = preview_root(fixture.path(), Instant::now(), &|| false, Some(true));
    assert_eq!(preview.status, ScanItemStatus::Limited);
    assert!(!preview.selectable);
    let result = execute_root(fixture.path(), false, &|| false);
    assert_eq!(
        result.reason_code,
        Some(CleanupActionReason::PreflightFailed)
    );
    assert_eq!(result.affected_item_count, 0);
    assert_eq!(fs::read(old).unwrap(), b"old");
}

#[test]
fn system_temp_zero_byte_files_are_selectable_and_counted() {
    let fixture = tempfile::tempdir().unwrap();
    let path = write_file(fixture.path(), "empty.bin", b"", stale());
    let preview = preview_root(fixture.path(), Instant::now(), &|| false, Some(true));
    assert_eq!(preview.status, ScanItemStatus::Found);
    assert!(preview.selectable);
    assert_eq!(preview.bytes, 0);
    assert_eq!(preview.file_count, 1);
    let result = execute_root(fixture.path(), false, &|| false);
    assert_eq!(result.status, CleanupActionStatus::Completed);
    assert_eq!(result.released_bytes, 0);
    assert_eq!(result.affected_item_count, 1);
    assert!(!path.exists());
}

#[test]
fn system_temp_junction_never_exposes_external_files() {
    let fixture = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let protected = write_file(external.path(), "keep.bin", b"keep", stale());
    let link = fixture.path().join("junction");
    let output = Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(external.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "create fixture junction: {output:?}"
    );
    let root_scan = preview_root(&link, Instant::now(), &|| false, Some(true));
    let result = execute_root(fixture.path(), false, &|| false);
    // Remove only the junction itself before TempDir recursively cleans the fixture.
    fs::remove_dir(&link).unwrap();
    assert_eq!(root_scan.status, ScanItemStatus::Limited);
    assert_eq!(result.affected_item_count, 0);
    assert_eq!(fs::read(protected).unwrap(), b"keep");
}

#[test]
fn system_temp_cancel_after_first_file_keeps_truthful_counts() {
    let fixture = tempfile::tempdir().unwrap();
    let first = write_file(fixture.path(), "a.bin", b"first", stale());
    let second = write_file(fixture.path(), "b.bin", b"second", stale());
    let inventory = discover(fixture.path(), SystemTime::now(), &|| false)
        .unwrap()
        .unwrap();
    let result = execute_inventory(inventory, false, &|| !first.exists());
    assert_eq!(result.status, CleanupActionStatus::Partial);
    assert_eq!(result.reason_code, Some(CleanupActionReason::Cancelled));
    assert_eq!(result.released_bytes, 5);
    assert_eq!(result.affected_item_count, 1);
    assert!(second.exists());
}

#[test]
fn system_temp_missing_empty_and_cancelled_scans_remain_distinct() {
    let fixture = tempfile::tempdir().unwrap();
    assert_eq!(
        preview_root(
            &fixture.path().join("absent"),
            Instant::now(),
            &|| false,
            Some(false)
        )
        .status,
        ScanItemStatus::NotApplicable
    );
    assert_eq!(
        preview_root(fixture.path(), Instant::now(), &|| false, Some(false)).status,
        ScanItemStatus::Clean
    );
    assert_eq!(
        preview_root(fixture.path(), Instant::now(), &|| true, Some(false)).status,
        ScanItemStatus::Limited
    );
    assert_eq!(
        execute_root(fixture.path(), false, &|| true).reason_code,
        Some(CleanupActionReason::Cancelled)
    );
}

#[test]
fn system_temp_post_cleanup_access_loss_reports_verification_failure() {
    let fixture = tempfile::tempdir().unwrap();
    let first = write_file(fixture.path(), "a.bin", b"first", stale());
    let second = write_file(fixture.path(), "b.bin", b"second", stale());
    let inventory = discover(fixture.path(), SystemTime::now(), &|| false)
        .unwrap()
        .unwrap();
    let denied = std::cell::RefCell::new(None);
    let result = execute_inventory(inventory, false, &|| {
        if !first.exists() {
            *denied.borrow_mut() = Some(DenyAccess::new(fixture.path(), "RD"));
            true
        } else {
            false
        }
    });
    drop(denied);
    assert_eq!(result.status, CleanupActionStatus::Partial);
    assert_eq!(
        result.reason_code,
        Some(CleanupActionReason::VerificationFailed)
    );
    assert_eq!(result.released_bytes, 5);
    assert_eq!(result.affected_item_count, 1);
    assert!(second.exists());
}

fn actual_execute(dry_run: bool) -> CleanupActionResult {
    let result = crate::cleanup::CleanupService::execute(crate::cleanup::CleanupRequest {
        rule_ids: vec![CLEANER_ID.to_string()],
        source_selections: Vec::new(),
        dry_run,
        project_roots: Vec::new(),
    })
    .expect("execute the registered SystemTemp cleaner through Core");
    result
        .actions
        .into_iter()
        .find(|action| action.rule_id == CLEANER_ID)
        .expect("the result must contain the SystemTemp action")
}

#[test]
#[ignore = "requires explicit authorization to clean the real Windows SystemTemp directory"]
fn actual_windows_system_temp_cleanup() {
    enable_diagnostics();
    let _operation_lock = crate::shared::operation::test_operation_lock();
    assert_eq!(
        std::env::var("MANGODISK_TEST_SYSTEM_TEMP_APPLY").as_deref(),
        Ok("1")
    );
    let root = windows_system_temporary_directory().unwrap();
    let fixture = tempfile::Builder::new()
        .prefix("mangodisk-system-temp-")
        .tempdir_in(&root)
        .unwrap();
    let old = write_file(fixture.path(), "old.bin", b"old", stale());
    let recent = write_file(fixture.path(), "recent.bin", b"recent", SystemTime::now());
    let locked = write_file(fixture.path(), "locked.bin", b"locked", stale());
    let denied = write_file(fixture.path(), "denied.bin", b"denied", stale());
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(&locked)
        .unwrap();
    let directory_acl = DenyAccess::new(fixture.path(), "DC");
    let file_acl = DenyAccess::new(&denied, "DE");
    let before = preview_root(
        &root,
        Instant::now(),
        &|| false,
        windows_process_is_elevated().ok(),
    );
    println!(
        "system_temp_before={}",
        serde_json::to_string(&before).unwrap()
    );
    let dry_run = actual_execute(true);
    println!(
        "system_temp_dry_run={}",
        serde_json::to_string(&dry_run).unwrap()
    );
    assert_eq!(dry_run.status, CleanupActionStatus::Previewed);
    assert!(old.exists() && locked.exists() && denied.exists());
    let result = actual_execute(false);
    println!(
        "system_temp_result={}",
        serde_json::to_string(&result).unwrap()
    );
    let after = preview_root(
        &root,
        Instant::now(),
        &|| false,
        windows_process_is_elevated().ok(),
    );
    println!(
        "system_temp_after={}",
        serde_json::to_string(&after).unwrap()
    );
    assert!(root.is_dir());
    assert_eq!(result.status, CleanupActionStatus::Partial);
    assert!(!old.exists());
    assert_eq!(fs::read(&recent).unwrap(), b"recent");
    assert!(locked.exists() && denied.exists());
    drop(lock);
    drop(file_acl);
    drop(directory_acl);
    let retry = actual_execute(false);
    println!(
        "system_temp_retry={}",
        serde_json::to_string(&retry).unwrap()
    );
    assert!(!locked.exists() && !denied.exists());
    assert_eq!(fs::read(&recent).unwrap(), b"recent");
}

#[test]
#[ignore = "requires a real non-elevated Windows desktop token"]
fn actual_windows_system_temp_non_admin_preview() {
    assert!(!windows_process_is_elevated().unwrap());
    let result = preview(&PlatformCancellation::new(|| false));
    println!(
        "system_temp_non_admin={}",
        serde_json::to_string(&result).unwrap()
    );
    assert_eq!(result.status, ScanItemStatus::RequiresElevation);
    assert!(!result.selectable);
    assert!(!result.default_selected && !result.recommended_selected);
}
