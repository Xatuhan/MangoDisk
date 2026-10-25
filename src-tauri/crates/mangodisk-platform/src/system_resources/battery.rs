//! Read-only battery telemetry; charging power is net power entering the battery.
use crate::PlatformResult;
use serde::Serialize;

pub mod details;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;
#[cfg(any(windows, test))]
mod windows_values;
pub const BATTERY_FRESHNESS_MS: u64 = 5_000;

#[derive(Default)]
pub struct BatteryPresenceReader {
    #[cfg(any(target_os = "macos", windows))]
    checked_at: Option<std::time::Instant>,
    #[cfg(any(target_os = "macos", windows, test))]
    present: Option<bool>,
    #[cfg(any(target_os = "macos", windows, test))]
    last_outcome: Option<String>,
}

impl BatteryPresenceReader {
    pub fn read(&mut self) -> Option<bool> {
        #[cfg(any(target_os = "macos", windows))]
        {
            if self
                .checked_at
                .is_none_or(|at| at.elapsed().as_secs() >= 30)
            {
                self.checked_at = Some(std::time::Instant::now());
                #[cfg(target_os = "macos")]
                self.observe(macos::battery_present());
                #[cfg(windows)]
                self.observe(windows::battery_present());
            }
            self.present
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            None
        }
    }

    #[cfg(any(target_os = "macos", windows, test))]
    fn observe(&mut self, result: PlatformResult<bool>) {
        let (outcome, failed) = match result {
            Ok(present) => {
                self.present = Some(present);
                (format!("present={present}"), false)
            }
            // A failed registry query cannot prove that a physical battery is absent.
            Err(error) => (
                format!(
                    "code={:?} error={}",
                    error.code(),
                    crate::diagnostics::text(&error.to_string())
                ),
                true,
            ),
        };
        if self.last_outcome.as_ref() != Some(&outcome) {
            if failed {
                log::warn!("resident_battery_presence {outcome}");
            } else {
                log::info!("resident_battery_presence {outcome}");
            }
        }
        self.last_outcome = Some(outcome);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BatterySource {
    AppleSmc,
    AppleSmartBattery,
    WindowsBattery,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryTemperature {
    pub celsius: f64,
    pub source: BatterySource,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChargingPower {
    pub watts: f64,
    pub charging: bool,
    pub external_connected: bool,
    pub source: BatterySource,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BatteryPowerState {
    Charging,
    Discharging,
    Idle,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryPower {
    pub watts: f64,
    pub state: BatteryPowerState,
    pub source: BatterySource,
}
pub struct BatterySample {
    pub details: Option<details::BatteryDetails>,
    pub temperature: Option<BatteryTemperature>,
    pub charging_power: Option<ChargingPower>,
    pub battery_power: Option<BatteryPower>,
}
#[derive(Default)]
pub struct BatteryReader {
    #[cfg(target_os = "macos")]
    reader: macos::Reader,
    #[cfg(windows)]
    reader: windows::Reader,
    last_outcome: Option<String>,
    samples: u64,
}
impl BatteryReader {
    pub fn read(&mut self) -> PlatformResult<BatterySample> {
        #[cfg(any(target_os = "macos", windows))]
        let result = self.reader.read();
        #[cfg(not(any(target_os = "macos", windows)))]
        let result: PlatformResult<BatterySample> = Err(crate::PlatformError::new(
            crate::PlatformErrorCode::Unsupported,
            "battery telemetry unavailable",
        ));
        self.samples = self.samples.wrapping_add(1);
        let outcome = match &result {
            Ok(sample) => format!(
                "temperature_source={:?} power_source={:?} charging={:?} external_connected={:?} battery_power_source={:?} battery_state={:?}",
                sample.temperature.as_ref().map(|v| v.source),
                sample.charging_power.as_ref().map(|v| v.source),
                sample.charging_power.as_ref().map(|v| v.charging),
                sample.charging_power.as_ref().map(|v| v.external_connected),
                sample.battery_power.as_ref().map(|v| v.source),
                sample.battery_power.as_ref().map(|v| v.state)
            ),
            Err(error) => format!(
                "code={:?} error={}",
                error.code(),
                crate::diagnostics::text(&error.to_string())
            ),
        };
        if self.last_outcome.as_ref() != Some(&outcome) || self.samples.is_multiple_of(30) {
            match &result {
                Ok(sample) => log::info!(
                    "resident_battery_observation {outcome} celsius={:?} watts={:?} battery_watts={:?}",
                    sample.temperature.as_ref().map(|v| v.celsius),
                    sample.charging_power.as_ref().map(|v| v.watts),
                    sample.battery_power.as_ref().map(|v| v.watts)
                ),
                Err(_) => log::warn!("resident_battery_observation {outcome}"),
            }
        }
        self.last_outcome = Some(outcome);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn non_macos_battery_readers_report_unavailable_without_a_presence_claim() {
        assert_eq!(BatteryPresenceReader::default().read(), None);
        let Err(error) = BatteryReader::default().read() else {
            panic!("battery telemetry must be unavailable outside macOS");
        };
        assert_eq!(error.code(), crate::PlatformErrorCode::Unsupported);
    }

    #[test]
    fn presence_errors_never_mean_absent_and_recovery_updates_known_state() {
        let mut reader = BatteryPresenceReader::default();
        let failure = || {
            Err(crate::PlatformError::new(
                crate::PlatformErrorCode::OperationFailed,
                "registry test failure",
            ))
        };
        reader.observe(failure());
        assert_eq!(reader.present, None);
        reader.observe(Ok(true));
        reader.observe(failure());
        assert_eq!(reader.present, Some(true));
        reader.observe(Ok(false));
        reader.observe(failure());
        assert_eq!(reader.present, Some(false));
        reader.observe(Ok(true));
        assert_eq!(reader.present, Some(true));
    }
}
