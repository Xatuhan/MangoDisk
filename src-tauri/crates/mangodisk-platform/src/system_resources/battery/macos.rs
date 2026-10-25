use super::{
    BatteryPower, BatteryPowerState, BatterySample, BatterySource, BatteryTemperature,
    ChargingPower,
};
use crate::system_resources::macos_smc::ValueReader;
use crate::{PlatformError, PlatformErrorCode, PlatformResult};
use core_foundation::{
    base::{CFType, CFTypeRef, TCFType},
    boolean::CFBoolean,
    dictionary::CFMutableDictionaryRef,
    number::CFNumber,
    string::{CFString, CFStringRef},
};

mod details;
mod power_source;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const std::ffi::c_char) -> CFMutableDictionaryRef;
    fn IOServiceGetMatchingService(port: u32, matching: CFMutableDictionaryRef) -> u32;
    fn IOServiceGetMatchingServices(
        port: u32,
        matching: CFMutableDictionaryRef,
        iterator: *mut u32,
    ) -> i32;
    fn IOIteratorNext(iterator: u32) -> u32;
    fn IORegistryEntryCreateCFProperty(
        entry: u32,
        key: CFStringRef,
        allocator: *const std::ffi::c_void,
        options: u32,
    ) -> CFTypeRef;
    fn IOObjectRelease(object: u32) -> i32;
}
struct Battery(u32);
impl Drop for Battery {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                IOObjectRelease(self.0);
            }
        }
    }
}
impl Battery {
    fn property(&self, key: &str) -> Option<CFType> {
        let key = CFString::new(key);
        let value = unsafe {
            IORegistryEntryCreateCFProperty(self.0, key.as_concrete_TypeRef(), std::ptr::null(), 0)
        };
        (!value.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(value) })
    }
    fn number(&self, key: &str) -> Option<f64> {
        self.property(key)?.downcast::<CFNumber>()?.to_f64()
    }
    fn boolean(&self, key: &str) -> Option<bool> {
        self.property(key)?
            .downcast::<CFBoolean>()
            .map(|value| value.into())
    }
}

pub(super) fn battery_present() -> PlatformResult<bool> {
    let mut iterator = 0;
    let code = unsafe {
        IOServiceGetMatchingServices(
            0,
            IOServiceMatching(c"AppleSmartBattery".as_ptr()),
            &mut iterator,
        )
    };
    if code != 0 {
        return Err(PlatformError::new(
            PlatformErrorCode::OperationFailed,
            format!(
                "battery presence matching native_code=0x{:08x}",
                code as u32
            ),
        ));
    }
    if iterator == 0 {
        // A successful query can return no iterator when no services match.
        return Ok(false);
    }
    let iterator = Battery(iterator);
    let battery = Battery(unsafe { IOIteratorNext(iterator.0) });
    Ok(battery.0 != 0 && battery.boolean("BatteryInstalled").unwrap_or(true))
}
#[derive(Default)]
pub(super) struct Reader {
    smc: Option<ValueReader>,
    last_smc_error: Option<String>,
}
impl Reader {
    fn smc_value(&self, key: [u8; 4], failure: &mut Option<String>) -> Option<f64> {
        let result = self.smc.as_ref()?.read(key);
        match result {
            Ok(value) => value,
            Err(error) => {
                failure.get_or_insert_with(|| {
                    format!(
                        "stage=read sensor={} code={:?} error={}",
                        String::from_utf8_lossy(&key),
                        error.code(),
                        crate::diagnostics::text(&error.to_string())
                    )
                });
                None
            }
        }
    }

    fn observe_smc(&mut self, failure: Option<String>) {
        if let Some(error) = &failure {
            // Reopen once on the next scheduled sample, including after wake.
            self.smc = None;
            if self.last_smc_error.as_ref() != Some(error) {
                log::warn!("resident_battery_smc_failed {error} outcome=registry_fallback");
            }
        } else if self.last_smc_error.is_some() {
            log::info!("resident_battery_smc_recovered outcome=native_connection_ready");
        }
        self.last_smc_error = failure;
    }

    pub(super) fn read(&mut self) -> PlatformResult<BatterySample> {
        // The registry owns device lifetime; reacquiring lets reconnect and wake recover.
        let battery = Battery(unsafe {
            IOServiceGetMatchingService(0, IOServiceMatching(c"AppleSmartBattery".as_ptr()))
        });
        if battery.0 == 0 {
            return Err(PlatformError::new(
                PlatformErrorCode::Unsupported,
                "AppleSmartBattery not present",
            ));
        }
        let mut smc_failure = None;
        if self.smc.is_none() {
            match ValueReader::new() {
                Ok(smc) => self.smc = Some(smc),
                Err(error) => {
                    smc_failure = Some(format!(
                        "stage=open code={:?} error={}",
                        error.code(),
                        crate::diagnostics::text(&error.to_string())
                    ));
                }
            }
        }
        let temperatures: Vec<f64> = [*b"TB1T", *b"TB2T"]
            .into_iter()
            .filter_map(|key| self.smc_value(key, &mut smc_failure))
            .filter(|value| value.is_finite() && *value > 0.0 && *value <= 100.0)
            .collect();
        let temperature = if temperatures.is_empty() {
            battery
                .number("Temperature")
                .map(|value| value / 100.0)
                .filter(|value| value.is_finite() && *value > 0.0 && *value <= 100.0)
                .map(|celsius| BatteryTemperature {
                    celsius,
                    source: BatterySource::AppleSmartBattery,
                })
        } else {
            Some(BatteryTemperature {
                celsius: temperatures.iter().sum::<f64>() / temperatures.len() as f64,
                source: BatterySource::AppleSmc,
            })
        };
        let (charging_power, battery_power) = battery
            .boolean("IsCharging")
            .zip(battery.boolean("ExternalConnected"))
            .map(|(charging, external)| {
                let registry_power = battery
                    .number("Voltage")
                    .zip(
                        battery
                            .number("InstantAmperage")
                            .or_else(|| battery.number("Amperage")),
                    )
                    .and_then(|(voltage, current)| calculate_power(voltage, current));
                // PPBR measures discharge-rail consumption on this hardware, even while
                // the pack charges. Only signed pack current can measure net charging.
                let smc_power = if !external && !charging {
                    self.smc_value(*b"PPBR", &mut smc_failure)
                } else {
                    None
                };
                (
                    charging_power(charging, external, registry_power),
                    battery_power(charging, external, registry_power, smc_power),
                )
            })
            .unwrap_or((None, None));
        let adapter_watts = if battery.boolean("ExternalConnected") == Some(true) {
            self.smc_value(*b"PDTR", &mut smc_failure)
        } else {
            None
        };
        let details = details::read(&battery, adapter_watts);
        self.observe_smc(smc_failure);
        Ok(BatterySample {
            details: Some(details),
            temperature,
            charging_power,
            battery_power,
        })
    }
}
fn calculate_power(voltage_mv: f64, current_ma: f64) -> Option<f64> {
    (voltage_mv.is_finite()
        && (1.0..=100_000.0).contains(&voltage_mv)
        && current_ma.is_finite()
        && current_ma.abs() <= 100_000.0)
        .then_some(voltage_mv * current_ma / 1_000_000.0)
}
fn charging_power(charging: bool, external: bool, registry: Option<f64>) -> Option<ChargingPower> {
    let (watts, source) = if !charging || !external {
        // A known paused/discharging state is a real zero; missing readings are not.
        (0.0, BatterySource::AppleSmartBattery)
    } else {
        let watts = registry.filter(|value| value.is_finite() && value.abs() <= 300.0)?;
        (watts.max(0.0), BatterySource::AppleSmartBattery)
    };
    Some(ChargingPower {
        watts,
        charging: charging && external,
        external_connected: external,
        source,
    })
}
fn battery_power(
    charging: bool,
    external: bool,
    registry: Option<f64>,
    smc: Option<f64>,
) -> Option<BatteryPower> {
    // Pack current can remain zero for a minute after unplugging. PPBR updates
    // promptly on battery power; it must never substitute for charging current.
    if !charging && !external {
        if let Some(watts) =
            smc.filter(|value| value.is_finite() && *value > 0.0 && *value <= 300.0)
        {
            return Some(BatteryPower {
                watts,
                state: BatteryPowerState::Discharging,
                source: BatterySource::AppleSmc,
            });
        }
    }
    if let Some(signed) = registry.filter(|value| value.is_finite() && value.abs() <= 300.0) {
        let state = if signed < 0.0 {
            BatteryPowerState::Discharging
        } else if charging && external {
            BatteryPowerState::Charging
        } else if signed == 0.0 {
            BatteryPowerState::Idle
        } else {
            // Flags and pack current are separate observations. Do not claim charging
            // from a residual positive current after the charger has disconnected.
            return None;
        };
        return Some(BatteryPower {
            watts: signed.abs(),
            state,
            source: BatterySource::AppleSmartBattery,
        });
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a physical Mac battery and pmset"]
    fn native_charge_and_time_match_the_system_power_source() {
        let output = std::process::Command::new("/usr/bin/pmset")
            .args(["-g", "batt"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let output = String::from_utf8(output.stdout).unwrap();
        let percent = output
            .split('%')
            .next()
            .unwrap()
            .split_whitespace()
            .last()
            .unwrap()
            .parse::<f64>()
            .unwrap();
        let details = Reader::default().read().unwrap().details.unwrap();
        assert!(
            (details.charge_percent.unwrap() - percent).abs() <= 1.0,
            "system_percent={percent} sample_percent={:?}",
            details.charge_percent
        );
        if output.contains("discharging") {
            let remaining = output
                .split(';')
                .nth(2)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap();
            if let Some((hours, minutes)) = remaining.split_once(':') {
                let seconds =
                    hours.parse::<u64>().unwrap() * 3600 + minutes.parse::<u64>().unwrap() * 60;
                assert!(
                    details
                        .seconds_to_empty
                        .is_some_and(|value| value.abs_diff(seconds) <= 60),
                    "system_remaining_seconds={seconds} sample_remaining_seconds={:?}",
                    details.seconds_to_empty
                );
            }
        }
        println!("system_battery_details={details:?}");
    }
    #[test]
    #[ignore = "requires a physical Mac battery; measures native collection latency"]
    fn native_power_details_match_registry_and_fit_the_sampling_interval() {
        use std::time::Instant;
        let output = std::process::Command::new("/usr/sbin/ioreg")
            .args(["-r", "-c", "AppleSmartBattery", "-a"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let registry = plist::Value::from_reader(std::io::Cursor::new(output.stdout)).unwrap();
        let properties = registry.as_array().unwrap()[0].as_dictionary().unwrap();
        let mut reader = Reader::default();
        let mut elapsed = Vec::new();
        for _ in 0..100 {
            let start = Instant::now();
            let sample = reader.read().unwrap();
            elapsed.push(start.elapsed().as_micros());
            let details = sample.details.unwrap();
            assert_eq!(
                details.cycle_count.map(u64::from),
                properties
                    .get("CycleCount")
                    .and_then(plist::Value::as_unsigned_integer)
            );
            assert_eq!(
                details.capacity.unwrap().design.map(|value| value as u64),
                properties
                    .get("DesignCapacity")
                    .and_then(plist::Value::as_unsigned_integer)
            );
            assert!(details
                .charge_percent
                .is_some_and(|value| (0.0..=100.0).contains(&value)));
            assert!(details.seconds_to_full.is_none_or(|value| value <= 604800));
        }
        elapsed.sort_unstable();
        println!(
            "battery_native_samples=100 p50_us={} p95_us={} max_us={}",
            elapsed[50], elapsed[95], elapsed[99]
        );
        assert!(
            elapsed[95] < 200_000,
            "collection must leave most of the two-second interval idle"
        );
    }

    #[test]
    #[ignore = "requires macOS IOKit and ioreg"]
    fn native_battery_presence_matches_an_independent_registry_query() {
        let output = std::process::Command::new("/usr/sbin/ioreg")
            .args(["-r", "-c", "AppleSmartBattery", "-a"])
            .output()
            .expect("ioreg must be available for this hardware test");
        assert!(output.status.success());
        // ioreg emits no plist at all when no services match.
        let expected = if output.stdout.is_empty() {
            false
        } else {
            let registry = plist::Value::from_reader(std::io::Cursor::new(output.stdout)).unwrap();
            let services = registry.as_array().expect("ioreg must return an array");
            services.first().is_some_and(|service| {
                service
                    .as_dictionary()
                    .and_then(|properties| properties.get("BatteryInstalled"))
                    .and_then(plist::Value::as_boolean)
                    .unwrap_or(true)
            })
        };
        assert_eq!(battery_present().unwrap(), expected);
    }
    #[test]
    #[ignore = "requires a Mac with AppleSmartBattery and readable SMC sensors"]
    fn native_battery_smc_recovers_after_an_initial_open_failure() {
        let smc = ValueReader::new().expect("native SMC must be available for this hardware test");
        assert!(smc.read(*b"TB1T").unwrap().is_some());
        drop(smc);
        // This is the reader state left by a transient SMC open failure.
        let mut reader = Reader::default();
        reader.observe_smc(Some("stage=open native_code=test_failure".into()));
        let sample = reader.read().unwrap();
        assert_eq!(sample.temperature.unwrap().source, BatterySource::AppleSmc);
        assert!(reader.smc.is_some());
        assert!(reader.last_smc_error.is_none());
        // Exercise the same invalidation path used for failed native queries.
        reader.observe_smc(Some(
            "stage=read sensor=TB1T native_code=test_failure".into(),
        ));
        assert!(reader.smc.is_none());
        let recovered = reader.read().unwrap();
        assert_eq!(
            recovered.temperature.unwrap().source,
            BatterySource::AppleSmc
        );
        assert!(reader.last_smc_error.is_none());
    }
    #[test]
    fn discharge_prefers_live_smc_over_current_left_zero_after_unplug() {
        let power = battery_power(false, false, Some(0.0), Some(27.0)).unwrap();
        assert_eq!(power.state, BatteryPowerState::Discharging);
        assert_eq!(power.watts, 27.0);
        assert_eq!(power.source, BatterySource::AppleSmc);
    }
    #[test]
    fn charging_uses_pack_voltage_and_current_instead_of_ppbr_discharge_rail() {
        let power = charging_power(true, true, Some(75.0)).unwrap();
        assert_eq!(power.watts, 75.0);
        assert_eq!(power.source, BatterySource::AppleSmartBattery);
        assert_eq!(
            battery_power(true, true, Some(75.0), Some(0.35))
                .unwrap()
                .watts,
            75.0
        );
        assert!(battery_power(true, true, None, Some(0.35)).is_none());
    }
    #[test]
    fn charge_power_never_uses_discharge_or_missing_values_as_positive_charge() {
        assert_eq!(calculate_power(12_000.0, 2_000.0), Some(24.0));
        assert_eq!(
            charging_power(false, false, Some(-24.0)).unwrap().watts,
            0.0
        );
        assert_eq!(charging_power(false, true, None).unwrap().watts, 0.0);
        assert_eq!(charging_power(true, true, Some(24.0)).unwrap().watts, 24.0);
        assert_eq!(charging_power(true, true, Some(-24.0)).unwrap().watts, 0.0);
        assert!(charging_power(true, true, None).is_none());
        assert!(charging_power(true, true, Some(f64::NAN)).is_none());
        assert!(calculate_power(0.0, 2_000.0).is_none());
    }
    #[test]
    fn battery_power_distinguishes_charge_discharge_idle_and_unavailable() {
        let discharge = battery_power(false, false, Some(-24.0), Some(25.0)).unwrap();
        assert_eq!(discharge.watts, 25.0);
        assert_eq!(discharge.state, BatteryPowerState::Discharging);
        assert_eq!(
            battery_power(false, false, Some(-24.0), Some(f64::NAN))
                .unwrap()
                .watts,
            24.0
        );
        assert_eq!(
            battery_power(false, false, Some(-24.0), Some(0.0))
                .unwrap()
                .watts,
            24.0
        );
        assert_eq!(
            battery_power(false, true, Some(0.0), None).unwrap().state,
            BatteryPowerState::Idle
        );
        assert_eq!(
            battery_power(true, true, Some(75.0), None).unwrap().state,
            BatteryPowerState::Charging
        );
        assert_eq!(
            battery_power(false, false, None, Some(25.0)).unwrap().state,
            BatteryPowerState::Discharging
        );
        assert!(battery_power(false, false, Some(75.0), None).is_none());
        assert!(battery_power(false, true, None, Some(25.0)).is_none());
        assert!(battery_power(false, false, Some(f64::NAN), None).is_none());
    }
}
