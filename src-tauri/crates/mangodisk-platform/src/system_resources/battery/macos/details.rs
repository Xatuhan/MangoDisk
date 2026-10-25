//! Battery status and capacity stay distinct from instantaneous rail power.
use super::Battery;
use crate::system_resources::battery::details::{
    bounded, charge_state, percentage, BatteryCapacity, BatteryDetails, CapacityUnit, ChargeState,
    PowerAdapter,
};
use core_foundation::{
    base::{CFType, TCFType},
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};

pub(super) fn read(battery: &Battery, adapter_watts: Option<f64>) -> BatteryDetails {
    let external = battery.boolean("ExternalConnected");
    let charging = battery.boolean("IsCharging");
    let full = battery.boolean("FullyCharged");
    let source = super::power_source::read().unwrap_or_default();
    let charge_state = charge_state(external, charging, full, source.optimized_charging_paused);
    let capacity = BatteryCapacity {
        current: bounded(
            raw_capacity(battery, "AppleRawCurrentCapacity", "CurrentCapacity"),
            0.0,
            1_000_000.0,
        ),
        full_charge: bounded(
            raw_capacity(battery, "AppleRawMaxCapacity", "MaxCapacity")
                .or_else(|| battery.number("NominalChargeCapacity")),
            1.0,
            1_000_000.0,
        ),
        design: bounded(battery.number("DesignCapacity"), 1.0, 1_000_000.0),
        unit: CapacityUnit::MilliampHours,
    };
    let adapter = if external == Some(true) {
        let values = battery
            .property("AdapterDetails")
            .and_then(|value| value.downcast::<CFDictionary>())
            .map(|value| unsafe {
                CFDictionary::<CFString, CFType>::wrap_under_get_rule(value.as_concrete_TypeRef())
            });
        let number = |key: &str| -> Option<f64> {
            values
                .as_ref()?
                .find(CFString::new(key))?
                .downcast::<CFNumber>()?
                .to_f64()
        };
        Some(PowerAdapter {
            input_watts: bounded(adapter_watts, 0.0, 3000.0),
            reported_watts: bounded(number("Watts"), 1.0, 3000.0),
            voltage_volts: bounded(
                number("AdapterVoltage").map(|value| value / 1000.0),
                0.1,
                100.0,
            ),
        })
    } else {
        None
    };
    BatteryDetails {
        charge_percent: source.charge_percent,
        external_connected: external,
        charge_state,
        seconds_to_empty: (charge_state == ChargeState::Discharging)
            .then_some(source.seconds_to_empty)
            .flatten(),
        seconds_to_full: (charge_state == ChargeState::Charging)
            .then_some(source.seconds_to_full)
            .flatten(),
        health_percent: percentage(capacity.full_charge, capacity.design),
        cycle_count: bounded(battery.number("CycleCount"), 0.0, 100_000.0)
            .filter(|value| value.fract() == 0.0)
            .map(|value| value as u32),
        capacity: Some(capacity),
        voltage_volts: bounded(
            battery.number("Voltage").map(|value| value / 1000.0),
            0.1,
            100.0,
        ),
        current_milliamps: bounded(
            battery
                .number("InstantAmperage")
                .or_else(|| battery.number("Amperage")),
            -100_000.0,
            100_000.0,
        ),
        adapter,
    }
}

fn raw_capacity(battery: &Battery, raw: &str, legacy: &str) -> Option<f64> {
    let value = battery.number(raw);
    // Intel reports mAh in the legacy keys. Apple Silicon uses percentages there.
    #[cfg(target_arch = "x86_64")]
    {
        value.or_else(|| battery.number(legacy))
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = legacy;
        value
    }
}
