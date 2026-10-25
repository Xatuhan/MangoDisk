use super::recovery::{RecoveryDocument, RecoveryItem};
use super::{
    CatalogNativeItem, PendingChangeItem, SystemSettingChangeItemResult,
    SystemSettingChangeOutcomeStatus, SystemSettingStatus, SystemSettingTargetState,
};
use mangodisk_platform::PlatformSystemSettingValue;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn recovery_items_by_id(
    document: Option<&RecoveryDocument>,
) -> BTreeMap<String, RecoveryItem> {
    document
        .into_iter()
        .flat_map(|document| document.items.iter().cloned())
        .map(|item| (item.setting_id.clone(), item))
        .collect()
}

pub(super) fn valid_recovery_baseline(recovery: &RecoveryItem, item: &CatalogNativeItem) -> bool {
    recovery.optimized_value == item.effective_value
        && (matches!(recovery.original_value, PlatformSystemSettingValue::Missing)
            || same_value_kind(&recovery.original_value, &item.disabled_value)
            || same_value_kind(&recovery.original_value, &item.recommended_value)
            || matches!(
                (&recovery.original_value, &item.current_value),
                (
                    PlatformSystemSettingValue::Snapshot(_),
                    PlatformSystemSettingValue::Snapshot(_)
                )
            ))
}

/// Resolves the requested target without weakening an explicit recovery operation. Manual
/// switches may still use the catalog's disabled value, while a setting advertised as restorable
/// must retain its exact durable baseline until the change plan is prepared.
pub(super) fn desired_value_for_target(
    item: &CatalogNativeItem,
    target: SystemSettingTargetState,
    recovery: Option<&RecoveryItem>,
) -> Option<PlatformSystemSettingValue> {
    match target {
        SystemSettingTargetState::Optimized => Some(item.recommended_value.clone()),
        SystemSettingTargetState::Default => recovery
            .filter(|recovery| valid_recovery_baseline(recovery, item))
            .map(|recovery| recovery.original_value.clone())
            .or_else(|| (!item.public.restore_available).then(|| item.disabled_value.clone())),
    }
}

/// Marks only available settings whose current native value still matches MangoDisk's durable
/// baseline. The catalog recommendation may change between releases, so recovery is intentionally
/// independent of the current optimized status. A value changed by another tool still fails the
/// baseline match and cannot appear in the bulk restore action.
pub(super) fn mark_restore_availability(
    items: &mut [CatalogNativeItem],
    recovery: Option<&RecoveryDocument>,
) -> bool {
    let recovery_items = recovery_items_by_id(recovery);
    let mut available = false;
    for item in items {
        item.public.restore_available = item.public.status != SystemSettingStatus::Unavailable
            && recovery_items
                .get(&item.public.setting_id)
                .is_some_and(|recovery| valid_recovery_baseline(recovery, item));
        available |= item.public.restore_available;
    }
    available
}

fn same_value_kind(left: &PlatformSystemSettingValue, right: &PlatformSystemSettingValue) -> bool {
    matches!(
        (left, right),
        (
            PlatformSystemSettingValue::Boolean(_),
            PlatformSystemSettingValue::Boolean(_)
        ) | (
            PlatformSystemSettingValue::Integer(_),
            PlatformSystemSettingValue::Integer(_)
        ) | (
            PlatformSystemSettingValue::Text(_),
            PlatformSystemSettingValue::Text(_)
        )
    )
}

/// Builds the durable baseline ledger written before native changes begin.
/// The plan retains matching baselines across policy updates, but captures the current native
/// value after an external override. Unrelated settings remain reversible across batches.
pub(super) fn recovery_intent(
    previous: Option<&RecoveryDocument>,
    pending_items: &[PendingChangeItem],
) -> Vec<RecoveryItem> {
    let mut items = recovery_items_by_id(previous);
    for pending in pending_items
        .iter()
        .filter(|item| item.public.target == SystemSettingTargetState::Optimized)
    {
        items.insert(
            pending.public.setting_id.clone(),
            RecoveryItem {
                setting_id: pending.public.setting_id.clone(),
                original_value: pending.recovery_value.clone(),
                optimized_value: pending.desired_value.clone(),
            },
        );
    }
    items.into_values().collect()
}

/// Resolves the post-batch recovery ledger without filesystem side effects so that mixed enable
/// and restore batches have one deterministic source of truth that can be regression tested.
pub(super) fn reconciled_recovery_items(
    previous: Option<&RecoveryDocument>,
    preflight_items: Vec<RecoveryItem>,
    pending_items: &[PendingChangeItem],
    results: &[SystemSettingChangeItemResult],
    possibly_changed: &BTreeSet<String>,
) -> Vec<RecoveryItem> {
    let previous_items = recovery_items_by_id(previous);
    let mut items = preflight_items
        .into_iter()
        .map(|item| (item.setting_id.clone(), item))
        .collect::<BTreeMap<_, _>>();
    for pending in pending_items {
        let result = results
            .iter()
            .find(|result| result.setting_id == pending.public.setting_id);
        let succeeded = result.is_some_and(|result| {
            result.verified && result.status != SystemSettingChangeOutcomeStatus::Failed
        });
        match pending.public.target {
            SystemSettingTargetState::Default if succeeded => {
                items.remove(&pending.public.setting_id);
            }
            SystemSettingTargetState::Optimized
                if (!succeeded && !possibly_changed.contains(&pending.public.setting_id))
                    || (result.is_some_and(|result| {
                        result.status == SystemSettingChangeOutcomeStatus::Unchanged
                    }) && !previous_items.contains_key(&pending.public.setting_id)) =>
            {
                if let Some(previous) = previous_items.get(&pending.public.setting_id) {
                    items.insert(pending.public.setting_id.clone(), previous.clone());
                } else {
                    items.remove(&pending.public.setting_id);
                }
            }
            _ => {}
        }
    }
    items.into_values().collect()
}
