use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use mangodisk_platform::{
    current_platform, probe_windows_file_delete_access, windows_process_is_elevated,
    windows_system_temporary_directory, Platform, PlatformCancellation,
};

use crate::{
    cleanup::{
        CleanupActionKind, CleanupActionReason, CleanupActionResult, CleanupActionStatus,
        CleanupCategory, CleanupGroup, RiskLevel, ScanItemStatus, ScanRuleResult,
    },
    filesystem::{
        metadata::{diagnostic_path, is_link_like},
        permanent_delete::{
            delete_path_permanently, physical_path_identity_snapshot,
            prepare_path_for_permanent_delete, PermanentDeleteError, PhysicalPathIdentity,
        },
    },
    shared::operation::OperationGuard,
};

pub(super) const CLEANER_ID: &str = "special.windows-system-temp";
pub(super) const CLEANER_REVISION: &str = "windows-system-temp-v1-seven-day-files";
const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MAX_DEPTH: usize = 64;
const FAILURE_DETAIL_LIMIT: u64 = 8;

struct Candidate {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
    identity: PhysicalPathIdentity,
}

struct Inventory {
    root: PathBuf,
    directories: HashMap<PathBuf, PhysicalPathIdentity>,
    candidates: Vec<Candidate>,
}

impl Inventory {
    fn bytes(&self) -> u64 {
        self.candidates
            .iter()
            .fold(0_u64, |total, item| total.saturating_add(item.bytes))
    }

    fn validate_parents(&self, path: &Path) -> bool {
        path.ancestors()
            .skip(1)
            .take_while(|parent| parent.starts_with(&self.root))
            .all(|parent| {
                self.directories.get(parent).is_some_and(|identity| {
                    physical_path_identity_snapshot(parent)
                        .is_ok_and(|current| current == *identity)
                })
            })
            && current_platform().validate_path_no_links(path).is_ok()
    }
}

#[derive(Debug)]
struct DiscoveryFailure {
    permission_denied: bool,
    cancelled: bool,
}

fn discovery_failure(
    path: &Path,
    stage: &str,
    error: impl std::fmt::Display,
    denied: bool,
) -> DiscoveryFailure {
    log::warn!(
        "windows_system_temp_scan_failed path={} stage={stage} error={} outcome=blocked",
        diagnostic_path(path),
        mangodisk_platform::diagnostics::text(&error)
    );
    DiscoveryFailure {
        permission_denied: denied,
        cancelled: false,
    }
}

fn io_failure(path: &Path, stage: &str, error: io::Error) -> DiscoveryFailure {
    let denied = error.kind() == io::ErrorKind::PermissionDenied;
    discovery_failure(
        path,
        stage,
        format!("native_code={:?} {error}", error.raw_os_error()),
        denied,
    )
}

fn identity_failure(path: &Path, error: PermanentDeleteError) -> DiscoveryFailure {
    let denied = error.native_code() == Some(5);
    discovery_failure(
        path,
        "identity",
        format!("native_code={:?} {error}", error.native_code()),
        denied,
    )
}

fn discover(
    root: &Path,
    now: SystemTime,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Inventory>, DiscoveryFailure> {
    if cancelled() {
        return Err(DiscoveryFailure {
            permission_denied: false,
            cancelled: true,
        });
    }
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_failure(root, "root_metadata", error)),
    };
    if !metadata.is_dir() || is_link_like(&metadata) {
        return Err(discovery_failure(
            root,
            "validate_root",
            "root is not a physical directory",
            false,
        ));
    }
    current_platform()
        .validate_path_no_links(root)
        .map_err(|error| discovery_failure(root, "validate_root", error, false))?;
    let mut inventory = Inventory {
        root: root.to_path_buf(),
        directories: HashMap::new(),
        candidates: Vec::new(),
    };
    let mut pending = vec![(root.to_path_buf(), 0)];
    while let Some((directory, depth)) = pending.pop() {
        if cancelled() {
            return Err(DiscoveryFailure {
                permission_denied: false,
                cancelled: true,
            });
        }
        current_platform()
            .validate_path_no_links(&directory)
            .map_err(|error| discovery_failure(&directory, "validate_directory", error, false))?;
        let identity = physical_path_identity_snapshot(&directory)
            .map_err(|error| identity_failure(&directory, error))?;
        inventory.directories.insert(directory.clone(), identity);
        let entries = fs::read_dir(&directory)
            .map_err(|error| io_failure(&directory, "read_directory", error))?;
        for entry in entries {
            if cancelled() {
                return Err(DiscoveryFailure {
                    permission_denied: false,
                    cancelled: true,
                });
            }
            let entry = entry.map_err(|error| io_failure(&directory, "read_entry", error))?;
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(io_failure(&path, "entry_metadata", error)),
            };
            if is_link_like(&metadata) {
                continue;
            }
            if metadata.is_dir() {
                if depth >= MAX_DEPTH {
                    return Err(discovery_failure(
                        &path,
                        "depth_limit",
                        "inventory is incomplete",
                        false,
                    ));
                }
                pending.push((path, depth + 1));
            } else if eligible(&metadata, now) {
                let prepared = prepare_path_for_permanent_delete(&path)
                    .map_err(|error| identity_failure(&path, error))?;
                // Use metadata from the pinned object, not the earlier path lookup.
                if eligible(prepared.metadata(), now) {
                    inventory.candidates.push(Candidate {
                        path,
                        bytes: prepared.metadata().len(),
                        modified: prepared
                            .metadata()
                            .modified()
                            .expect("eligible files have a timestamp"),
                        identity: prepared.identity(),
                    });
                }
            }
        }
        if physical_path_identity_snapshot(&directory).ok() != Some(identity) {
            return Err(discovery_failure(
                &directory,
                "verify_directory",
                "directory changed during discovery",
                false,
            ));
        }
    }
    inventory
        .candidates
        .sort_by(|left, right| left.path.cmp(&right.path));
    Ok(Some(inventory))
}

fn eligible(metadata: &fs::Metadata, now: SystemTime) -> bool {
    metadata.is_file()
        && !is_link_like(metadata)
        && !metadata.permissions().readonly()
        && metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > RETENTION)
}

fn failure_status(failure: &DiscoveryFailure, elevated: Option<bool>) -> ScanItemStatus {
    if failure.permission_denied && elevated == Some(false) {
        ScanItemStatus::RequiresElevation
    } else {
        ScanItemStatus::Limited
    }
}

pub(super) fn preview(cancellation: &PlatformCancellation) -> ScanRuleResult {
    let started = Instant::now();
    let root = match windows_system_temporary_directory() {
        Ok(root) => root,
        Err(error) => {
            log::warn!(
                "windows_system_temp_root_unavailable error={}",
                mangodisk_platform::diagnostics::text(&error)
            );
            return limited_rule();
        }
    };
    preview_root(
        &root,
        started,
        &|| cancellation.is_cancelled(),
        windows_process_is_elevated().ok(),
    )
}

fn preview_root(
    root: &Path,
    started: Instant,
    cancelled: &dyn Fn() -> bool,
    elevated: Option<bool>,
) -> ScanRuleResult {
    let (status, bytes, count) = match discover(root, SystemTime::now(), cancelled) {
        Ok(None) => (ScanItemStatus::NotApplicable, 0, 0),
        Err(error) => (failure_status(&error, elevated), 0, 0),
        Ok(Some(inventory)) => {
            let mut accessible = 0;
            let mut denied = 0;
            let mut other = 0;
            for candidate in &inventory.candidates {
                if cancelled() {
                    return limited_rule();
                }
                match probe_windows_file_delete_access(&candidate.path) {
                    Ok(()) => accessible += 1,
                    Err(error) if error.raw_os_error() == Some(5) => denied += 1,
                    Err(_) => other += 1,
                }
            }
            let status = if inventory.candidates.is_empty() {
                ScanItemStatus::Clean
            } else if accessible > 0 {
                ScanItemStatus::Found
            } else if denied > 0 && other == 0 && elevated == Some(false) {
                ScanItemStatus::RequiresElevation
            } else {
                ScanItemStatus::Limited
            };
            (status, inventory.bytes(), inventory.candidates.len() as u64)
        }
    };
    log::info!("windows_system_temp_scanned path={} status={status:?} bytes={bytes} item_count={count} elapsed_ms={}", diagnostic_path(root), started.elapsed().as_millis());
    rule(status, bytes, count, started.elapsed().as_millis() as u64)
}

pub(super) fn limited_rule() -> ScanRuleResult {
    rule(ScanItemStatus::Limited, 0, 0, 0)
}

fn rule(
    status: ScanItemStatus,
    bytes: u64,
    file_count: u64,
    scan_elapsed_ms: u64,
) -> ScanRuleResult {
    ScanRuleResult {
        rule_id: CLEANER_ID.to_string(),
        category: CleanupCategory::System,
        group: CleanupGroup::System,
        risk: RiskLevel::Recoverable,
        default_selected: false,
        recommended_selected: false,
        bytes,
        file_count,
        available: status != ScanItemStatus::NotApplicable,
        selectable: status == ScanItemStatus::Found,
        status,
        running_processes: Vec::new(),
        requires_app_close: false,
        sources: Vec::new(),
        source_count: 0,
        sources_truncated: false,
        scan_elapsed_ms,
    }
}

pub(super) fn execute(dry_run: bool, operation: &OperationGuard) -> CleanupActionResult {
    let root = match windows_system_temporary_directory() {
        Ok(root) => root,
        Err(error) => {
            log::warn!(
                "windows_system_temp_root_unavailable error={}",
                mangodisk_platform::diagnostics::text(&error)
            );
            return failed(CleanupActionReason::PreflightFailed);
        }
    };
    execute_root(&root, dry_run, &|| {
        operation.ensure_not_cancelled().is_err()
    })
}

fn execute_root(root: &Path, dry_run: bool, cancelled: &dyn Fn() -> bool) -> CleanupActionResult {
    let inventory = match discover(root, SystemTime::now(), cancelled) {
        Ok(Some(inventory)) => inventory,
        Ok(None) => return failed(CleanupActionReason::PreflightFailed),
        Err(error) => {
            return failed(if error.cancelled {
                CleanupActionReason::Cancelled
            } else {
                CleanupActionReason::PreflightFailed
            })
        }
    };
    execute_inventory(inventory, dry_run, cancelled)
}

fn execute_inventory(
    inventory: Inventory,
    dry_run: bool,
    cancelled: &dyn Fn() -> bool,
) -> CleanupActionResult {
    let mut result = action();
    result.bytes_expected = inventory.bytes();
    if dry_run {
        result.status = CleanupActionStatus::Previewed;
        return result;
    }
    let mut was_cancelled = false;
    for candidate in &inventory.candidates {
        if cancelled() {
            was_cancelled = true;
            break;
        }
        let deletion = (|| {
            if !inventory.validate_parents(&candidate.path) {
                return Err(PermanentDeleteError::before_mutation(
                    "SystemTemp directory identity changed",
                ));
            }
            // Staging changes the parent directory used by Windows delete-child checks.
            // Respect deletion access at the original path before any rename can occur.
            probe_windows_file_delete_access(&candidate.path).map_err(|error| {
                PermanentDeleteError::before_mutation(format!(
                    "SystemTemp delete access denied or file in use native_code={:?}: {error}",
                    error.raw_os_error()
                ))
            })?;
            let prepared = prepare_path_for_permanent_delete(&candidate.path)?;
            if prepared.identity() != candidate.identity
                || prepared.metadata().len() != candidate.bytes
                || prepared.metadata().modified().ok() != Some(candidate.modified)
                || !eligible(prepared.metadata(), SystemTime::now())
            {
                return Err(PermanentDeleteError::before_mutation(
                    "SystemTemp file changed after preflight",
                ));
            }
            delete_path_permanently(prepared, candidate.bytes, 1)
        })();
        match deletion {
            Ok(()) => {
                result.released_bytes = result.released_bytes.saturating_add(candidate.bytes);
                result.affected_item_count += 1;
            }
            Err(error) => {
                result.released_bytes =
                    result.released_bytes.saturating_add(error.released_bytes());
                result.affected_item_count = result
                    .affected_item_count
                    .saturating_add(error.affected_item_count());
                result.failed_item_count += 1;
                if result.failed_item_count <= FAILURE_DETAIL_LIMIT {
                    log::warn!("windows_system_temp_item_failed path={} stage={} native_code={:?} error={} outcome=skipped",
                        diagnostic_path(&candidate.path), error.stage(), error.native_code(), mangodisk_platform::diagnostics::text(&error));
                }
            }
        }
    }
    // A fresh inventory verifies that the fixed boundary is still readable after mutation.
    // Successful per-file transactions supply counts; concurrent writers must not inflate them.
    let verified = discover(&inventory.root, SystemTime::now(), &|| false).is_ok_and(|after| {
        after.is_some_and(|after| {
            after.directories.get(&after.root) == inventory.directories.get(&inventory.root)
        })
    });
    if !verified {
        result.failed_item_count += 1;
        result.reason_code = Some(CleanupActionReason::VerificationFailed);
    } else if was_cancelled {
        result.reason_code = Some(CleanupActionReason::Cancelled);
    } else if result.failed_item_count > 0 {
        result.reason_code = Some(CleanupActionReason::ItemsSkipped);
    }
    result.status = if result.failed_item_count == 0 && !was_cancelled {
        CleanupActionStatus::Completed
    } else if result.affected_item_count > 0 {
        CleanupActionStatus::Partial
    } else if was_cancelled {
        CleanupActionStatus::Blocked
    } else {
        CleanupActionStatus::Failed
    };
    log::info!("windows_system_temp_cleanup_finished status={:?} expected_bytes={} released_bytes={} affected_item_count={} failed_item_count={} suppressed_failure_count={} verified={verified}",
        result.status, result.bytes_expected, result.released_bytes, result.affected_item_count, result.failed_item_count,
        result.failed_item_count.saturating_sub(FAILURE_DETAIL_LIMIT));
    result
}

fn action() -> CleanupActionResult {
    CleanupActionResult {
        rule_id: CLEANER_ID.to_string(),
        action_kind: CleanupActionKind::Delete,
        status: CleanupActionStatus::Completed,
        reason_code: None,
        bytes_expected: 0,
        released_bytes: 0,
        affected_item_count: 0,
        failed_item_count: 0,
        running_processes: Vec::new(),
    }
}

fn failed(reason: CleanupActionReason) -> CleanupActionResult {
    let mut result = action();
    result.status = if reason == CleanupActionReason::Cancelled {
        CleanupActionStatus::Blocked
    } else {
        CleanupActionStatus::Failed
    };
    result.reason_code = Some(reason);
    result.failed_item_count = 1;
    result
}

#[cfg(test)]
#[path = "windows_system_temp_tests.rs"]
mod tests;
