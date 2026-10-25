mod recovery;
mod recovery_policy;

use recovery::{
    load_recovery, recovery_exists, remove_recovery, save_recovery, RecoveryDocument, RecoveryItem,
};
use recovery_policy::{
    desired_value_for_target, mark_restore_availability, reconciled_recovery_items,
    recovery_intent, recovery_items_by_id, valid_recovery_baseline,
};

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Mutex, OnceLock},
    time::Instant,
};

use mangodisk_platform::{
    current_platform, PlatformCancellation, PlatformErrorCode, PlatformMutationState,
    PlatformSystemSettingChangeRequest, PlatformSystemSettingDiagnosticCode,
    PlatformSystemSettingValue, SystemSettingsPlatform as _,
};

use crate::{
    history::{
        HistoryService, OperationCategory, OperationDetails, OperationOutcome, OperationRecord,
        SystemOptimizationHistoryItem, SystemOptimizationHistoryItemStatus,
        SystemOptimizationOperationDetails, OPERATION_RECORD_SCHEMA_VERSION,
    },
    shared::{
        operation::{CoordinatedOperationKind, OperationCancellationToken, OperationGuard},
        CoreError, CoreResult,
    },
};

use super::{
    catalog::{definitions, SettingDefinition},
    CatalogNativeItem, CatalogSession, PendingChangeItem, PendingChangePlan,
    SystemSettingChangeFailureReason, SystemSettingChangeItemResult,
    SystemSettingChangeOutcomeStatus, SystemSettingChangePlanItem, SystemSettingChangeSkipReason,
    SystemSettingChangeSkippedItem, SystemSettingItem, SystemSettingSelectionKind,
    SystemSettingStatus, SystemSettingTargetState, SystemSettingsCatalog,
    SystemSettingsCatalogSummary, SystemSettingsChangePlan, SystemSettingsChangeResult,
    SystemSettingsChangeSelection, SystemSettingsPlatform, SYSTEM_SETTINGS_CATALOG_SCHEMA_VERSION,
    SYSTEM_SETTINGS_CHANGE_PLAN_SCHEMA_VERSION,
};

const CHANGE_PLAN_TTL_MS: u64 = 5 * 60 * 1_000;
const MAX_CHANGE_ITEMS: usize = 256;

static CATALOG_SESSION: OnceLock<Mutex<Option<CatalogSession>>> = OnceLock::new();
static CHANGE_PLAN: OnceLock<Mutex<Option<PendingChangePlan>>> = OnceLock::new();

pub struct SystemSettingsService;

impl SystemSettingsService {
    pub fn cancel_scan() {
        OperationCancellationToken::system_settings_scan().cancel();
    }

    pub fn scan() -> CoreResult<SystemSettingsCatalog> {
        let operation = OperationGuard::start(CoordinatedOperationKind::SystemSettingsScan)?;
        let session = capture_catalog(&operation)?;
        replace_catalog_session(session.clone())?;
        log_catalog(operation.id(), &session.public);
        operation.complete();
        Ok(session.public)
    }

    pub fn prepare_change(
        selection: SystemSettingsChangeSelection,
    ) -> CoreResult<SystemSettingsChangePlan> {
        validate_selection(&selection)?;
        let operation = OperationGuard::start(CoordinatedOperationKind::SystemSettingsChange)?;
        let session = current_catalog_session()?;
        if session.public.scan_id != selection.scan_id {
            return Err(CoreError::invalid_input(
                "system settings catalog session has expired",
            ));
        }

        let recovery = load_recovery()?;
        let pending = build_change_plan(
            selection,
            session,
            recovery.as_ref(),
            crate::shared::clock::unix_time_ms(),
        );
        let public = pending.public.clone();
        replace_change_plan(pending)?;
        log::info!(
            "system_settings_change_prepared operation_id={} target_count={} enable_count={} restore_count={} skipped_count={} restart_required={}",
            operation.id(),
            public.items.len(),
            public
                .items
                .iter()
                .filter(|item| item.target == SystemSettingTargetState::Optimized)
                .count(),
            public
                .items
                .iter()
                .filter(|item| item.target == SystemSettingTargetState::Default)
                .count(),
            public.skipped_items.len(),
            public.requires_restart
        );
        operation.complete();
        Ok(public)
    }

    pub fn execute_change(plan_id: String) -> CoreResult<SystemSettingsChangeResult> {
        validate_plan_id(&plan_id)?;
        let operation = OperationGuard::start(CoordinatedOperationKind::SystemSettingsChange)?;
        let started_at_ms = crate::shared::clock::unix_time_ms();
        let pending = take_change_plan(&plan_id)?;
        if crate::shared::clock::unix_time_ms() > pending.public.expires_at_ms {
            return Err(CoreError::invalid_input(
                "system settings change plan has expired",
            ));
        }

        // Cancellation must be observed before replacing the durable recovery record. Once the
        // preflight intent is persisted there is no further cancellable Core step before the
        // platform batch, otherwise an early return could discard recovery for a prior operation.
        operation.ensure_not_cancelled()?;
        let platform = current_platform();
        let mut results = Vec::with_capacity(pending.items.len());
        let mut possibly_changed = BTreeSet::new();
        let previous_recovery = load_recovery()?;
        let preflight_recovery = recovery_intent(previous_recovery.as_ref(), &pending.items);
        // Persist the merged per-setting baseline before the first operating-system mutation.
        // This keeps earlier settings reversible when a later batch changes unrelated items.
        if !preflight_recovery.is_empty() {
            save_recovery(&plan_id, preflight_recovery.clone(), operation.id())?;
        }
        let requests = pending
            .items
            .iter()
            .map(|item| PlatformSystemSettingChangeRequest {
                setting_id: item.public.setting_id.clone(),
                expected_value: item.expected_value.clone(),
                desired_value: item.desired_value.clone(),
            })
            .collect::<Vec<_>>();
        let platform_results = match platform.change_system_settings(&requests) {
            Ok(platform_results) if platform_results.len() == pending.items.len() => {
                Some(platform_results)
            }
            Ok(platform_results) => {
                log::error!(
                    "system_settings_change_batch_count_invalid operation_id={} expected_count={} actual_count={}",
                    operation.id(),
                    pending.items.len(),
                    platform_results.len()
                );
                for item in &pending.items {
                    possibly_changed.insert(item.public.setting_id.clone());
                    results.push(change_result(
                        item.public.setting_id.clone(),
                        SystemSettingChangeOutcomeStatus::Failed,
                        false,
                        Some(SystemSettingChangeFailureReason::PlatformFailure),
                    ));
                }
                None
            }
            Err(error) => {
                log::warn!(
                    "system_settings_change_batch_failed operation_id={} code={:?} error={}",
                    operation.id(),
                    error.code(),
                    mangodisk_platform::diagnostics::text(&error)
                );
                for item in &pending.items {
                    if error.mutation_state() == PlatformMutationState::MayHaveChanged {
                        possibly_changed.insert(item.public.setting_id.clone());
                    }
                    results.push(change_result(
                        item.public.setting_id.clone(),
                        SystemSettingChangeOutcomeStatus::Failed,
                        false,
                        Some(failure_reason(error.code())),
                    ));
                }
                None
            }
        };
        for (item, platform_result) in pending
            .items
            .iter()
            .zip(platform_results.into_iter().flatten())
        {
            match platform_result {
                Ok(result) if result.verified => {
                    let status = if result.changed {
                        SystemSettingChangeOutcomeStatus::Changed
                    } else {
                        SystemSettingChangeOutcomeStatus::Unchanged
                    };
                    log::info!(
                        "system_setting_change_completed operation_id={} setting_id={} target={:?} changed={} verified=true restart_required={}",
                        operation.id(), item.public.setting_id, item.public.target, result.changed,
                        item.public.requires_restart
                    );
                    results.push(change_result(
                        item.public.setting_id.clone(),
                        status,
                        true,
                        None,
                    ));
                }
                Ok(_) => {
                    possibly_changed.insert(item.public.setting_id.clone());
                    log::warn!(
                        "system_setting_change_verification_failed operation_id={} setting_id={} mutation_state=may_have_changed",
                        operation.id(),
                        item.public.setting_id
                    );
                    results.push(change_result(
                        item.public.setting_id.clone(),
                        SystemSettingChangeOutcomeStatus::Failed,
                        false,
                        Some(SystemSettingChangeFailureReason::VerificationFailed),
                    ));
                }
                Err(error) => {
                    let mutation_state = error.mutation_state();
                    if mutation_state == PlatformMutationState::MayHaveChanged {
                        possibly_changed.insert(item.public.setting_id.clone());
                    }
                    log::warn!(
                        "system_setting_change_failed operation_id={} setting_id={} code={:?} mutation_state={:?} error={}",
                        operation.id(),
                        item.public.setting_id,
                        error.code(),
                        mutation_state,
                        mangodisk_platform::diagnostics::text(&error)
                    );
                    results.push(change_result(
                        item.public.setting_id.clone(),
                        SystemSettingChangeOutcomeStatus::Failed,
                        false,
                        Some(failure_reason(error.code())),
                    ));
                }
            }
        }

        let persisted_recovery_available = reconcile_recovery(
            previous_recovery.as_ref(),
            &plan_id,
            preflight_recovery,
            &pending.items,
            &results,
            &possibly_changed,
            operation.id(),
        );
        let changed_count = count_status(&results, SystemSettingChangeOutcomeStatus::Changed);
        let failed_count = count_status(&results, SystemSettingChangeOutcomeStatus::Failed);
        let requires_restart = changed_items_require_restart(&pending.items, &results);
        let catalog = capture_catalog(&operation)
            .and_then(|session| {
                replace_catalog_session(session.clone())?;
                Ok(session.public)
            })
            .ok();
        let recovery_available = catalog
            .as_ref()
            .map(|catalog| catalog.recovery_available)
            .unwrap_or(persisted_recovery_available);
        log::info!(
            "system_settings_change_finished operation_id={} changed_count={} failed_count={} uncertain_mutation_count={} recovery_available={}",
            operation.id(),
            changed_count,
            failed_count,
            possibly_changed.len(),
            recovery_available
        );
        append_history(
            &plan_id,
            started_at_ms,
            &pending.items,
            &results,
            changed_count,
            failed_count,
        );
        operation.complete();
        Ok(SystemSettingsChangeResult {
            plan_id,
            changed_count,
            failed_count,
            requires_restart,
            recovery_available,
            items: results,
            catalog,
        })
    }
}

/// Builds a deterministic, side-effect-free change plan from one authoritative catalog.
/// Keeping selection decisions in one function prevents GUI and CLI adapters from diverging and
/// lets recovery-sensitive skip behavior be tested without reading or changing real settings.
fn build_change_plan(
    selection: SystemSettingsChangeSelection,
    session: CatalogSession,
    recovery: Option<&RecoveryDocument>,
    created_at_ms: u64,
) -> PendingChangePlan {
    let recovery_items = recovery_items_by_id(recovery);
    let mut items = Vec::new();
    let mut skipped_items = Vec::new();
    for change in selection.items {
        let setting_id = change.setting_id;
        let Some(item) = session
            .native_items
            .iter()
            .find(|item| item.public.setting_id == setting_id)
        else {
            skipped_items.push(skipped(
                setting_id,
                SystemSettingChangeSkipReason::SettingMissing,
            ));
            continue;
        };
        if item.public.status == SystemSettingStatus::Unavailable {
            skipped_items.push(skipped(
                setting_id,
                skip_reason_for_diagnostic(item.public.diagnostic),
            ));
            continue;
        }
        if change.target == SystemSettingTargetState::Optimized
            && item.public.status == SystemSettingStatus::Optimized
        {
            skipped_items.push(skipped(
                setting_id,
                SystemSettingChangeSkipReason::AlreadyOptimized,
            ));
            continue;
        }
        if change.target == SystemSettingTargetState::Default
            && item.public.status != SystemSettingStatus::Optimized
            && !item.public.restore_available
        {
            skipped_items.push(skipped(
                setting_id,
                SystemSettingChangeSkipReason::AlreadyDefault,
            ));
            continue;
        }
        let Some(desired_value) =
            desired_value_for_target(item, change.target, recovery_items.get(&setting_id))
        else {
            skipped_items.push(skipped(
                setting_id,
                SystemSettingChangeSkipReason::SettingChanged,
            ));
            continue;
        };
        let public = SystemSettingChangePlanItem {
            setting_id: setting_id.clone(),
            category: item.public.category,
            target: change.target,
            requires_restart: item.public.requires_restart,
            requires_elevation: item.public.requires_elevation,
        };
        items.push(PendingChangeItem {
            public,
            expected_value: item.current_value.clone(),
            desired_value,
            recovery_value: recovery_items
                .get(&setting_id)
                .filter(|recovery| valid_recovery_baseline(recovery, item))
                .map_or_else(
                    || item.current_value.clone(),
                    |recovery| recovery.original_value.clone(),
                ),
        });
    }

    let expires_at_ms = created_at_ms.saturating_add(CHANGE_PLAN_TTL_MS);
    let plan_id = stable_id(
        "system-settings-plan",
        &[
            session.public.catalog_revision.as_str(),
            created_at_ms.to_string().as_str(),
            items
                .iter()
                .map(|item| {
                    format!(
                        "{}:{}",
                        item.public.setting_id,
                        target_state_name(item.public.target)
                    )
                })
                .collect::<Vec<_>>()
                .join("|")
                .as_str(),
        ],
    );
    let public = SystemSettingsChangePlan {
        schema_version: SYSTEM_SETTINGS_CHANGE_PLAN_SCHEMA_VERSION,
        plan_id,
        scan_id: session.public.scan_id,
        catalog_revision: session.public.catalog_revision,
        created_at_ms,
        expires_at_ms,
        items: items.iter().map(|item| item.public.clone()).collect(),
        skipped_items,
        requires_confirmation: !items.is_empty(),
        requires_restart: items.iter().any(|item| item.public.requires_restart),
    };
    PendingChangePlan { public, items }
}

fn capture_catalog(operation: &OperationGuard) -> CoreResult<CatalogSession> {
    let started = Instant::now();
    let platform_kind = current_platform_kind();
    let definitions = definitions(platform_kind);
    let setting_ids = definitions.iter().map(|item| item.id).collect::<Vec<_>>();
    let cancellation = PlatformCancellation::new({
        let cancelled = operation.cancellation_flag();
        move || cancelled.load(std::sync::atomic::Ordering::Relaxed)
    });
    let states = current_platform().scan_system_settings(&setting_ids, &cancellation)?;
    operation.ensure_not_cancelled()?;
    let native_items = catalog_native_items(platform_kind, states)?;
    let recovery = load_recovery()?;
    Ok(assemble_catalog_session(
        platform_kind,
        started,
        native_items,
        recovery.as_ref(),
    ))
}

/// Validates the adapter inventory before applying declarative setting definitions. Missing
/// states remain visible as unavailable settings, while duplicate or unknown identifiers fail
/// closed because silently overwriting them could authorize a change from ambiguous evidence.
fn catalog_native_items(
    platform_kind: SystemSettingsPlatform,
    states: Vec<mangodisk_platform::PlatformSystemSettingState>,
) -> CoreResult<Vec<CatalogNativeItem>> {
    let definitions = definitions(platform_kind);
    let known_ids = definitions
        .iter()
        .map(|definition| definition.id)
        .collect::<BTreeSet<_>>();
    let mut indexed_states = BTreeMap::new();
    for state in states {
        if !known_ids.contains(state.setting_id.as_str()) {
            return Err(CoreError::new(
                crate::shared::CoreErrorCode::Platform,
                "system settings adapter returned an unknown setting",
            ));
        }
        if indexed_states
            .insert(state.setting_id.clone(), state)
            .is_some()
        {
            return Err(CoreError::new(
                crate::shared::CoreErrorCode::Platform,
                "system settings adapter returned a duplicate setting",
            ));
        }
    }
    Ok(definitions
        .iter()
        .map(|definition| native_item(definition, indexed_states.get(definition.id)))
        .collect())
}

fn assemble_catalog_session(
    platform_kind: SystemSettingsPlatform,
    started: Instant,
    mut native_items: Vec<CatalogNativeItem>,
    recovery: Option<&RecoveryDocument>,
) -> CatalogSession {
    let recovery_available = mark_restore_availability(&mut native_items, recovery);
    let scanned_at_ms = crate::shared::clock::unix_time_ms();
    let catalog_revision = catalog_revision(&native_items);
    let scan_id = stable_id(
        "system-settings-scan",
        &[
            catalog_revision.as_str(),
            scanned_at_ms.to_string().as_str(),
        ],
    );
    let items = native_items
        .iter()
        .map(|item| item.public.clone())
        .collect::<Vec<_>>();
    let public = SystemSettingsCatalog {
        schema_version: SYSTEM_SETTINGS_CATALOG_SCHEMA_VERSION,
        scan_id,
        catalog_revision,
        platform: platform_kind,
        scanned_at_ms,
        summary: summary(&items),
        items,
        elapsed_ms: started.elapsed().as_millis() as u64,
        recovery_available,
    };
    CatalogSession {
        public,
        native_items,
    }
}

fn native_item(
    definition: &SettingDefinition,
    state: Option<&mangodisk_platform::PlatformSystemSettingState>,
) -> CatalogNativeItem {
    let elevation_mismatch =
        state.is_some_and(|state| state.requires_elevation != definition.requires_elevation);
    let diagnostic = if elevation_mismatch {
        Some(PlatformSystemSettingDiagnosticCode::InvalidData)
    } else {
        state.and_then(|state| state.diagnostic).or_else(|| {
            state
                .is_none()
                .then_some(PlatformSystemSettingDiagnosticCode::StateUnavailable)
        })
    };
    let current_value = state
        .map(|state| state.value.clone())
        .unwrap_or(PlatformSystemSettingValue::Missing);
    let effective_value = if current_value == PlatformSystemSettingValue::Missing {
        definition.default_value.owned()
    } else {
        state
            .map(|state| state.effective_value.clone())
            .unwrap_or_else(|| current_value.clone())
    };
    let recommended_value = definition.recommended_value.owned();
    let disabled_value = definition
        .disabled_value
        .unwrap_or(definition.default_value)
        .owned();
    let status = if diagnostic.is_some() {
        SystemSettingStatus::Unavailable
    } else if effective_value == recommended_value {
        SystemSettingStatus::Optimized
    } else {
        SystemSettingStatus::Recommended
    };
    CatalogNativeItem {
        public: SystemSettingItem {
            setting_id: definition.id.to_string(),
            category: definition.category,
            selection_kind: definition.selection_kind,
            risk_level: definition.risk_level,
            status,
            selected_by_default: status == SystemSettingStatus::Recommended
                && definition.selection_kind == SystemSettingSelectionKind::OneClick,
            restore_available: false,
            requires_restart: definition.requires_restart,
            requires_elevation: state
                .map(|state| state.requires_elevation)
                .unwrap_or(definition.requires_elevation),
            diagnostic,
        },
        current_value,
        effective_value,
        disabled_value,
        recommended_value,
    }
}

fn summary(items: &[SystemSettingItem]) -> SystemSettingsCatalogSummary {
    SystemSettingsCatalogSummary {
        item_count: items.len() as u64,
        recommended_count: items
            .iter()
            .filter(|item| item.status == SystemSettingStatus::Recommended)
            .count() as u64,
        optimized_count: items
            .iter()
            .filter(|item| item.status == SystemSettingStatus::Optimized)
            .count() as u64,
        selected_count: items.iter().filter(|item| item.selected_by_default).count() as u64,
        unavailable_count: items
            .iter()
            .filter(|item| item.status == SystemSettingStatus::Unavailable)
            .count() as u64,
    }
}

fn catalog_revision(items: &[CatalogNativeItem]) -> String {
    let serialized = serde_json::to_vec(
        &items
            .iter()
            .map(|item| {
                (
                    &item.public.setting_id,
                    &item.current_value,
                    item.public.diagnostic,
                )
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_default();
    blake3::hash(&serialized).to_hex().to_string()
}

fn current_platform_kind() -> SystemSettingsPlatform {
    #[cfg(target_os = "macos")]
    {
        SystemSettingsPlatform::Macos
    }
    #[cfg(windows)]
    {
        SystemSettingsPlatform::Windows
    }
    #[cfg(target_os = "linux")]
    {
        SystemSettingsPlatform::Linux
    }
}

fn validate_selection(selection: &SystemSettingsChangeSelection) -> CoreResult<()> {
    if selection.scan_id.is_empty() || selection.scan_id.len() > 128 {
        return Err(CoreError::invalid_input(
            "system settings scan identifier is invalid",
        ));
    }
    if selection.items.is_empty() || selection.items.len() > MAX_CHANGE_ITEMS {
        return Err(CoreError::invalid_input(
            "system settings selection is invalid",
        ));
    }
    if selection
        .items
        .iter()
        .any(|item| item.setting_id.is_empty() || item.setting_id.len() > 128)
    {
        return Err(CoreError::invalid_input(
            "system setting identifier is invalid",
        ));
    }
    let unique_ids = selection
        .items
        .iter()
        .map(|item| item.setting_id.as_str())
        .collect::<BTreeSet<_>>();
    if unique_ids.len() != selection.items.len() {
        return Err(CoreError::invalid_input(
            "system settings selection contains duplicate identifiers",
        ));
    }
    Ok(())
}

fn validate_plan_id(plan_id: &str) -> CoreResult<()> {
    if plan_id.is_empty() || plan_id.len() > 128 {
        Err(CoreError::invalid_input(
            "system settings plan identifier is invalid",
        ))
    } else {
        Ok(())
    }
}

fn stable_id(namespace: &str, parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(namespace.as_bytes());
    for part in parts {
        hasher.update(&[0]);
        hasher.update(part.as_bytes());
    }
    format!("{namespace}-{}", &hasher.finalize().to_hex()[..24])
}

fn target_state_name(target: SystemSettingTargetState) -> &'static str {
    match target {
        SystemSettingTargetState::Optimized => "optimized",
        SystemSettingTargetState::Default => "default",
    }
}

fn skipped(
    setting_id: String,
    reason: SystemSettingChangeSkipReason,
) -> SystemSettingChangeSkippedItem {
    SystemSettingChangeSkippedItem { setting_id, reason }
}

fn skip_reason_for_diagnostic(
    diagnostic: Option<PlatformSystemSettingDiagnosticCode>,
) -> SystemSettingChangeSkipReason {
    match diagnostic {
        Some(PlatformSystemSettingDiagnosticCode::Unsupported) => {
            SystemSettingChangeSkipReason::Unsupported
        }
        _ => SystemSettingChangeSkipReason::StateUnavailable,
    }
}

fn failure_reason(code: PlatformErrorCode) -> SystemSettingChangeFailureReason {
    match code {
        PlatformErrorCode::AccessDenied => SystemSettingChangeFailureReason::PermissionDenied,
        PlatformErrorCode::UserCancelled => SystemSettingChangeFailureReason::UserCancelled,
        PlatformErrorCode::ItemChanged => SystemSettingChangeFailureReason::SettingChanged,
        PlatformErrorCode::Unsupported => SystemSettingChangeFailureReason::Unsupported,
        _ => SystemSettingChangeFailureReason::PlatformFailure,
    }
}

fn append_history(
    plan_id: &str,
    started_at_ms: u64,
    pending_items: &[PendingChangeItem],
    results: &[SystemSettingChangeItemResult],
    changed_count: u64,
    failed_count: u64,
) {
    if results.is_empty() {
        return;
    }
    let cancelled = results
        .iter()
        .any(|item| item.failure_reason == Some(SystemSettingChangeFailureReason::UserCancelled));
    let items = results
        .iter()
        .map(|item| SystemOptimizationHistoryItem {
            setting_id: item.setting_id.clone(),
            status: match item.status {
                SystemSettingChangeOutcomeStatus::Changed => {
                    SystemOptimizationHistoryItemStatus::Changed
                }
                SystemSettingChangeOutcomeStatus::Unchanged => {
                    SystemOptimizationHistoryItemStatus::Unchanged
                }
                SystemSettingChangeOutcomeStatus::Failed => {
                    SystemOptimizationHistoryItemStatus::Failed
                }
            },
            failure_reason: item.failure_reason,
            desired_optimized: pending_items
                .iter()
                .find(|pending| pending.public.setting_id == item.setting_id)
                .map(|pending| pending.public.target == SystemSettingTargetState::Optimized),
        })
        .collect::<Vec<_>>();
    let time_component = started_at_ms.to_string();
    let record = OperationRecord {
        schema_version: OPERATION_RECORD_SCHEMA_VERSION,
        operation_id: stable_id("system-settings-change", &[plan_id, &time_component]),
        category: OperationCategory::SystemOptimization,
        started_at_ms,
        finished_at_ms: crate::shared::clock::unix_time_ms(),
        outcome: if cancelled {
            OperationOutcome::Cancelled
        } else if failed_count > 0 {
            OperationOutcome::CompletedWithWarnings
        } else {
            OperationOutcome::Completed
        },
        dry_run: false,
        selected_item_count: items.len() as u64,
        affected_item_count: changed_count,
        expected_bytes: 0,
        released_bytes: None,
        released_bytes_is_estimate: false,
        failed_item_count: failed_count,
        details: OperationDetails::SystemOptimization(SystemOptimizationOperationDetails {
            plan_id: plan_id.to_string(),
            restoration: false,
            items,
        }),
    };
    let history_operation_id = record.operation_id.clone();
    if let Err(error) = HistoryService::append(record) {
        log::warn!(
            "system_settings_history_save_failed history_operation_id={} error={}",
            history_operation_id,
            mangodisk_platform::diagnostics::text(&error)
        );
    }
}

fn change_result(
    setting_id: String,
    status: SystemSettingChangeOutcomeStatus,
    verified: bool,
    failure_reason: Option<SystemSettingChangeFailureReason>,
) -> SystemSettingChangeItemResult {
    SystemSettingChangeItemResult {
        setting_id,
        status,
        verified,
        failure_reason,
    }
}

fn count_status(
    items: &[SystemSettingChangeItemResult],
    status: SystemSettingChangeOutcomeStatus,
) -> u64 {
    items.iter().filter(|item| item.status == status).count() as u64
}

fn changed_items_require_restart(
    pending_items: &[PendingChangeItem],
    results: &[SystemSettingChangeItemResult],
) -> bool {
    pending_items.iter().any(|pending| {
        pending.public.requires_restart
            && results.iter().any(|result| {
                result.setting_id == pending.public.setting_id
                    && result.status == SystemSettingChangeOutcomeStatus::Changed
            })
    })
}

fn catalog_session_lock() -> &'static Mutex<Option<CatalogSession>> {
    CATALOG_SESSION.get_or_init(|| Mutex::new(None))
}

fn change_plan_lock() -> &'static Mutex<Option<PendingChangePlan>> {
    CHANGE_PLAN.get_or_init(|| Mutex::new(None))
}

fn replace_catalog_session(session: CatalogSession) -> CoreResult<()> {
    *catalog_session_lock()
        .lock()
        .map_err(|_| CoreError::operation_failed("system settings catalog lock is poisoned"))? =
        Some(session);
    Ok(())
}

fn current_catalog_session() -> CoreResult<CatalogSession> {
    catalog_session_lock()
        .lock()
        .map_err(|_| CoreError::operation_failed("system settings catalog lock is poisoned"))?
        .clone()
        .ok_or_else(|| CoreError::invalid_input("scan system settings before preparing changes"))
}

fn replace_change_plan(plan: PendingChangePlan) -> CoreResult<()> {
    *change_plan_lock()
        .lock()
        .map_err(|_| CoreError::operation_failed("system settings plan lock is poisoned"))? =
        Some(plan);
    Ok(())
}

fn take_change_plan(plan_id: &str) -> CoreResult<PendingChangePlan> {
    let mut guard = change_plan_lock()
        .lock()
        .map_err(|_| CoreError::operation_failed("system settings plan lock is poisoned"))?;
    if guard
        .as_ref()
        .is_none_or(|plan| plan.public.plan_id != plan_id)
    {
        return Err(CoreError::invalid_input(
            "system settings change plan is unavailable",
        ));
    }
    guard
        .take()
        .ok_or_else(|| CoreError::invalid_input("system settings change plan is unavailable"))
}

fn reconcile_recovery(
    previous: Option<&RecoveryDocument>,
    recovery_id: &str,
    preflight_items: Vec<RecoveryItem>,
    pending_items: &[PendingChangeItem],
    results: &[SystemSettingChangeItemResult],
    possibly_changed: &BTreeSet<String>,
    operation_id: u64,
) -> bool {
    let items = reconciled_recovery_items(
        previous,
        preflight_items,
        pending_items,
        results,
        possibly_changed,
    );
    log::info!(
        "system_settings_recovery_reconciled operation_id={} previous_count={} retained_count={} uncertain_mutation_count={}",
        operation_id,
        previous.map_or(0, |document| document.items.len()),
        items.len(),
        possibly_changed.len()
    );
    let result = if items.is_empty() {
        remove_recovery().map(|_| false)
    } else {
        save_recovery(recovery_id, items, operation_id).map(|_| true)
    };
    match result {
        Ok(available) => available,
        Err(error) => {
            // The preflight document is already durable. A failed compaction must not turn a
            // successfully changed operation into an error or hide the conservative recovery.
            log::warn!(
                "system_settings_recovery_reconcile_failed operation_id={} error={}",
                operation_id,
                mangodisk_platform::diagnostics::text(&error)
            );
            recovery_exists()
        }
    }
}

fn log_catalog(operation_id: u64, catalog: &SystemSettingsCatalog) {
    let restorable_count = catalog
        .items
        .iter()
        .filter(|item| item.restore_available)
        .count();
    log::info!(
        "system_settings_catalog_scanned operation_id={} platform={:?} item_count={} recommended_count={} selected_count={} restorable_count={} unavailable_count={} elapsed_ms={}",
        operation_id,
        catalog.platform,
        catalog.summary.item_count,
        catalog.summary.recommended_count,
        catalog.summary.selected_count,
        restorable_count,
        catalog.summary.unavailable_count,
        catalog.elapsed_ms
    );
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
