use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

use mangodisk_platform::{
    current_platform, ApplicationDate, ApplicationDateReadStatus, ApplicationDateSource,
    ApplicationDateTarget, ApplicationDates, InstalledApplication, Platform, PlatformCancellation,
};

use super::models::ApplicationUninstallCandidate;

pub(super) fn reported(application: &InstalledApplication) -> ApplicationDates {
    let now = now_millis();
    ApplicationDates {
        last_used: application.last_used_at_ms.and_then(|date| {
            ApplicationDate::new(date, ApplicationDateSource::PlatformReported, now)
        }),
        installed: application.installed_at_ms.and_then(|date| {
            ApplicationDate::new(date, ApplicationDateSource::PlatformReported, now)
        }),
        read_status: ApplicationDateReadStatus::Complete,
    }
}

pub(super) fn refresh(
    candidates: &mut [ApplicationUninstallCandidate],
    applications: &[InstalledApplication],
    cancellation: &PlatformCancellation,
) {
    let indices = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| (candidate.application_id.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut identity_counts = HashMap::new();
    for application in applications {
        *identity_counts
            .entry(application.primary_identifier.as_str())
            .or_insert(0_usize) += 1;
    }
    let mut targets = Vec::new();
    let mut target_indices = HashMap::new();
    for application in applications {
        let id = mangodisk_platform::application_uninstall_diagnostic_id(
            &application.catalog_identifier,
        );
        let (Some(index), Some(path)) = (indices.get(&id), application.bundle_path.as_ref()) else {
            continue;
        };
        targets.push(ApplicationDateTarget {
            catalog_identifier: application.catalog_identifier.clone(),
            identifier: application.primary_identifier.clone(),
            path: path.clone(),
            version: application.version.clone(),
            identity_is_unique: identity_counts.get(application.primary_identifier.as_str())
                == Some(&1),
        });
        target_indices.insert(application.catalog_identifier.as_str(), *index);
    }
    for record in current_platform().application_dates(&targets, cancellation) {
        if let Some(index) = target_indices.get(record.catalog_identifier.as_str()) {
            candidates[*index].dates = record.dates;
        }
    }
    let observations = crate::applications::usage::ApplicationUsageService::dates();
    let now = now_millis();
    for target in targets {
        let Some(timestamp) = observations.get(&(target.identifier, target.path)) else {
            continue;
        };
        let Some(date) = ApplicationDate::new(
            *timestamp,
            ApplicationDateSource::ForegroundObservation,
            now,
        ) else {
            continue;
        };
        if let Some(index) = target_indices.get(target.catalog_identifier.as_str()) {
            let current = &mut candidates[*index].dates.last_used;
            if current.as_ref().is_none_or(|value| {
                value.source.is_estimated() || value.timestamp_ms < date.timestamp_ms
            }) {
                *current = Some(date);
            }
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}
