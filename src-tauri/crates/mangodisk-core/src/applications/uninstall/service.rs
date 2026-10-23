use super::batch::{aggregate_batch_result, validate_batch_selections};
use super::execution_history::{save_batch_execution_history, save_execution_history};
use crate::system_inventory::SystemInventoryContext;

use std::collections::{HashMap, HashSet};
#[cfg(target_os = "macos")]
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[cfg(windows)]
use mangodisk_platform::ApplicationInstallScope;
use mangodisk_platform::{
    current_platform, ApplicationInventorySource, ApplicationUninstallRegistration,
    InstalledApplication, Platform, PlatformCancellation,
};
#[cfg(target_os = "macos")]
use mangodisk_platform::{
    macos_privileged_application_removal_supported, remove_application_bundle_with_privileges,
    MacosPrivilegedApplicationRemovalOutcome,
};

#[cfg(target_os = "macos")]
use crate::filesystem::permanent_delete::{
    delete_path_permanently, prepare_path_for_permanent_delete,
};
#[cfg(any(target_os = "macos", windows))]
use crate::shared::operation::OPERATION_CANCELLED_ERROR;
use crate::{
    applications::{
        catalog::ProcessSnapshot,
        process_control::{close_resolved_applications, ResolvedApplicationCloseTarget},
    },
    filesystem::metadata::display_path,
    shared::progress::{ProgressSink, ProgressTracker, TraversalStage},
    shared::{
        operation::{CoordinatedOperationKind, OperationCancellationToken, OperationGuard},
        CoreError, CoreResult,
    },
};

#[cfg(target_os = "macos")]
use super::execution;
#[cfg(target_os = "macos")]
use super::macos;
#[cfg(windows)]
use super::models::ApplicationUninstallActionResult;
use super::models::{
    ApplicationUninstallActionReason, ApplicationUninstallActionStatus,
    ApplicationUninstallBatchPlan, ApplicationUninstallBatchPreparation,
    ApplicationUninstallBatchResult, ApplicationUninstallBatchSelection,
    ApplicationUninstallCandidate, ApplicationUninstallCapability,
    ApplicationUninstallCloseRequest, ApplicationUninstallComponentKind,
    ApplicationUninstallComponentSummary, ApplicationUninstallExecutionItemResult,
    ApplicationUninstallExecutionItemStatus, ApplicationUninstallExecutionMode,
    ApplicationUninstallExecutionProgress, ApplicationUninstallExecutionStage,
    ApplicationUninstallInspection, ApplicationUninstallInstallerKind,
    ApplicationUninstallInventorySource, ApplicationUninstallPlan, ApplicationUninstallPlanItem,
    ApplicationUninstallPlatform, ApplicationUninstallRecordState, ApplicationUninstallResult,
    ApplicationUninstallScanResult, ApplicationUninstallSourceIdentity,
    APPLICATION_UNINSTALL_SCAN_SCHEMA_VERSION,
};
#[cfg(windows)]
use super::windows;
use super::{batch, plan, preflight};

#[cfg(windows)]
const CURRENT_APPLICATION_NAME: &str = "MangoDisk";

struct PreflightCandidate {
    result: ApplicationUninstallResult,
    #[cfg(any(target_os = "macos", windows))]
    inspection: Option<ApplicationUninstallInspection>,
    #[cfg(any(target_os = "macos", windows))]
    process_target: Option<ResolvedApplicationCloseTarget>,
}

struct UninstallExecutionReporter<F> {
    handler: F,
    started: Instant,
    completed_applications: Vec<ApplicationUninstallExecutionItemResult>,
    completed_application_count: u64,
    total_application_count: u64,
    affected_application_count: u64,
    failed_application_count: u64,
    released_bytes: u64,
}

impl<F> UninstallExecutionReporter<F>
where
    F: FnMut(ApplicationUninstallExecutionProgress),
{
    fn new(total_application_count: usize, handler: F) -> Self {
        Self {
            handler,
            started: Instant::now(),
            completed_applications: Vec::with_capacity(total_application_count),
            completed_application_count: 0,
            total_application_count: total_application_count as u64,
            affected_application_count: 0,
            failed_application_count: 0,
            released_bytes: 0,
        }
    }

    fn emit(
        &mut self,
        stage: ApplicationUninstallExecutionStage,
        current_application_id: Option<&str>,
    ) {
        (self.handler)(ApplicationUninstallExecutionProgress {
            stage,
            current_application_id: current_application_id.map(str::to_owned),
            completed_applications: self.completed_applications.clone(),
            completed_application_count: self.completed_application_count,
            total_application_count: self.total_application_count,
            affected_application_count: self.affected_application_count,
            failed_application_count: self.failed_application_count,
            released_bytes: self.released_bytes,
            elapsed_ms: self.started.elapsed().as_millis() as u64,
        });
    }

    fn record(&mut self, result: &ApplicationUninstallResult) {
        self.completed_applications
            .push(ApplicationUninstallExecutionItemResult {
                application_id: result.application_id.clone(),
                status: if result
                    .actions
                    .iter()
                    .any(|action| action.status == ApplicationUninstallActionStatus::Cancelled)
                {
                    ApplicationUninstallExecutionItemStatus::Cancelled
                } else if result.failed_item_count > 0 {
                    ApplicationUninstallExecutionItemStatus::Failed
                } else {
                    ApplicationUninstallExecutionItemStatus::Completed
                },
                released_bytes: result.released_bytes,
            });
        self.completed_application_count = self
            .completed_application_count
            .saturating_add(1)
            .min(self.total_application_count);
        self.affected_application_count = self
            .affected_application_count
            .saturating_add(u64::from(result.affected_item_count > 0));
        self.failed_application_count = self
            .failed_application_count
            .saturating_add(u64::from(result.failed_item_count > 0));
        self.released_bytes = self.released_bytes.saturating_add(result.released_bytes);
    }
}

pub struct ApplicationUninstallService;

impl ApplicationUninstallService {
    pub fn cancel_scan() {
        OperationCancellationToken::application_scan().cancel();
    }

    /// Requests cooperative cancellation of the active uninstall batch.
    ///
    /// A native Windows uninstaller may own a separate interactive process and
    /// cannot be terminated safely. Core stops waiting without terminating that
    /// process, records it as continuing externally, and prevents every later
    /// application in the batch from starting.
    pub fn cancel_execution() {
        OperationCancellationToken::applications().cancel();
    }

    /// Closes applications selected from a trusted uninstall catalog snapshot.
    ///
    /// Stable application IDs cross the adapter boundary. Core resolves the
    /// corresponding process names and application path from its catalog so
    /// callers cannot provide arbitrary process identities.
    pub fn close_applications_from_catalog(
        request: ApplicationUninstallCloseRequest,
        scan: &mut ApplicationUninstallScanResult,
    ) -> CoreResult<crate::ApplicationCloseBatchResult> {
        let operation = OperationGuard::start(CoordinatedOperationKind::ApplicationClose)?;
        if !scan.catalog_actionable {
            return Err(CoreError::operation_failed(
                "application uninstall catalog is not actionable",
            ));
        }
        let selected = request
            .application_ids
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        if selected.is_empty() || selected.len() != request.application_ids.len() {
            return Err(CoreError::invalid_input(
                "the application close selection is invalid",
            ));
        }

        let mut targets = Vec::with_capacity(request.application_ids.len());
        for application_id in &request.application_ids {
            let candidate = scan
                .candidates
                .iter()
                .find(|candidate| candidate.application_id == *application_id)
                .ok_or_else(|| {
                    CoreError::invalid_input(
                        "the application close request contains an unknown application",
                    )
                })?;
            if candidate.running_processes.is_empty() {
                return Err(CoreError::invalid_input(
                    "the application close target is not running in the reviewed catalog",
                ));
            }
            targets.push(close_target(candidate));
        }
        let result = close_resolved_applications(targets, request.mode)?;
        refresh_catalog_after_close(scan, &result);
        operation.complete();
        Ok(result)
    }

    /// Explicitly removes the selected installation record, independently of uninstallability.
    /// Native code resolves the ID under fixed registry roots and revalidates that exact snapshot.
    /// A catalog-wide revision, running processes, or missing files cannot veto this separate
    /// user action. No application paths or filesystem cleanup plans participate in it.
    pub fn remove_application_record(application_id: &str, dry_run: bool) -> CoreResult<()> {
        let operation = OperationGuard::start(CoordinatedOperationKind::Applications)?;
        // Reject arbitrary transport input before it can enter diagnostic logs.
        if !application_id
            .strip_prefix("application-")
            .is_some_and(|suffix| {
                suffix.len() == 24 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        {
            return Err(CoreError::invalid_input("invalid application record ID"));
        }
        log::info!("application_record_removal_started operation_id={} application_id={} dry_run={dry_run}", operation.id(), application_id);
        let result = current_platform().remove_application_record(application_id, dry_run);
        if let Err(error) = &result {
            if error.code() == mangodisk_platform::PlatformErrorCode::UserCancelled {
                log::info!("application_record_removal_cancelled operation_id={} application_id={application_id}", operation.id());
            } else {
                log::warn!("application_record_removal_failed operation_id={} application_id={} code={:?} mutation_state={:?} error={}", operation.id(), application_id, error.code(), error.mutation_state(), mangodisk_platform::diagnostics::text(&error));
            }
        }
        result?;
        log::info!("application_record_removal_finished operation_id={} application_id={} dry_run={dry_run} verified=true files_deleted=0", operation.id(), application_id);
        operation.complete();
        Ok(())
    }

    pub fn scan() -> CoreResult<ApplicationUninstallScanResult> {
        let operation = OperationGuard::start(CoordinatedOperationKind::ApplicationScan)?;
        let result = scan_without_guard(operation.id(), true, None, operation.cancellation_flag())?;
        operation.complete();
        Ok(result)
    }

    pub fn scan_with_progress(
        callback: impl ProgressSink,
    ) -> CoreResult<ApplicationUninstallScanResult> {
        let operation = OperationGuard::start(CoordinatedOperationKind::ApplicationScan)?;
        let result = scan_without_guard(
            operation.id(),
            true,
            Some(Box::new(callback)),
            operation.cancellation_flag(),
        )?;
        operation.complete();
        Ok(result)
    }

    pub fn inspect(application_id: &str) -> CoreResult<ApplicationUninstallInspection> {
        if application_id.trim().is_empty() {
            return Err(CoreError::invalid_input(
                "application uninstall identifier is empty",
            ));
        }
        let operation = OperationGuard::start(CoordinatedOperationKind::ApplicationPreparation)?;
        let started = Instant::now();
        let scan = scan_without_guard(operation.id(), false, None, operation.cancellation_flag())?;
        if !scan.catalog_actionable {
            return Err(CoreError::operation_failed(
                "application uninstall catalog is not actionable",
            ));
        }
        let candidate = scan
            .candidates
            .iter()
            .find(|candidate| candidate.application_id == application_id)
            .ok_or_else(|| "application uninstall candidate is unavailable".to_string())?;
        let catalog_revision = scan
            .catalog_revision
            .as_deref()
            .ok_or_else(|| "application uninstall catalog revision is unavailable".to_string())?;

        let inspection = inspect_candidate(candidate, catalog_revision, started)?;

        log::info!(
            "application_uninstall_inspection_ready operation_id={} application_id={} application_name={} component_count={} total_bytes={} default_selected_bytes={} elapsed_ms={}",
            operation.id(),
            inspection.application_id,
            mangodisk_platform::diagnostics::text(&inspection.application_name),
            inspection.components.len(),
            inspection.total_bytes,
            inspection.default_selected_bytes,
            inspection.elapsed_ms
        );
        operation.complete();
        Ok(inspection)
    }

    pub fn create_plan(
        inspection: &ApplicationUninstallInspection,
        component_ids: &[String],
    ) -> CoreResult<ApplicationUninstallPlan> {
        Ok(plan::create_plan(inspection, component_ids)?)
    }

    pub fn create_plan_for_application(
        application_id: &str,
        component_ids: &[String],
    ) -> CoreResult<ApplicationUninstallPlan> {
        // Re-inspect in Core so GUI and other adapters submit only stable
        // component IDs. Paths, sizes, and snapshots are always sourced from
        // the current operating-system inventory at the trust boundary.
        let inspection = Self::inspect(application_id)?;
        Self::create_plan(&inspection, component_ids)
    }

    pub fn create_reviewed_plan(
        application_id: String,
        catalog_revision: String,
        items: Vec<ApplicationUninstallPlanItem>,
    ) -> Result<ApplicationUninstallPlan, String> {
        plan::create_reviewed_plan(application_id, catalog_revision, items)
    }

    pub fn validate_plan(plan: &ApplicationUninstallPlan) -> Result<(), String> {
        plan::validate_plan(plan)
    }

    pub fn create_batch_plan(
        selections: &[ApplicationUninstallBatchSelection],
    ) -> CoreResult<ApplicationUninstallBatchPlan> {
        validate_batch_selections(selections)?;
        let operation = OperationGuard::start(CoordinatedOperationKind::ApplicationPreparation)?;
        let started = Instant::now();
        let scan = scan_without_guard(operation.id(), false, None, operation.cancellation_flag())?;
        let (batch_plan, _) = create_batch_plan_from_scan(selections, &scan)?;
        log::info!(
            "application_uninstall_batch_plan_ready operation_id={} application_count={} component_count={} expected_bytes={} elapsed_ms={}",
            operation.id(),
            batch_plan.plans.len(),
            batch_plan.plans.iter().map(|plan| plan.items.len()).sum::<usize>(),
            batch_plan.expected_bytes,
            started.elapsed().as_millis()
        );
        operation.complete();
        Ok(batch_plan)
    }

    pub fn prepare_batch(
        selections: &[ApplicationUninstallBatchSelection],
    ) -> CoreResult<ApplicationUninstallBatchPreparation> {
        validate_batch_selections(selections)?;
        let operation = OperationGuard::start(CoordinatedOperationKind::ApplicationPreparation)?;
        let started = Instant::now();
        let scan = scan_without_guard(operation.id(), false, None, operation.cancellation_flag())?;
        let preparation = prepare_batch_from_scan(operation.id(), selections, &scan, started)?;
        operation.complete();
        Ok(preparation)
    }

    pub fn prepare_batch_from_catalog(
        selections: &[ApplicationUninstallBatchSelection],
        scan: &ApplicationUninstallScanResult,
    ) -> CoreResult<ApplicationUninstallBatchPreparation> {
        validate_batch_selections(selections)?;
        let operation = OperationGuard::start(CoordinatedOperationKind::ApplicationPreparation)?;
        let started = Instant::now();
        let preparation = prepare_batch_from_scan(operation.id(), selections, scan, started)?;
        operation.complete();
        Ok(preparation)
    }

    pub fn validate_batch_plan(plan: &ApplicationUninstallBatchPlan) -> Result<(), String> {
        batch::validate_plan(plan)
    }

    pub fn execute_batch(
        batch_plan: ApplicationUninstallBatchPlan,
        dry_run: bool,
    ) -> CoreResult<ApplicationUninstallBatchResult> {
        Self::execute_batch_with_progress(batch_plan, dry_run, None, |_| {})
    }

    /// Executes a prepared batch and forwards an ephemeral native authorization prompt.
    ///
    /// The prompt is UI context only: it is never persisted or logged, and platforms
    /// that do not display MangoDisk-managed authorization UI ignore it.
    pub fn execute_batch_with_progress<F>(
        batch_plan: ApplicationUninstallBatchPlan,
        dry_run: bool,
        authorization_prompt: Option<&str>,
        progress: F,
    ) -> CoreResult<ApplicationUninstallBatchResult>
    where
        F: FnMut(ApplicationUninstallExecutionProgress),
    {
        Self::validate_batch_plan(&batch_plan)?;
        let operation = OperationGuard::start(CoordinatedOperationKind::Applications)?;
        let started_at_ms = crate::shared::clock::unix_time_ms();
        let started = Instant::now();
        let mut progress = UninstallExecutionReporter::new(batch_plan.plans.len(), progress);
        progress.emit(ApplicationUninstallExecutionStage::Validating, None);
        let scan = scan_without_guard(operation.id(), false, None, operation.cancellation_flag())?;
        if !scan.catalog_actionable {
            return Err(CoreError::operation_failed(
                "application uninstall catalog is not actionable",
            ));
        }

        // The global inventory revision can change when an unrelated app is
        // installed or removed. Revalidate each selected registration and its
        // component fingerprint instead of rejecting every plan for unrelated
        // catalog churn. This preserves the mutation-boundary safety check while
        // allowing users to uninstall another app without a manual rescan.
        let preflighted = batch_plan
            .plans
            .iter()
            .map(|plan| preview_candidate(plan, &scan, started))
            .collect::<Result<Vec<_>, _>>()?;

        let mut results = if dry_run {
            preflighted
                .into_iter()
                .map(|candidate| {
                    progress.record(&candidate.result);
                    progress.emit(
                        ApplicationUninstallExecutionStage::Validating,
                        Some(&candidate.result.application_id),
                    );
                    candidate.result
                })
                .collect::<Vec<_>>()
        } else {
            // Native uninstallers can block for minutes and may open their own
            // UI. Execute one prepared application at a time so the current
            // target and completed count always describe real batch state.
            let mut results = Vec::with_capacity(batch_plan.plans.len());
            for (plan, candidate) in batch_plan.plans.iter().zip(preflighted) {
                progress.emit(
                    ApplicationUninstallExecutionStage::Uninstalling,
                    Some(&plan.application_id),
                );
                // Cancellation is checked only at an application boundary. A
                // currently open native uninstaller remains in control of its
                // own process, while no later application is allowed to start.
                let result = if operation.ensure_not_cancelled().is_err() {
                    preflight::cancel_all(plan, candidate.result.application_name, None)
                } else {
                    execute_preflighted(
                        plan,
                        candidate,
                        operation.cancellation_flag(),
                        authorization_prompt,
                    )
                };
                log::info!(
                    "application_uninstall_item_finished operation_id={} application_id={} application_name={:?} plan_id={} dry_run={} affected_count={} failed_count={} reason={}",
                    operation.id(), result.application_id, result.application_name, result.plan_id, result.dry_run,
                    result.affected_item_count, result.failed_item_count,
                    result.actions.iter().find_map(|action| action.reason).map_or("none", |reason| reason.stable_code())
                );
                progress.record(&result);
                progress.emit(
                    ApplicationUninstallExecutionStage::Uninstalling,
                    Some(&plan.application_id),
                );
                results.push(result);
            }
            results
        };
        let history_saved = !dry_run
            && save_batch_execution_history(
                operation.id(),
                started_at_ms,
                crate::shared::clock::unix_time_ms(),
                &batch_plan,
                &results,
                &scan.candidates,
            );
        for result in &mut results {
            result.history_saved = history_saved;
        }
        let result = aggregate_batch_result(&batch_plan, dry_run, results);
        progress.emit(ApplicationUninstallExecutionStage::Finalizing, None);
        let cancelled_application_count = result
            .results
            .iter()
            .filter(|application| {
                application
                    .actions
                    .iter()
                    .any(|action| action.status == ApplicationUninstallActionStatus::Cancelled)
            })
            .count();
        log::info!(
            "application_uninstall_batch_finished operation_id={} dry_run={} application_count={} affected_application_count={} failed_application_count={} cancelled_application_count={} component_count={} affected_count={} failed_count={} expected_bytes={} released_bytes={} history_saved={} elapsed_ms={}",
            operation.id(),
            dry_run,
            result.selected_application_count,
            result.affected_application_count,
            result.failed_application_count,
            cancelled_application_count,
            batch_plan.plans.iter().map(|plan| plan.items.len()).sum::<usize>(),
            result.affected_item_count,
            result.failed_item_count,
            result.expected_bytes,
            result.released_bytes,
            history_saved,
            started.elapsed().as_millis()
        );
        operation.complete();
        Ok(result)
    }

    pub fn execute(
        plan: ApplicationUninstallPlan,
        dry_run: bool,
    ) -> CoreResult<ApplicationUninstallResult> {
        Self::validate_plan(&plan)?;
        let operation = OperationGuard::start(CoordinatedOperationKind::Applications)?;
        let started_at_ms = crate::shared::clock::unix_time_ms();
        let started = Instant::now();
        let scan = scan_without_guard(operation.id(), false, None, operation.cancellation_flag())?;
        if !scan.catalog_actionable {
            return Err(CoreError::operation_failed(
                "application uninstall catalog is not actionable",
            ));
        }

        let preflight = preview_candidate(&plan, &scan, started)?;
        if dry_run {
            log_preflight(operation.id(), &plan, &preflight.result, started);
            operation.complete();
            return Ok(preflight.result);
        }

        let mut result = execute_preflighted(&plan, preflight, operation.cancellation_flag(), None);
        let application = scan
            .candidates
            .iter()
            .find(|application| application.application_id == plan.application_id);
        result.history_saved = save_execution_history(
            operation.id(),
            started_at_ms,
            crate::shared::clock::unix_time_ms(),
            &plan,
            &result,
            application,
        );
        log::info!(
            "application_uninstall_execution_finished operation_id={} component_count={} affected_count={} failed_count={} expected_bytes={} released_bytes={} history_saved={} elapsed_ms={}",
            operation.id(),
            plan.items.len(),
            result.affected_item_count,
            result.failed_item_count,
            result.expected_bytes,
            result.released_bytes,
            result.history_saved,
            started.elapsed().as_millis()
        );
        operation.complete();
        Ok(result)
    }
}

fn close_target(candidate: &ApplicationUninstallCandidate) -> ResolvedApplicationCloseTarget {
    let mut executable_names = candidate.running_processes.clone();
    executable_names.push(candidate.name.clone());
    executable_names.push(candidate.primary_identifier.clone());
    executable_names.sort_by_key(|name| name.to_ascii_lowercase());
    executable_names.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    ResolvedApplicationCloseTarget {
        target_id: candidate.application_id.clone(),
        // Exact platform inventory paths are the primary authority. Process
        // names are retained only for catalog sources without an executable
        // path; OR-matching both could close another installation's helper.
        executable_names: if candidate.executable_paths.is_empty() {
            executable_names
        } else {
            Vec::new()
        },
        executable_paths: candidate.executable_paths.clone(),
    }
}

/// Applies a trusted process-close result to the catalog snapshot used for
/// uninstall planning. Closing a process does not change the platform
/// inventory revision, so rebuilding application components and related-path
/// summaries would add substantial latency without improving safety. Targets
/// that still have a matching process remain blocked; only targets confirmed
/// as fully stopped receive their underlying platform capability.
fn refresh_catalog_after_close(
    scan: &mut ApplicationUninstallScanResult,
    result: &crate::ApplicationCloseBatchResult,
) {
    let stopped_ids = result
        .targets
        .iter()
        .filter(|target| {
            target.status == crate::ApplicationCloseTargetStatus::Completed
                && target.remaining_processes.is_empty()
        })
        .map(|target| target.target_id.as_str())
        .collect::<HashSet<_>>();

    for candidate in &mut scan.candidates {
        if stopped_ids.contains(candidate.application_id.as_str())
            && candidate.capability == ApplicationUninstallCapability::ApplicationRunning
        {
            candidate.running_processes.clear();
            candidate.capability = capability_after_close(candidate);
        }
    }
    scan.ready_count = scan
        .candidates
        .iter()
        .filter(|candidate| candidate.capability.supports_execution())
        .count() as u64;
    scan.blocked_count = scan.candidates.len() as u64 - scan.ready_count;
}

#[cfg(target_os = "macos")]
fn capability_after_close(
    candidate: &ApplicationUninstallCandidate,
) -> ApplicationUninstallCapability {
    let Some(path) = candidate.application_path.as_deref().map(Path::new) else {
        // A running candidate originally passed the bundle safety checks. If
        // its path is unexpectedly unavailable now, fail closed instead of
        // making permanent deletion actionable from incomplete evidence.
        return ApplicationUninstallCapability::ViewOnly;
    };
    if !macos_bundle_is_deletable_without_elevation(path) {
        return if macos_privileged_application_removal_supported(path) {
            ApplicationUninstallCapability::RequiresElevation
        } else {
            ApplicationUninstallCapability::ViewOnly
        };
    }
    ApplicationUninstallCapability::Ready
}

#[cfg(windows)]
fn capability_after_close(
    candidate: &ApplicationUninstallCandidate,
) -> ApplicationUninstallCapability {
    let Some(registration) = candidate.uninstall_registration.as_ref() else {
        return ApplicationUninstallCapability::ViewOnly;
    };
    if matches!(
        registration,
        ApplicationUninstallRegistration::WindowsMsi {
            scope: ApplicationInstallScope::Machine,
            ..
        } | ApplicationUninstallRegistration::WindowsChocolatey { .. }
    ) {
        return ApplicationUninstallCapability::RequiresElevation;
    }
    ApplicationUninstallCapability::Ready
}

#[cfg(not(any(target_os = "macos", windows)))]
fn capability_after_close(
    _candidate: &ApplicationUninstallCandidate,
) -> ApplicationUninstallCapability {
    ApplicationUninstallCapability::ViewOnly
}

fn prepare_batch_from_scan(
    operation_id: u64,
    selections: &[ApplicationUninstallBatchSelection],
    scan: &ApplicationUninstallScanResult,
    started: Instant,
) -> Result<ApplicationUninstallBatchPreparation, String> {
    let (plan, inspections) = create_batch_plan_from_scan(selections, scan)?;
    let results = plan
        .plans
        .iter()
        .zip(inspections)
        .map(|(application_plan, inspection)| preflight::compare(application_plan, &inspection))
        .collect();
    let preview = aggregate_batch_result(&plan, true, results);
    log::info!(
        "application_uninstall_batch_prepared operation_id={} application_count={} component_count={} expected_bytes={} failed_count={} elapsed_ms={}",
        operation_id,
        plan.plans.len(),
        plan.plans.iter().map(|plan| plan.items.len()).sum::<usize>(),
        plan.expected_bytes,
        preview.failed_item_count,
        started.elapsed().as_millis()
    );
    Ok(ApplicationUninstallBatchPreparation { plan, preview })
}

fn create_batch_plan_from_scan(
    selections: &[ApplicationUninstallBatchSelection],
    scan: &ApplicationUninstallScanResult,
) -> Result<
    (
        ApplicationUninstallBatchPlan,
        Vec<ApplicationUninstallInspection>,
    ),
    String,
> {
    if !scan.catalog_actionable {
        return Err("application uninstall catalog is not actionable".to_string());
    }
    let catalog_revision = scan
        .catalog_revision
        .as_deref()
        .ok_or_else(|| "application uninstall catalog revision is unavailable".to_string())?;
    let mut inspections = HashMap::with_capacity(selections.len());
    let mut application_plans = Vec::with_capacity(selections.len());
    for selection in selections {
        let candidate = scan
            .candidates
            .iter()
            .find(|candidate| candidate.application_id == selection.application_id)
            .ok_or_else(|| "application uninstall candidate is unavailable".to_string())?;
        if !candidate.capability.supports_execution() {
            return Err("application is not ready for uninstall planning".to_string());
        }
        let inspection = inspect_candidate(candidate, catalog_revision, Instant::now())?;
        let application_plan = plan::create_plan(&inspection, &selection.component_ids)?;
        inspections.insert(application_plan.application_id.clone(), inspection);
        application_plans.push(application_plan);
    }
    let batch_plan = batch::create_plan(catalog_revision.to_string(), application_plans)?;
    let sorted_inspections = batch_plan
        .plans
        .iter()
        .map(|application_plan| {
            inspections
                .remove(&application_plan.application_id)
                .ok_or_else(|| "application uninstall inspection is unavailable".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((batch_plan, sorted_inspections))
}

fn preview_candidate(
    plan: &ApplicationUninstallPlan,
    scan: &ApplicationUninstallScanResult,
    started: Instant,
) -> Result<PreflightCandidate, String> {
    let Some(candidate) = scan
        .candidates
        .iter()
        .find(|candidate| candidate.application_id == plan.application_id)
    else {
        return Ok(PreflightCandidate {
            result: preflight::fail_all(
                plan,
                None,
                ApplicationUninstallActionReason::ApplicationUnavailable,
            ),
            #[cfg(any(target_os = "macos", windows))]
            inspection: None,
            #[cfg(any(target_os = "macos", windows))]
            process_target: None,
        });
    };
    #[cfg(any(target_os = "macos", windows))]
    let process_target = Some(close_target(candidate));
    if !candidate.capability.supports_execution() {
        let reason = if candidate.capability == ApplicationUninstallCapability::ApplicationRunning {
            ApplicationUninstallActionReason::ApplicationRunning
        } else {
            ApplicationUninstallActionReason::UnsupportedExecutor
        };
        return Ok(PreflightCandidate {
            result: preflight::fail_all(plan, Some(candidate.name.clone()), reason),
            #[cfg(any(target_os = "macos", windows))]
            inspection: None,
            #[cfg(any(target_os = "macos", windows))]
            process_target,
        });
    }
    let catalog_revision = scan
        .catalog_revision
        .as_deref()
        .ok_or_else(|| "application uninstall catalog revision is unavailable".to_string())?;
    let inspection = match inspect_candidate(candidate, catalog_revision, started) {
        Ok(inspection) => inspection,
        Err(error) => {
            log::warn!(
                "application_uninstall_preflight_inspection_failed error={}",
                mangodisk_platform::diagnostics::text(&error)
            );
            return Ok(PreflightCandidate {
                result: preflight::fail_all(
                    plan,
                    Some(candidate.name.clone()),
                    ApplicationUninstallActionReason::ComponentUnavailable,
                ),
                #[cfg(any(target_os = "macos", windows))]
                inspection: None,
                #[cfg(any(target_os = "macos", windows))]
                process_target,
            });
        }
    };
    let result = preflight::compare(plan, &inspection);
    Ok(PreflightCandidate {
        result,
        #[cfg(any(target_os = "macos", windows))]
        inspection: Some(inspection),
        #[cfg(any(target_os = "macos", windows))]
        process_target,
    })
}

fn log_preflight(
    operation_id: u64,
    plan: &ApplicationUninstallPlan,
    result: &ApplicationUninstallResult,
    started: Instant,
) {
    log::info!(
        "application_uninstall_preflight_finished operation_id={} component_count={} previewed_count={} failed_count={} expected_bytes={} previewed_bytes={} elapsed_ms={}",
        operation_id,
        plan.items.len(),
        result.previewed_item_count,
        result.failed_item_count,
        result.expected_bytes,
        result.previewed_bytes,
        started.elapsed().as_millis()
    );
}

#[cfg(target_os = "macos")]
fn execute_preflighted(
    plan: &ApplicationUninstallPlan,
    preflighted: PreflightCandidate,
    _cancellation: Arc<AtomicBool>,
    authorization_prompt: Option<&str>,
) -> ApplicationUninstallResult {
    if preflighted.result.failed_item_count > 0 {
        let reason = preflighted
            .result
            .actions
            .iter()
            .find_map(|action| action.reason)
            .unwrap_or(ApplicationUninstallActionReason::ComponentChanged);
        let mut result = preflight::fail_all(plan, preflighted.result.application_name, reason);
        result.dry_run = false;
        return result;
    }
    let Some(inspection) = preflighted.inspection else {
        let mut result = preflight::fail_all(
            plan,
            preflighted.result.application_name,
            ApplicationUninstallActionReason::ComponentUnavailable,
        );
        result.dry_run = false;
        return result;
    };
    match application_target_is_running(preflighted.process_target.as_ref()) {
        Ok(false) => {}
        Ok(true) => {
            let mut result = preflight::fail_all(
                plan,
                Some(inspection.application_name),
                ApplicationUninstallActionReason::ApplicationRunning,
            );
            result.dry_run = false;
            return result;
        }
        Err(error) => {
            log::warn!(
                "application_uninstall_process_recheck_failed error={}",
                mangodisk_platform::diagnostics::text(&error)
            );
            let mut result = preflight::fail_all(
                plan,
                Some(inspection.application_name),
                ApplicationUninstallActionReason::ProcessStateUnavailable,
            );
            result.dry_run = false;
            return result;
        }
    }

    execution::execute_with(
        plan,
        &inspection,
        macos::component_matches,
        |component| {
            let path = component
                .path
                .as_deref()
                .ok_or(ApplicationUninstallActionReason::ComponentUnavailable)?;
            if !macos::component_matches(component) {
                return Err(execution::DeleteFailure::new(
                    ApplicationUninstallActionReason::ComponentChanged,
                    0,
                ));
            }
            if inspection.capability == ApplicationUninstallCapability::RequiresElevation
                && component.kind == ApplicationUninstallComponentKind::ApplicationBinary
            {
                log::info!(
                    "application_uninstall_privileged_removal_requested component_id={}",
                    component.component_id
                );
                return match remove_application_bundle_with_privileges(
                    Path::new(path),
                    authorization_prompt,
                ) {
                    Ok(MacosPrivilegedApplicationRemovalOutcome::Completed) => Ok(()),
                    Ok(MacosPrivilegedApplicationRemovalOutcome::UserCancelled) => {
                        log::info!(
                            "application_uninstall_privileged_removal_cancelled component_id={}",
                            component.component_id
                        );
                        Err(execution::DeleteFailure::cancelled())
                    }
                    Ok(MacosPrivilegedApplicationRemovalOutcome::ItemChanged) => {
                        log::warn!(
                            "application_uninstall_privileged_removal_stopped component_id={} reason=item_changed",
                            component.component_id
                        );
                        Err(execution::DeleteFailure::new(
                            ApplicationUninstallActionReason::ComponentChanged,
                            0,
                        ))
                    }
                    Ok(MacosPrivilegedApplicationRemovalOutcome::RecoveryRequired) => {
                        log::error!(
                            "application_uninstall_privileged_removal_failed component_id={} reason=recovery_required",
                            component.component_id
                        );
                        Err(execution::DeleteFailure::new(
                            ApplicationUninstallActionReason::RecoveryRequired,
                            0,
                        ))
                    }
                    Err(error) => {
                        log::warn!(
                            "application_uninstall_privileged_removal_failed component_id={} error_code={:?} error={}",
                            component.component_id,
                            error.code(),
                            mangodisk_platform::diagnostics::text(&error)
                        );
                        Err(execution::DeleteFailure::new(
                            ApplicationUninstallActionReason::PermanentDeleteFailed,
                            0,
                        ))
                    }
                };
            }
            let prepared = prepare_path_for_permanent_delete(Path::new(path)).map_err(|_| {
                execution::DeleteFailure::new(ApplicationUninstallActionReason::ComponentChanged, 0)
            })?;
            // Bind the just-in-time component snapshot to the prepared
            // physical identity. A replacement after this check is rejected
            // by the staging boundary before any bytes are removed.
            if !macos::component_matches(component) {
                return Err(execution::DeleteFailure::new(
                    ApplicationUninstallActionReason::ComponentChanged,
                    0,
                ));
            }
            delete_path_permanently(prepared, component.bytes, component.file_count).map_err(|error| {
                log::warn!(
                    "application_uninstall_permanent_delete_failed component_id={} partial={} released_bytes={} error={}",
                    component.component_id,
                    error.is_partial(),
                    error.released_bytes(),
                    mangodisk_platform::diagnostics::text(&error)
                );
                execution::DeleteFailure::new(
                    ApplicationUninstallActionReason::PermanentDeleteFailed,
                    error.released_bytes(),
                )
            })
        },
        macos::component_is_absent,
    )
}

#[cfg(windows)]
fn execute_preflighted(
    plan: &ApplicationUninstallPlan,
    preflighted: PreflightCandidate,
    cancellation: Arc<AtomicBool>,
    _authorization_prompt: Option<&str>,
) -> ApplicationUninstallResult {
    if preflighted.result.failed_item_count > 0 {
        let reason = preflighted
            .result
            .actions
            .iter()
            .find_map(|action| action.reason)
            .unwrap_or(ApplicationUninstallActionReason::ComponentChanged);
        let mut result = preflight::fail_all(plan, preflighted.result.application_name, reason);
        result.dry_run = false;
        return result;
    }
    let Some(inspection) = preflighted.inspection else {
        let mut result = preflight::fail_all(
            plan,
            preflighted.result.application_name,
            ApplicationUninstallActionReason::ComponentUnavailable,
        );
        result.dry_run = false;
        return result;
    };
    match application_target_is_running(preflighted.process_target.as_ref()) {
        Ok(false) => {}
        Ok(true) => {
            let mut result = preflight::fail_all(
                plan,
                Some(inspection.application_name),
                ApplicationUninstallActionReason::ApplicationRunning,
            );
            result.dry_run = false;
            return result;
        }
        Err(error) => {
            log::warn!(
                "application_uninstall_process_recheck_failed error={}",
                mangodisk_platform::diagnostics::text(&error)
            );
            let mut result = preflight::fail_all(
                plan,
                Some(inspection.application_name),
                ApplicationUninstallActionReason::ProcessStateUnavailable,
            );
            result.dry_run = false;
            return result;
        }
    }
    let component = inspection
        .components
        .iter()
        .find(|component| component.kind == ApplicationUninstallComponentKind::NativeInstaller);
    let Some(component) = component else {
        let mut result = preflight::fail_all(
            plan,
            Some(inspection.application_name),
            ApplicationUninstallActionReason::ComponentUnavailable,
        );
        result.dry_run = false;
        return result;
    };
    match windows::execute_registration(&inspection, &plan.plan_id, cancellation) {
        Ok(windows::ApplicationUninstallExecution::Completed(outcome)) => {
            ApplicationUninstallResult {
                plan_id: plan.plan_id.clone(),
                application_id: plan.application_id.clone(),
                application_name: Some(inspection.application_name),
                expected_bytes: plan.expected_bytes,
                previewed_bytes: 0,
                released_bytes: component.bytes,
                previewed_item_count: 0,
                affected_item_count: 1,
                failed_item_count: 0,
                released_bytes_is_estimate: true,
                restart_required: matches!(
                    outcome,
                    mangodisk_platform::ApplicationUninstallExecutionOutcome::RestartRequired
                ),
                dry_run: false,
                actions: vec![ApplicationUninstallActionResult {
                    component_id: component.component_id.clone(),
                    kind: component.kind,
                    status: ApplicationUninstallActionStatus::Completed,
                    reason: None,
                    expected_bytes: component.bytes,
                    released_bytes: component.bytes,
                }],
                history_saved: false,
            }
        }
        Ok(windows::ApplicationUninstallExecution::Detached) => preflight::cancel_all(
            plan,
            Some(inspection.application_name),
            Some(ApplicationUninstallActionReason::ExternalUninstallerContinuing),
        ),
        Ok(windows::ApplicationUninstallExecution::Cancelled) => {
            log::info!(
                "application_uninstall_native_execution_result_cancelled reason=native_user_cancelled"
            );
            let mut result = preflight::cancel_all(plan, Some(inspection.application_name), None);
            result.dry_run = false;
            result
        }
        Err(reason) => {
            log::warn!(
                "application_uninstall_native_execution_result_failed application_id={} plan_id={} reason={}",
                plan.application_id, plan.plan_id, reason.stable_code()
            );
            let mut result = preflight::fail_all(plan, Some(inspection.application_name), reason);
            result.dry_run = false;
            result
        }
    }
}

#[cfg(any(target_os = "macos", windows))]
fn application_target_is_running(
    target: Option<&ResolvedApplicationCloseTarget>,
) -> Result<bool, String> {
    let target = target.ok_or_else(|| "application process identity is unavailable".to_string())?;
    ProcessSnapshot::capture().map(|processes| {
        !processes
            .matching_application_processes(&target.executable_names, &target.executable_paths)
            .is_empty()
    })
}

#[cfg(not(any(target_os = "macos", windows)))]
fn execute_preflighted(
    plan: &ApplicationUninstallPlan,
    preflighted: PreflightCandidate,
    _cancellation: Arc<AtomicBool>,
    _authorization_prompt: Option<&str>,
) -> ApplicationUninstallResult {
    let mut result = preflight::fail_all(
        plan,
        preflighted.result.application_name,
        ApplicationUninstallActionReason::UnsupportedExecutor,
    );
    result.dry_run = false;
    result
}

#[cfg(target_os = "macos")]
fn inspect_candidate(
    candidate: &ApplicationUninstallCandidate,
    catalog_revision: &str,
    started: Instant,
) -> Result<ApplicationUninstallInspection, String> {
    macos::inspect_candidate(candidate, catalog_revision, started)
}

#[cfg(windows)]
fn inspect_candidate(
    candidate: &ApplicationUninstallCandidate,
    catalog_revision: &str,
    started: Instant,
) -> Result<ApplicationUninstallInspection, String> {
    windows::inspect_candidate(candidate, catalog_revision, started)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn inspect_candidate(
    _candidate: &ApplicationUninstallCandidate,
    _catalog_revision: &str,
    _started: Instant,
) -> Result<ApplicationUninstallInspection, String> {
    Err("application uninstall inspection is not supported on this system".to_string())
}

fn scan_without_guard(
    operation_id: u64,
    include_component_summaries: bool,
    progress_sink: Option<Box<dyn ProgressSink>>,
    cancellation: Arc<AtomicBool>,
) -> CoreResult<ApplicationUninstallScanResult> {
    let started = Instant::now();
    let progress = progress_sink.map(|sink| ProgressTracker::from_sink(operation_id, sink, 0));
    ensure_scan_not_cancelled(&cancellation)?;
    if let Some(progress) = &progress {
        progress.emit(
            TraversalStage::DiscoveringApplications,
            application_catalog_source(),
        );
    }
    let inventory_started = Instant::now();
    let platform_cancellation_flag = Arc::clone(&cancellation);
    let platform_cancellation =
        PlatformCancellation::new(move || platform_cancellation_flag.load(Ordering::Relaxed));
    let (mut context, mut revision_before) =
        SystemInventoryContext::capture_with_revision_and_cancellation(&platform_cancellation);
    let mut inventory_elapsed_ms = inventory_started.elapsed().as_millis();
    ensure_scan_not_cancelled(&cancellation)?;
    if let Some(progress) = &progress {
        progress.emit(TraversalStage::CheckingProcesses, std::path::Path::new(""));
    }
    let process_snapshot_started = Instant::now();
    let mut processes = ProcessSnapshot::capture_with_cancellation(&platform_cancellation);
    let mut process_snapshot_elapsed_ms = process_snapshot_started.elapsed().as_millis();
    ensure_scan_not_cancelled(&cancellation)?;
    let mut revision_after = current_platform()
        .system_inventory_revision_with_cancellation(&platform_cancellation)
        .ok();
    ensure_scan_not_cancelled(&cancellation)?;

    // A package manager or the operating system can update inventory evidence
    // while the first cold scan is reading it. Retry that specific race once
    // instead of publishing an empty, incomplete catalog that looks like the
    // machine has no applications. A second mismatch still fails closed via
    // `inventory_complete` below and remains visible to the adapter.
    if should_retry_changed_inventory(
        context.inventory.applications.applications_complete(),
        processes.is_ok(),
        revision_before.as_deref(),
        revision_after.as_deref(),
    ) {
        log::info!(
            "application_uninstall_inventory_changed_during_scan operation_id={operation_id} action=retry_once"
        );
        if let Some(progress) = &progress {
            progress.emit(
                TraversalStage::DiscoveringApplications,
                application_catalog_source(),
            );
        }
        let retry_inventory_started = Instant::now();
        (context, revision_before) =
            SystemInventoryContext::capture_with_revision_and_cancellation(&platform_cancellation);
        inventory_elapsed_ms =
            inventory_elapsed_ms.saturating_add(retry_inventory_started.elapsed().as_millis());
        ensure_scan_not_cancelled(&cancellation)?;
        if let Some(progress) = &progress {
            progress.emit(TraversalStage::CheckingProcesses, std::path::Path::new(""));
        }
        let retry_process_started = Instant::now();
        processes = ProcessSnapshot::capture_with_cancellation(&platform_cancellation);
        process_snapshot_elapsed_ms =
            process_snapshot_elapsed_ms.saturating_add(retry_process_started.elapsed().as_millis());
        ensure_scan_not_cancelled(&cancellation)?;
        revision_after = current_platform()
            .system_inventory_revision_with_cancellation(&platform_cancellation)
            .ok();
        ensure_scan_not_cancelled(&cancellation)?;
    }
    if let Some(progress) = &progress {
        progress.emit(
            TraversalStage::ValidatingApplications,
            application_catalog_source(),
        );
    }
    let candidate_build_started = Instant::now();
    let catalog_stable = revision_before.is_some() && revision_before == revision_after;
    let process_snapshot_complete = processes.is_ok();
    let inventory_complete = context.inventory.applications.applications_complete()
        && catalog_stable
        && process_snapshot_complete;
    let catalog_actionable = catalog_is_actionable(
        context.inventory.applications.applications_complete(),
        catalog_stable,
        process_snapshot_complete,
    );

    // A partial macOS inventory can still contain verified application
    // bundles. Exclude unreadable bundles while preserving the normal
    // uninstall behavior of every candidate whose identity can be verified.
    let installed_applications = context.inventory.applications.installed_applications();
    let self_excluded_count = installed_applications
        .iter()
        .filter(|application| is_current_application(application))
        .count() as u64;
    let discovered = discovered_candidates(installed_applications, processes.as_ref().ok());
    let hidden_count = self_excluded_count.saturating_add(
        discovered
            .iter()
            .filter(|candidate| !is_visible_candidate(candidate))
            .count() as u64,
    );
    let mut candidates = discovered
        .into_iter()
        .filter(is_visible_candidate)
        .collect::<Vec<_>>();
    #[cfg(windows)]
    let observation_stats = super::windows_observations::annotate(&mut candidates, &cancellation)
        .map_err(map_scan_error)?;
    #[cfg(not(windows))]
    let observation_stats = (0_u64, 0_u64);
    candidates.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.application_id.cmp(&right.application_id))
    });
    let candidate_build_elapsed_ms = candidate_build_started.elapsed().as_millis();
    let date_capture_started = Instant::now();
    if include_component_summaries {
        super::dates::refresh(
            &mut candidates,
            installed_applications,
            &platform_cancellation,
        );
        ensure_scan_not_cancelled(&cancellation)?;
    }
    let date_capture_elapsed_ms = date_capture_started.elapsed().as_millis();
    if let Some(progress) = &progress {
        progress.set_total_steps(candidates.len() as u64);
    }
    let mut last_progress_path = PathBuf::new();
    let component_summary_started = Instant::now();
    #[cfg(target_os = "macos")]
    let mut summary_metrics = macos::ComponentSummaryMetrics::default();
    if include_component_summaries {
        for candidate in &mut candidates {
            ensure_scan_not_cancelled(&cancellation)?;
            last_progress_path = candidate
                .application_path
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(&candidate.name));
            if let Some(progress) = &progress {
                progress
                    .visit_directory(TraversalStage::InspectingApplications, &last_progress_path);
            }
            #[cfg(target_os = "macos")]
            let (components, associated_data_complete) = macos::summarize_candidate(
                candidate,
                &cancellation,
                progress.as_ref(),
                &mut summary_metrics,
            )
            .map_err(map_scan_error)?;
            #[cfg(windows)]
            let (components, associated_data_complete) = windows::summarize_candidate(candidate);
            #[cfg(not(any(target_os = "macos", windows)))]
            let (components, associated_data_complete) = (Vec::new(), false);
            if candidate.capability.supports_execution() && !has_primary_component(&components) {
                candidate.capability = ApplicationUninstallCapability::ViewOnly;
            }
            candidate.total_bytes = components.iter().fold(0_u64, |total, component| {
                total.saturating_add(component.bytes)
            });
            if candidate.total_bytes == 0 {
                // Windows uninstall registrations often expose a useful
                // logical size even when MangoDisk cannot safely enumerate or
                // uninstall the installer payload. Preserve that platform
                // estimate for catalog presentation instead of showing 0 B.
                candidate.total_bytes = candidate.estimated_bytes;
            }
            candidate.default_selected_bytes = components
                .iter()
                .filter(|component| component.default_selected)
                .fold(0_u64, |total, component| {
                    total.saturating_add(component.bytes)
                });
            candidate.components = components;
            candidate.associated_data_complete = associated_data_complete;
            if let Some(progress) = &progress {
                progress.complete_step(
                    TraversalStage::InspectingApplications,
                    &last_progress_path,
                    candidate.total_bytes,
                );
            }
        }
    }
    if let Some(progress) = &progress {
        progress.finish(TraversalStage::InspectingApplications, &last_progress_path);
    }
    ensure_scan_not_cancelled(&cancellation)?;
    let component_summary_elapsed_ms = component_summary_started.elapsed().as_millis();
    let ready_count = candidates
        .iter()
        .filter(|candidate| candidate.capability.supports_execution())
        .count() as u64;
    let blocked_count = candidates.len() as u64 - ready_count;

    super::system_classification::log_catalog(
        operation_id,
        &candidates,
        include_component_summaries,
    );

    // Summaries alone cannot explain one user's disabled row. Emit the catalog decision with
    // the stable reference and readable name, including cached inventories on later scans.
    for candidate in candidates
        .iter()
        .filter(|candidate| !candidate.capability.supports_execution())
    {
        log::info!(
            "application_uninstall_candidate_blocked operation_id={} application_id={} application_name={} capability={:?} record_state={:?} reason={} running_process_count={} has_application_location={} known_executable_count={}",
            operation_id, candidate.application_id, mangodisk_platform::diagnostics::text(&candidate.name), candidate.capability, candidate.record_state,
            candidate.uninstall_diagnostic.map_or("none", |reason| reason.stable_code()),
            candidate.running_processes.len(), candidate.application_path.is_some(), candidate.executable_paths.len()
        );
    }

    log::info!(
            "application_uninstall_catalog_ready operation_id={} candidate_count={} ready_count={} blocked_count={} hidden_count={} self_excluded_count={} catalog_actionable={} inventory_complete={} component_summaries={} inventory_elapsed_ms={} process_snapshot_elapsed_ms={} candidate_build_elapsed_ms={} date_capture_elapsed_ms={} component_summary_elapsed_ms={} elapsed_ms={}",
            operation_id,
            candidates.len(),
            ready_count,
            blocked_count,
            hidden_count,
            self_excluded_count,
            catalog_actionable,
            inventory_complete,
            include_component_summaries,
            inventory_elapsed_ms,
            process_snapshot_elapsed_ms,
            candidate_build_elapsed_ms,
            date_capture_elapsed_ms,
            component_summary_elapsed_ms,
            started.elapsed().as_millis()
        );
    #[cfg(target_os = "macos")]
    log::info!(
        "application_uninstall_component_summary_metrics operation_id={} native_component_count={} portable_fallback_count={} spotlight_size_hit_count={} spotlight_size_fallback_count={} association_tree_count={} measured_file_count={} measured_bytes={} incomplete_component_count={}",
        operation_id,
        summary_metrics.native_component_count,
        summary_metrics.portable_fallback_count,
        summary_metrics.spotlight_size_hit_count,
        summary_metrics.spotlight_size_fallback_count,
        summary_metrics.association_tree_count,
        summary_metrics.measured_file_count,
        summary_metrics.measured_bytes,
        summary_metrics.incomplete_component_count,
    );
    Ok(ApplicationUninstallScanResult {
        schema_version: APPLICATION_UNINSTALL_SCAN_SCHEMA_VERSION,
        scanned_at_ms: crate::shared::clock::unix_time_ms(),
        supported: true,
        execution_supported: cfg!(any(target_os = "macos", windows)),
        catalog_actionable,
        inventory_complete,
        catalog_revision: revision_after,
        candidates,
        ready_count,
        blocked_count,
        hidden_count,
        #[cfg(windows)]
        related_directory_count: observation_stats.directory_count,
        #[cfg(not(windows))]
        related_directory_count: observation_stats.0,
        #[cfg(windows)]
        related_path_scan_elapsed_ms: observation_stats.elapsed_ms,
        #[cfg(not(windows))]
        related_path_scan_elapsed_ms: observation_stats.1,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

fn ensure_scan_not_cancelled(cancellation: &AtomicBool) -> CoreResult<()> {
    if cancellation.load(Ordering::Relaxed) {
        Err(CoreError::operation_cancelled())
    } else {
        Ok(())
    }
}

fn should_retry_changed_inventory(
    inventory_complete: bool,
    processes_complete: bool,
    revision_before: Option<&str>,
    revision_after: Option<&str>,
) -> bool {
    inventory_complete
        && processes_complete
        && revision_before
            .zip(revision_after)
            .is_some_and(|(before, after)| before != after)
}

fn catalog_is_actionable(
    application_inventory_complete: bool,
    catalog_stable: bool,
    process_snapshot_complete: bool,
) -> bool {
    #[cfg(target_os = "macos")]
    {
        let _ = application_inventory_complete;
        catalog_stable && process_snapshot_complete
    }
    #[cfg(not(target_os = "macos"))]
    {
        application_inventory_complete && catalog_stable && process_snapshot_complete
    }
}

#[cfg(any(target_os = "macos", windows))]
fn map_scan_error(error: String) -> CoreError {
    if error == OPERATION_CANCELLED_ERROR {
        CoreError::operation_cancelled()
    } else {
        CoreError::operation_failed(error)
    }
}

#[cfg(windows)]
fn application_catalog_source() -> &'static std::path::Path {
    std::path::Path::new(r"HKCU / HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall")
}

#[cfg(target_os = "macos")]
fn application_catalog_source() -> &'static std::path::Path {
    std::path::Path::new("/Applications")
}

#[cfg(not(any(target_os = "macos", windows)))]
fn application_catalog_source() -> &'static std::path::Path {
    std::path::Path::new("")
}

fn has_primary_component(components: &[ApplicationUninstallComponentSummary]) -> bool {
    components.iter().any(|component| {
        matches!(
            component.kind,
            ApplicationUninstallComponentKind::ApplicationBinary
                | ApplicationUninstallComponentKind::NativeInstaller
        )
    })
}

#[cfg(target_os = "macos")]
fn is_visible_candidate(candidate: &ApplicationUninstallCandidate) -> bool {
    matches!(
        candidate.capability,
        ApplicationUninstallCapability::Ready
            | ApplicationUninstallCapability::ApplicationRunning
            | ApplicationUninstallCapability::RequiresElevation
    )
}

#[cfg(not(target_os = "macos"))]
fn is_visible_candidate(_candidate: &ApplicationUninstallCandidate) -> bool {
    true
}

fn candidate(
    application: &InstalledApplication,
    processes: &ProcessSnapshot,
) -> ApplicationUninstallCandidate {
    let running_processes = processes.matching_application_processes(
        &[
            application.name.clone(),
            application.primary_identifier.clone(),
        ],
        &application.executable_paths,
    );
    let application_path = application
        .bundle_path
        .as_deref()
        .filter(|path| path.exists())
        .or_else(|| {
            application
                .executable_paths
                .iter()
                .map(PathBuf::as_path)
                .find(|path| path.exists())
        })
        .map(display_path);
    #[cfg(windows)]
    let application_path = if matches!(
        application.uninstall_registration.as_ref(),
        Some(ApplicationUninstallRegistration::WindowsAppx { .. })
    ) {
        // Package roots live under the access-controlled WindowsApps tree.
        // Explorer prompts for permanent access instead of safely revealing
        // the app, so AppX entries must not expose a misleading folder action.
        None
    } else {
        application_path
    };

    ApplicationUninstallCandidate {
        system_kind: super::system_classification::classify(application),
        application_id: application_id(application),
        primary_identifier: application.primary_identifier.clone(),
        source_identities: application
            .source_identities
            .iter()
            .map(|identity| ApplicationUninstallSourceIdentity {
                source: match identity.source {
                    ApplicationInventorySource::MacosBundle => {
                        ApplicationUninstallInventorySource::MacosBundle
                    }
                    ApplicationInventorySource::WindowsRegistry => {
                        ApplicationUninstallInventorySource::WindowsRegistry
                    }
                    ApplicationInventorySource::WindowsMsi => {
                        ApplicationUninstallInventorySource::WindowsMsi
                    }
                    ApplicationInventorySource::WindowsAppx => {
                        ApplicationUninstallInventorySource::WindowsAppx
                    }
                    ApplicationInventorySource::Winget => {
                        ApplicationUninstallInventorySource::Winget
                    }
                    ApplicationInventorySource::Steam => ApplicationUninstallInventorySource::Steam,
                    ApplicationInventorySource::Scoop => ApplicationUninstallInventorySource::Scoop,
                    ApplicationInventorySource::Chocolatey => {
                        ApplicationUninstallInventorySource::Chocolatey
                    }
                    ApplicationInventorySource::LinuxApt => {
                        ApplicationUninstallInventorySource::LinuxApt
                    }
                    ApplicationInventorySource::LinuxSnap => {
                        ApplicationUninstallInventorySource::LinuxSnap
                    }
                    ApplicationInventorySource::LinuxFlatpak => {
                        ApplicationUninstallInventorySource::LinuxFlatpak
                    }
                    ApplicationInventorySource::LinuxPacman => {
                        ApplicationUninstallInventorySource::LinuxPacman
                    }
                },
                identifier: identity.identifier.clone(),
            })
            .collect(),
        name: application.name.clone(),
        version: application.version.clone(),
        publisher: application.publisher.clone(),
        estimated_bytes: application.estimated_bytes,
        dates: super::dates::reported(application),
        platform: platform_kind(),
        installer_kind: application
            .uninstall_registration
            .as_ref()
            .map(|registration| match registration {
                ApplicationUninstallRegistration::WindowsMsi { .. } => {
                    ApplicationUninstallInstallerKind::WindowsMsi
                }
                ApplicationUninstallRegistration::WindowsAppx { .. } => {
                    ApplicationUninstallInstallerKind::WindowsAppx
                }
                ApplicationUninstallRegistration::WindowsScoop { .. } => {
                    ApplicationUninstallInstallerKind::WindowsScoop
                }
                ApplicationUninstallRegistration::WindowsChocolatey { .. } => {
                    ApplicationUninstallInstallerKind::WindowsChocolatey
                }
                ApplicationUninstallRegistration::WindowsRegistered { .. } => {
                    ApplicationUninstallInstallerKind::WindowsRegistered
                }
            }),
        execution_mode: application
            .uninstall_registration
            .as_ref()
            .map(|registration| match registration {
                ApplicationUninstallRegistration::WindowsMsi { .. }
                | ApplicationUninstallRegistration::WindowsAppx { .. }
                | ApplicationUninstallRegistration::WindowsScoop { .. } => {
                    ApplicationUninstallExecutionMode::Silent
                }
                ApplicationUninstallRegistration::WindowsChocolatey { .. } => {
                    ApplicationUninstallExecutionMode::ExternalClient
                }
                ApplicationUninstallRegistration::WindowsRegistered { command_kind, .. } => {
                    match command_kind {
                        mangodisk_platform::WindowsRegisteredUninstallKind::WingetProduct => {
                            ApplicationUninstallExecutionMode::ExternalClient
                        }
                        mangodisk_platform::WindowsRegisteredUninstallKind::Executable
                        | mangodisk_platform::WindowsRegisteredUninstallKind::Rundll32
                        | mangodisk_platform::WindowsRegisteredUninstallKind::BatchScript
                        | mangodisk_platform::WindowsRegisteredUninstallKind::UserPowerShellScript => {
                            ApplicationUninstallExecutionMode::Interactive
                        }
                    }
                }
            }),
        capability: capability(application, &running_processes),
        record_state: if running_processes.is_empty() {
            record_state(application)
        } else {
            ApplicationUninstallRecordState::Installed
        },
        uninstall_diagnostic: application.uninstall_diagnostic
            .filter(|_| application.uninstall_registration.is_none()),
        icon_path: application
            .icon_path
            .as_ref()
            .map(|path| display_path(path)),
        application_path,
        possible_related_paths: Vec::new(),
        running_processes,
        executable_paths: application.executable_paths.clone(),
        #[cfg(windows)]
        package_executable_path: application.package_executable_path.clone(),
        total_bytes: 0,
        default_selected_bytes: 0,
        associated_data_complete: false,
        components: Vec::new(),
        #[cfg(windows)]
        uninstall_registration: application.uninstall_registration.clone(),
    }
}

fn discovered_candidates(
    applications: &[InstalledApplication],
    processes: Option<&ProcessSnapshot>,
) -> Vec<ApplicationUninstallCandidate> {
    let Some(processes) = processes else {
        // Without a reliable process snapshot, Core cannot distinguish a
        // running application from a safe-to-review application. Fail closed
        // instead of publishing candidates with an incorrect capability.
        return Vec::new();
    };
    applications
        .iter()
        .filter(|application| !is_current_application(application))
        .map(|application| candidate(application, processes))
        .collect()
}

fn is_current_application(application: &InstalledApplication) -> bool {
    if application
        .identifiers
        .iter()
        .chain(std::iter::once(&application.primary_identifier))
        .any(|identifier| identifier.eq_ignore_ascii_case(crate::APPLICATION_IDENTIFIER))
    {
        return true;
    }

    #[cfg(windows)]
    {
        application
            .name
            .eq_ignore_ascii_case(CURRENT_APPLICATION_NAME)
    }

    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn record_state(application: &InstalledApplication) -> ApplicationUninstallRecordState {
    let Some(bundle_path) = application.bundle_path.as_deref() else {
        return ApplicationUninstallRecordState::Installed;
    };
    if application.uninstall_registration.is_some()
        || application.uninstall_diagnostic.is_some_and(|reason| {
            !matches!(
                reason,
                mangodisk_platform::ApplicationUninstallDiagnostic::CommandMissing
                    | mangodisk_platform::ApplicationUninstallDiagnostic::ExecutableMissing
            )
        })
        || !registered_path_is_missing(bundle_path)
        || application.executable_paths.is_empty()
        || application
            .executable_paths
            .iter()
            .any(|path| !registered_path_is_missing(path))
    {
        return ApplicationUninstallRecordState::Installed;
    }
    ApplicationUninstallRecordState::OrphanedRegistration
}

#[cfg(any(windows, test))]
use mangodisk_platform::registered_application_path_is_missing as registered_path_is_missing;

#[cfg(not(windows))]
fn record_state(_application: &InstalledApplication) -> ApplicationUninstallRecordState {
    ApplicationUninstallRecordState::Installed
}

fn application_id(application: &InstalledApplication) -> String {
    mangodisk_platform::application_uninstall_diagnostic_id(&application.catalog_identifier)
}

#[cfg(target_os = "macos")]
fn capability(
    application: &InstalledApplication,
    running_processes: &[String],
) -> ApplicationUninstallCapability {
    let Some(path) = application.bundle_path.as_deref() else {
        return ApplicationUninstallCapability::ProtectedApplication;
    };
    if path.starts_with("/System/Applications") || path.starts_with("/System/Library") {
        return ApplicationUninstallCapability::ProtectedApplication;
    }
    if !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
    {
        return ApplicationUninstallCapability::ProtectedApplication;
    }
    if !running_processes.is_empty() {
        return ApplicationUninstallCapability::ApplicationRunning;
    }
    if !macos_bundle_is_deletable_without_elevation(path) {
        return if macos_privileged_application_removal_supported(path) {
            ApplicationUninstallCapability::RequiresElevation
        } else {
            ApplicationUninstallCapability::ViewOnly
        };
    }
    ApplicationUninstallCapability::Ready
}

#[cfg(target_os = "macos")]
fn macos_bundle_is_deletable_without_elevation(path: &std::path::Path) -> bool {
    use std::{ffi::CString, fs, os::unix::ffi::OsStrExt};

    let writable_directory = |directory: &std::path::Path| {
        CString::new(directory.as_os_str().as_bytes()).is_ok_and(|directory| unsafe {
            libc::access(directory.as_ptr(), libc::W_OK | libc::X_OK) == 0
        })
    };

    let Some(parent) = path.parent() else {
        return false;
    };
    if !writable_directory(parent) {
        return false;
    }

    // Permanent deletion first renames the bundle into staging and then
    // removes the staged tree. Removing every child requires write and search
    // access to each real directory in that tree. A writable bundle root alone
    // is insufficient when a nested root-owned directory is protected.
    let mut pending_directories = vec![path.to_path_buf()];
    while let Some(directory) = pending_directories.pop() {
        if !writable_directory(&directory) {
            return false;
        }
        let Ok(entries) = fs::read_dir(&directory) else {
            return false;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                return false;
            };
            let Ok(file_type) = entry.file_type() else {
                return false;
            };
            // A symlink is removed from its containing directory. Following it
            // could cross the application boundary and misclassify permissions.
            if file_type.is_dir() && !file_type.is_symlink() {
                pending_directories.push(entry.path());
            }
        }
    }
    true
}

#[cfg(windows)]
fn capability(
    application: &InstalledApplication,
    running_processes: &[String],
) -> ApplicationUninstallCapability {
    let Some(registration) = application.uninstall_registration.as_ref() else {
        return ApplicationUninstallCapability::ViewOnly;
    };
    if !running_processes.is_empty() {
        return ApplicationUninstallCapability::ApplicationRunning;
    }
    if matches!(
        registration,
        ApplicationUninstallRegistration::WindowsMsi {
            scope: ApplicationInstallScope::Machine,
            ..
        } | ApplicationUninstallRegistration::WindowsChocolatey { .. }
    ) {
        return ApplicationUninstallCapability::RequiresElevation;
    }
    // The cold inventory validates every registration before exposing it, and scan_without_guard
    // rejects the complete candidate set unless the source revision is identical before and after
    // construction. Re-querying each AppX registration here previously launched one PowerShell
    // process per package. Planning and preflight still revalidate the selected registration at
    // the mutation boundary, so using the stable inventory fact for list capability removes only
    // redundant catalog work and does not weaken uninstall safety.
    ApplicationUninstallCapability::Ready
}

#[cfg(not(any(target_os = "macos", windows)))]
fn capability(
    _application: &InstalledApplication,
    _running_processes: &[String],
) -> ApplicationUninstallCapability {
    ApplicationUninstallCapability::ViewOnly
}

#[cfg(target_os = "macos")]
const fn platform_kind() -> ApplicationUninstallPlatform {
    ApplicationUninstallPlatform::MacosBundle
}

#[cfg(windows)]
const fn platform_kind() -> ApplicationUninstallPlatform {
    ApplicationUninstallPlatform::WindowsRegistry
}

#[cfg(target_os = "linux")]
const fn platform_kind() -> ApplicationUninstallPlatform {
    ApplicationUninstallPlatform::LinuxPackage
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
