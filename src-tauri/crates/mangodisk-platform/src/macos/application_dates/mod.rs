mod activity;
mod history;
mod metadata;
mod receipts;

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::{
    ApplicationDate, ApplicationDateReadStatus, ApplicationDateRecord, ApplicationDateTarget,
    PlatformCancellation,
};

pub(super) fn capture(
    targets: &[ApplicationDateTarget],
    cancellation: &PlatformCancellation,
) -> Vec<ApplicationDateRecord> {
    let started = Instant::now();
    let now = unix_millis(SystemTime::now()).unwrap_or_default();
    let mut records = Vec::with_capacity(targets.len());
    let history = history::capture(now, cancellation);
    let mut receipts = receipts::ReceiptIndex::read(cancellation);
    let mut activity_budget = activity::ActivityBudget::new();
    let mut reported = 0;
    let mut estimated = 0;
    for target in targets {
        if cancellation.is_cancelled() {
            break;
        }
        let mut dates = metadata::read(target, now);
        // System histories contain bundle identifiers, not installation paths. A shared identity
        // cannot attribute history or preferences to one of several installed copies.
        if target.identity_is_unique {
            if let Some(observation) = history.by_identifier.get(&target.identifier) {
                let mut observation = observation.clone();
                observation.partial = history.partial;
                choose_latest(&mut dates.last_used, observation);
            }
            if dates.last_used.is_none() {
                dates.last_used =
                    activity::estimate(target, now, cancellation, &mut activity_budget);
            }
        }
        if let Some(installed) = receipts.installation(target, now, cancellation) {
            dates.installed = Some(installed);
        }
        if history.partial || activity_budget.partial {
            dates.read_status = ApplicationDateReadStatus::Partial;
        }
        if let Some(date) = &dates.last_used {
            if date.source.is_estimated() {
                estimated += 1;
            } else {
                reported += 1;
            }
        }
        records.push(ApplicationDateRecord {
            catalog_identifier: target.catalog_identifier.clone(),
            dates,
        });
    }
    log::info!("application_date_capture_finished requested={} resolved_usage={} estimated_usage={} installed={} history_partial={} activity_entries={} activity_native_pages={} activity_portable_directories={} activity_bounded={} elapsed_ms={}",
        targets.len(), reported, estimated, records.iter().filter(|record| record.dates.installed.is_some()).count(),
        history.partial, activity_budget.entries, activity_budget.native_pages, activity_budget.portable_directories, activity_budget.partial, started.elapsed().as_millis());
    records
}

pub(super) fn unix_millis(time: SystemTime) -> Option<u64> {
    u64::try_from(time.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()
}

pub(super) fn apple_millis(seconds: f64) -> Option<u64> {
    let milliseconds = (seconds + 978_307_200.0) * 1_000.0;
    (milliseconds.is_finite() && milliseconds > 0.0 && milliseconds < u64::MAX as f64)
        .then_some(milliseconds as u64)
}

pub(super) fn choose_latest(current: &mut Option<ApplicationDate>, candidate: ApplicationDate) {
    if current
        .as_ref()
        .is_none_or(|value| candidate.timestamp_ms > value.timestamp_ms)
    {
        *current = Some(candidate);
    }
}

pub(super) fn valid_identifier(identifier: &str) -> bool {
    !identifier.is_empty()
        && identifier.len() <= 255
        && identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        && identifier != "."
        && identifier != ".."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_times_reject_invalid_numbers_and_traversal_identifiers() {
        assert_eq!(apple_millis(0.0), Some(978_307_200_000));
        assert!(apple_millis(f64::NAN).is_none());
        assert!(apple_millis(f64::INFINITY).is_none());
        for identifier in ["", ".", "..", "../other", "a/b", "a\\b", "a\0b"] {
            assert!(
                !valid_identifier(identifier),
                "unsafe identifier: {identifier:?}"
            );
        }
        assert!(valid_identifier("ynote-desktop"));
        assert!(valid_identifier("com.example.App"));
    }

    #[test]
    #[ignore = "reads the host application inventory and date evidence"]
    fn actual_host_date_capture_probe() {
        use crate::Platform;
        let platform = crate::current_platform();
        let inventory = platform.system_inventory().unwrap();
        let mut counts = std::collections::HashMap::new();
        for app in &inventory.installed_applications {
            *counts
                .entry(app.primary_identifier.clone())
                .or_insert(0_usize) += 1;
        }
        let targets = inventory
            .installed_applications
            .into_iter()
            .filter_map(|app| {
                let path = app.bundle_path?;
                if path.starts_with("/System") || app.primary_identifier == "app.mangodisk.desktop"
                {
                    return None;
                }
                Some(ApplicationDateTarget {
                    catalog_identifier: app.catalog_identifier,
                    identity_is_unique: counts.get(&app.primary_identifier) == Some(&1),
                    identifier: app.primary_identifier,
                    path,
                    version: app.version,
                })
            })
            .collect::<Vec<_>>();
        for iteration in 0..3 {
            let start = Instant::now();
            let records = capture(&targets, &PlatformCancellation::new(|| false));
            println!("application_date_probe iteration={} applications={} usage={} estimated={} installed={} partial={} elapsed_ms={}", iteration,
                records.len(), records.iter().filter(|record| record.dates.last_used.is_some()).count(),
                records.iter().filter(|record| record.dates.last_used.as_ref().is_some_and(|date| date.estimated)).count(),
                records.iter().filter(|record| record.dates.installed.is_some()).count(),
                records.iter().filter(|record| record.dates.read_status == ApplicationDateReadStatus::Partial).count(), start.elapsed().as_millis());
        }
    }
}
