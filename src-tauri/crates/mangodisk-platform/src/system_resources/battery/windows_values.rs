//! Native battery units are converted before any aggregation or presentation.
use super::{
    BatteryPower, BatteryPowerState, BatterySample, BatterySource, BatteryTemperature,
    ChargingPower,
};
use crate::{PlatformError, PlatformErrorCode, PlatformResult};

#[derive(Default)]
pub(super) struct BatteryData {
    pub temperature: Option<u32>,
    pub capabilities: u32,
    pub power_state: Option<u32>,
    pub rate: Option<i32>,
    pub current_capacity: Option<u32>,
    pub full_capacity: Option<u32>,
    pub design_capacity: Option<u32>,
    pub cycle_count: Option<u32>,
    pub voltage_mv: Option<u32>,
}

pub(super) fn sample(batteries: &[BatteryData]) -> PlatformResult<BatterySample> {
    if batteries.is_empty() {
        return Err(PlatformError::new(
            PlatformErrorCode::Unsupported,
            "battery stage=sample reason=no_system_battery",
        ));
    }
    let source = BatterySource::WindowsBattery;
    let temperature = batteries
        .iter()
        .filter_map(|battery| battery.temperature)
        .map(|raw| f64::from(raw) / 10.0 - 273.15)
        .filter(|value| value.is_finite() && *value > 0.0 && *value <= 100.0)
        .reduce(f64::max)
        .map(|celsius| BatteryTemperature { celsius, source });
    // Relative capacities use device-specific units, so their rate is not milliwatts.
    // Missing rates never become a plausible idle or zero-power observation.
    let mut signed_watts = Some(0.0);
    let mut external = Some(true);
    for battery in batteries {
        external = external
            .zip(battery.power_state)
            .map(|(external, state)| external && state & 1 != 0);
        let rate = battery.rate.filter(|rate| *rate != i32::MIN);
        let (Some(rate), Some(state)) = (rate, battery.power_state) else {
            signed_watts = None;
            continue;
        };
        if battery.capabilities & 0x4000_0000 != 0 {
            signed_watts = None;
            continue;
        }
        if (rate > 0 && state & 7 != 5)
            || (rate < 0 && state & 6 != 2)
            || state & 6 == 6
            || rate.unsigned_abs() > 300_000
        {
            return Err(PlatformError::new(PlatformErrorCode::InvalidData,
                format!("battery stage=decode_status native_state={state:#x} native_rate_mw={rate} outcome=invalid_direction_or_range")));
        }
        signed_watts = signed_watts.map(|sum| sum + f64::from(rate) / 1000.0);
    }
    let signed_watts = signed_watts.filter(|watts| watts.abs() <= 300.0);
    let battery_power = signed_watts.map(|watts| BatteryPower {
        watts: watts.abs(),
        state: if watts > 0.0 {
            BatteryPowerState::Charging
        } else if watts < 0.0 {
            BatteryPowerState::Discharging
        } else {
            BatteryPowerState::Idle
        },
        source,
    });
    let charging_power = signed_watts
        .zip(external)
        .filter(|(watts, external)| *watts <= 0.0 || *external)
        .map(|(watts, external_connected)| ChargingPower {
            watts: watts.max(0.0),
            charging: watts > 0.0,
            external_connected,
            source,
        });
    Ok(BatterySample {
        details: Some(details(batteries)),
        temperature,
        battery_power,
        charging_power,
    })
}

#[cfg(any(windows, test))]
pub(super) fn apply_system_status(
    details: &mut super::details::BatteryDetails,
    percent: u8,
    lifetime_seconds: u32,
) {
    if percent <= 100 {
        details.charge_percent = Some(f64::from(percent));
        use super::details::ChargeState;
        // The system percentage can differ from the driver's raw capacity ratio.
        if matches!(
            details.charge_state,
            ChargeState::Full | ChargeState::Connected
        ) {
            details.charge_state = if percent == 100 {
                ChargeState::Full
            } else {
                ChargeState::Connected
            };
        }
    }
    if details.charge_state == super::details::ChargeState::Discharging
        && lifetime_seconds != u32::MAX
        && lifetime_seconds > 0
    {
        details.seconds_to_empty = Some(u64::from(lifetime_seconds));
    }
}

fn details(batteries: &[BatteryData]) -> super::details::BatteryDetails {
    use super::details::{percentage, BatteryCapacity, BatteryDetails, CapacityUnit, ChargeState};
    let absolute = batteries
        .iter()
        .all(|battery| battery.capabilities & 0x4000_0000 == 0);
    let total = |read: fn(&BatteryData) -> Option<u32>, minimum: u32| -> Option<f64> {
        if !absolute {
            return None;
        }
        batteries.iter().try_fold(0.0, |sum, battery| {
            let value = read(battery).filter(|value| *value >= minimum && *value != u32::MAX)?;
            Some(sum + f64::from(value))
        })
    };
    let current = total(|battery| battery.current_capacity, 0);
    let full_charge = total(|battery| battery.full_capacity, 1);
    let design = total(|battery| battery.design_capacity, 1);
    let online = batteries.iter().try_fold(true, |online, battery| {
        Some(online && battery.power_state? & 1 != 0)
    });
    let charging = batteries
        .iter()
        .any(|battery| battery.power_state.is_some_and(|value| value & 4 != 0));
    let discharging = batteries
        .iter()
        .any(|battery| battery.power_state.is_some_and(|value| value & 2 != 0));
    let charge_percent = percentage(current, full_charge);
    let charge_state = match (online, charging, discharging) {
        (Some(true), true, false) => ChargeState::Charging,
        (_, false, true) => ChargeState::Discharging,
        (Some(true), false, false) if charge_percent == Some(100.0) => ChargeState::Full,
        (Some(true), false, false) => ChargeState::Connected,
        _ => ChargeState::Unknown,
    };
    BatteryDetails {
        charge_percent,
        external_connected: online,
        charge_state,
        health_percent: percentage(full_charge, design),
        cycle_count: (batteries.len() == 1)
            .then(|| {
                batteries[0]
                    .cycle_count
                    .filter(|value| *value != 0 && *value != u32::MAX)
            })
            .flatten(),
        capacity: absolute.then_some(BatteryCapacity {
            current,
            full_charge,
            design,
            unit: CapacityUnit::MilliwattHours,
        }),
        voltage_volts: (batteries.len() == 1)
            .then(|| {
                batteries[0]
                    .voltage_mv
                    .filter(|value| (1..=100_000).contains(value))
                    .map(|value| f64::from(value) / 1000.0)
            })
            .flatten(),
        ..BatteryDetails::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn battery(rate: i32, state: u32) -> BatteryData {
        BatteryData {
            temperature: Some(2982),
            capabilities: 0x8000_0000,
            power_state: Some(state),
            rate: Some(rate),
            ..BatteryData::default()
        }
    }
    #[test]
    fn system_percentage_and_full_state_remain_consistent() {
        use super::super::details::ChargeState;
        let mut pack = battery(0, 1);
        pack.current_capacity = Some(40_000);
        pack.full_capacity = Some(40_000);
        let mut value = sample(&[pack]).unwrap().details.unwrap();
        apply_system_status(&mut value, 99, u32::MAX);
        assert_eq!(value.charge_percent, Some(99.0));
        assert_eq!(value.charge_state, ChargeState::Connected);
    }

    #[test]
    fn absolute_capacity_is_weighted_and_unknown_packs_never_become_zero() {
        use super::super::details::{CapacityUnit, ChargeState};
        let pack = |current, full, design| BatteryData {
            current_capacity: current,
            full_capacity: Some(full),
            design_capacity: Some(design),
            power_state: Some(2),
            ..Default::default()
        };
        let value = sample(&[
            pack(Some(10_000), 20_000, 25_000),
            pack(Some(20_000), 80_000, 100_000),
        ])
        .unwrap()
        .details
        .unwrap();
        assert_eq!(value.charge_percent, Some(30.0));
        assert_eq!(value.health_percent, Some(80.0));
        assert_eq!(value.capacity.unwrap().unit, CapacityUnit::MilliwattHours);
        assert_eq!(value.charge_state, ChargeState::Discharging);
        assert_eq!(value.cycle_count, None);
        let unknown = sample(&[pack(Some(u32::MAX), 20_000, 25_000)])
            .unwrap()
            .details
            .unwrap();
        assert_eq!(unknown.charge_percent, None);
        assert_eq!(unknown.capacity.unwrap().current, None);
        let relative = BatteryData {
            capabilities: 0x4000_0000,
            current_capacity: Some(50),
            full_capacity: Some(100),
            design_capacity: Some(100),
            ..Default::default()
        };
        assert!(sample(&[relative])
            .unwrap()
            .details
            .unwrap()
            .capacity
            .is_none());
    }

    #[test]
    fn system_estimates_exclude_unknown_sentinels_and_non_discharge_states() {
        use super::super::details::{BatteryDetails, ChargeState};
        let mut value = BatteryDetails {
            charge_state: ChargeState::Discharging,
            ..Default::default()
        };
        apply_system_status(&mut value, 255, u32::MAX);
        assert_eq!(value.charge_percent, None);
        assert_eq!(value.seconds_to_empty, None);
        apply_system_status(&mut value, 0, 7200);
        assert_eq!(value.charge_percent, Some(0.0));
        assert_eq!(value.seconds_to_empty, Some(7200));
        value.charge_state = ChargeState::Charging;
        value.seconds_to_empty = None;
        apply_system_status(&mut value, 50, 100);
        assert_eq!(value.seconds_to_empty, None);
    }

    #[test]
    fn native_units_and_charge_direction_are_preserved() {
        let value = sample(&[battery(-24_000, 2)]).unwrap();
        assert!((value.temperature.unwrap().celsius - 25.05).abs() < 0.001);
        assert_eq!(value.battery_power.as_ref().unwrap().watts, 24.0);
        assert_eq!(
            value.battery_power.unwrap().state,
            BatteryPowerState::Discharging
        );
        assert_eq!(value.charging_power.unwrap().watts, 0.0);
        assert_eq!(
            sample(&[battery(50_000, 5)])
                .unwrap()
                .charging_power
                .unwrap()
                .watts,
            50.0
        );
        assert_eq!(
            sample(&[battery(0, 1)])
                .unwrap()
                .battery_power
                .unwrap()
                .state,
            BatteryPowerState::Idle
        );
    }
    #[test]
    fn unknown_relative_and_contradictory_rates_are_not_zero_watts() {
        assert!(sample(&[battery(i32::MIN, 2)])
            .unwrap()
            .battery_power
            .is_none());
        for value in [
            battery(-10_000, 4),
            battery(10_000, 2),
            battery(0, 6),
            battery(10_000, 4),
            battery(-10_000, 1),
            battery(301_000, 5),
        ] {
            assert!(sample(&[value]).is_err());
        }
        let mut relative = battery(10_000, 5);
        relative.capabilities |= 0x4000_0000;
        assert!(sample(&[relative]).unwrap().charging_power.is_none());
        let mut value = battery(10_000, 5);
        value.rate = None;
        assert!(sample(&[value]).unwrap().charging_power.is_none());
        assert!(sample(&[]).is_err());
    }
    #[test]
    fn multiple_batteries_require_complete_rates_and_keep_the_hottest_sensor() {
        let mut one = battery(-10_000, 2);
        one.temperature = Some(3032);
        let value = sample(&[one, battery(-20_000, 2)]).unwrap();
        assert_eq!(value.battery_power.unwrap().watts, 30.0);
        assert!((value.temperature.unwrap().celsius - 30.05).abs() < 0.001);
        let mut missing = battery(-20_000, 2);
        missing.rate = None;
        assert!(sample(&[battery(-10_000, 2), missing])
            .unwrap()
            .battery_power
            .is_none());
    }
}
