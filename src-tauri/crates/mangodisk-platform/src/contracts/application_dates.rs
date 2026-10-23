use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApplicationDateSource {
    SpotlightLastUsed,
    SpotlightUsedDates,
    ForegroundObservation,
    KnowledgeUsage,
    BiomeFocus,
    PreferencesModified,
    ApplicationDataModified,
    CacheModified,
    DirectoryModified,
    PackageReceipt,
    DirectoryAdded,
    SpotlightAdded,
    BundleCreated,
    PlatformReported,
}

impl ApplicationDateSource {
    pub fn is_estimated(self) -> bool {
        matches!(
            self,
            Self::PreferencesModified
                | Self::ApplicationDataModified
                | Self::CacheModified
                | Self::DirectoryModified
                | Self::DirectoryAdded
                | Self::SpotlightAdded
                | Self::BundleCreated
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApplicationDatePrecision {
    Timestamp,
    /// A calendar date encoded in UTC; adapters must not shift it across time zones.
    Day,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApplicationDateReadStatus {
    #[default]
    Unavailable,
    Complete,
    Partial,
}

/// Evidence is presentation-only and must never authorize uninstall or infer disuse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationDate {
    pub timestamp_ms: u64,
    pub source: ApplicationDateSource,
    pub precision: ApplicationDatePrecision,
    pub captured_at_ms: u64,
    pub estimated: bool,
    /// A bound or unreadable subtree can hide a newer observation.
    pub partial: bool,
}

impl ApplicationDate {
    pub fn new(
        timestamp_ms: u64,
        source: ApplicationDateSource,
        captured_at_ms: u64,
    ) -> Option<Self> {
        (timestamp_ms > 0 && timestamp_ms <= captured_at_ms).then_some(Self {
            timestamp_ms,
            source,
            precision: ApplicationDatePrecision::Timestamp,
            captured_at_ms,
            estimated: source.is_estimated(),
            partial: false,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationDates {
    pub last_used: Option<ApplicationDate>,
    pub installed: Option<ApplicationDate>,
    pub read_status: ApplicationDateReadStatus,
}

/// Minimal native facts needed to refresh dates independently of inventory revisions.
pub struct ApplicationDateTarget {
    pub catalog_identifier: String,
    pub identifier: String,
    pub path: PathBuf,
    pub version: Option<String>,
    /// Computed from the complete inventory, including hidden or protected copies.
    pub identity_is_unique: bool,
}

pub struct ApplicationDateRecord {
    pub catalog_identifier: String,
    pub dates: ApplicationDates,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_rejects_empty_and_future_dates() {
        assert!(ApplicationDate::new(0, ApplicationDateSource::BundleCreated, 10).is_none());
        assert!(ApplicationDate::new(11, ApplicationDateSource::BundleCreated, 10).is_none());
        assert!(ApplicationDate::new(10, ApplicationDateSource::BundleCreated, 10).is_some());
    }

    #[test]
    fn source_and_precision_have_stable_wire_codes() {
        let mut date =
            ApplicationDate::new(10, ApplicationDateSource::SpotlightUsedDates, 20).unwrap();
        date.precision = ApplicationDatePrecision::Day;
        let wire = serde_json::to_value(date).unwrap();
        assert_eq!(wire["source"], "spotlightUsedDates");
        assert_eq!(wire["precision"], "day");
        assert_eq!(wire["timestampMs"], 10);
        assert!(ApplicationDateSource::DirectoryModified.is_estimated());
        assert!(!ApplicationDateSource::KnowledgeUsage.is_estimated());
    }
}
