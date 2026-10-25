//! Optional physical battery facts. Unknown values retain their native absence.
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CapacityUnit {
    MilliampHours,
    MilliwattHours,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryCapacity {
    pub current: Option<f64>,
    pub full_charge: Option<f64>,
    pub design: Option<f64>,
    pub unit: CapacityUnit,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ChargeState {
    Charging,
    Discharging,
    Full,
    Paused,
    Connected,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerAdapter {
    pub input_watts: Option<f64>,
    pub reported_watts: Option<f64>,
    pub voltage_volts: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryDetails {
    pub charge_percent: Option<f64>,
    pub external_connected: Option<bool>,
    pub charge_state: ChargeState,
    pub seconds_to_empty: Option<u64>,
    pub seconds_to_full: Option<u64>,
    pub health_percent: Option<f64>,
    pub cycle_count: Option<u32>,
    pub capacity: Option<BatteryCapacity>,
    pub voltage_volts: Option<f64>,
    pub current_milliamps: Option<f64>,
    pub adapter: Option<PowerAdapter>,
}

pub(super) fn bounded(value: Option<f64>, minimum: f64, maximum: f64) -> Option<f64> {
    value.filter(|value| value.is_finite() && (minimum..=maximum).contains(value))
}

pub(super) fn percentage(current: Option<f64>, full: Option<f64>) -> Option<f64> {
    let current = bounded(current, 0.0, 10_000_000.0)?;
    let full = bounded(full, 1.0, 10_000_000.0)?;
    Some((current / full * 100.0).min(100.0))
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn minutes_to_seconds(minutes: Option<f64>) -> Option<u64> {
    // 0xffff and -1 are unknown estimates, not multi-week battery life.
    bounded(minutes, 1.0, 10_080.0).map(|minutes| (minutes * 60.0).round() as u64)
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn charge_state(
    external: Option<bool>,
    charging: Option<bool>,
    full: Option<bool>,
    paused: Option<bool>,
) -> ChargeState {
    match (external, charging, full, paused) {
        (Some(false), Some(true), _, _) => ChargeState::Unknown,
        (Some(false), _, _, _) => ChargeState::Discharging,
        (Some(true), Some(true), _, _) => ChargeState::Charging,
        (Some(true), Some(false), Some(true), _) => ChargeState::Full,
        (Some(true), Some(false), _, Some(true)) => ChargeState::Paused,
        (Some(true), Some(false), _, _) => ChargeState::Connected,
        _ => ChargeState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_ratios_reject_missing_or_invalid_native_values() {
        assert_eq!(percentage(Some(0.0), Some(100.0)), Some(0.0));
        assert_eq!(percentage(Some(120.0), Some(100.0)), Some(100.0));
        assert_eq!(percentage(None, Some(100.0)), None);
        assert_eq!(percentage(Some(80.0), Some(0.0)), None);
        assert_eq!(percentage(Some(f64::NAN), Some(100.0)), None);
        assert_eq!(percentage(Some(-1.0), Some(100.0)), None);
    }

    #[test]
    fn battery_time_sentinels_do_not_become_duration_estimates() {
        for value in [
            None,
            Some(-1.0),
            Some(0.0),
            Some(65535.0),
            Some(f64::INFINITY),
        ] {
            assert_eq!(minutes_to_seconds(value), None);
        }
        assert_eq!(minutes_to_seconds(Some(90.0)), Some(5400));
    }

    #[test]
    fn a_connected_adapter_does_not_imply_charging_or_an_optimized_pause() {
        assert_eq!(
            charge_state(Some(true), Some(false), Some(false), None),
            ChargeState::Connected
        );
        assert_eq!(
            charge_state(Some(true), Some(false), Some(false), Some(true)),
            ChargeState::Paused
        );
        assert_eq!(
            charge_state(Some(true), Some(false), Some(true), Some(true)),
            ChargeState::Full
        );
        assert_eq!(
            charge_state(Some(false), Some(true), None, None),
            ChargeState::Unknown
        );
        assert_eq!(charge_state(None, None, None, None), ChargeState::Unknown);
    }
}
