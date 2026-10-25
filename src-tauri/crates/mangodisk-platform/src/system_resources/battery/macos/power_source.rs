//! System-calibrated charge and time estimates, separate from raw pack capacities.
use crate::system_resources::battery::details::{minutes_to_seconds, percentage};
use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFType, CFTypeRef, TCFType},
    boolean::CFBoolean,
    dictionary::{CFDictionary, CFDictionaryRef},
    number::CFNumber,
    string::CFString,
};

#[derive(Default)]
pub(super) struct PowerSource {
    pub(super) charge_percent: Option<f64>,
    pub(super) seconds_to_empty: Option<u64>,
    pub(super) seconds_to_full: Option<u64>,
    pub(super) optimized_charging_paused: Option<bool>,
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSCopyPowerSourcesList(info: CFTypeRef) -> CFArrayRef;
    fn IOPSGetPowerSourceDescription(info: CFTypeRef, source: CFTypeRef) -> CFDictionaryRef;
}

pub(super) fn read() -> Option<PowerSource> {
    // Copy functions transfer ownership; descriptions borrow from the retained info.
    let raw = unsafe { IOPSCopyPowerSourcesInfo() };
    if raw.is_null() {
        return None;
    }
    let info = unsafe { CFType::wrap_under_create_rule(raw) };
    let raw = unsafe { IOPSCopyPowerSourcesList(info.as_CFTypeRef()) };
    if raw.is_null() {
        return None;
    }
    let list = unsafe { CFArray::<CFType>::wrap_under_create_rule(raw) };
    for source in list.iter() {
        let raw =
            unsafe { IOPSGetPowerSourceDescription(info.as_CFTypeRef(), source.as_CFTypeRef()) };
        if raw.is_null() {
            continue;
        }
        let description = unsafe { CFDictionary::<CFString, CFType>::wrap_under_get_rule(raw) };
        if let Some(source) = parse(&description) {
            return Some(source);
        }
    }
    None
}

fn parse(description: &CFDictionary<CFString, CFType>) -> Option<PowerSource> {
    let kind = description
        .find(CFString::new("Type"))
        .and_then(|value| value.downcast::<CFString>());
    if kind.is_none_or(|kind| kind != "InternalBattery") {
        return None;
    }
    let boolean = |key: &str| {
        let value = description.find(CFString::new(key))?;
        value.downcast::<CFBoolean>().map(bool::from).or_else(|| {
            value
                .downcast::<CFNumber>()?
                .to_i64()
                .map(|value| value == 1)
        })
    };
    if boolean("Is Present") == Some(false) {
        return None;
    }
    let number = |key: &str| {
        description
            .find(CFString::new(key))?
            .downcast::<CFNumber>()?
            .to_f64()
    };
    Some(PowerSource {
        // Raw registry capacity ratios differ from macOS calibration on Intel Macs.
        charge_percent: percentage(number("Current Capacity"), number("Max Capacity")),
        seconds_to_empty: minutes_to_seconds(number("Time to Empty")),
        seconds_to_full: minutes_to_seconds(number("Time to Full Charge")),
        optimized_charging_paused: boolean("Optimized Battery Charging Engaged"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn description(values: &[(&str, CFType)]) -> CFDictionary<CFString, CFType> {
        CFDictionary::from_CFType_pairs(
            &values
                .iter()
                .map(|(key, value)| (CFString::new(key), value.clone()))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn system_calibration_is_independent_of_raw_pack_capacity() {
        let values = description(&[
            ("Type", CFString::new("InternalBattery").as_CFType()),
            ("Current Capacity", CFNumber::from(96).as_CFType()),
            ("Max Capacity", CFNumber::from(100).as_CFType()),
            ("Time to Empty", CFNumber::from(249).as_CFType()),
            ("Time to Full Charge", CFNumber::from(-1).as_CFType()),
        ]);
        let sample = parse(&values).unwrap();
        assert_eq!(sample.charge_percent, Some(96.0));
        assert_eq!(sample.seconds_to_empty, Some(14_940));
        assert_eq!(sample.seconds_to_full, None);
        assert_eq!(sample.optimized_charging_paused, None);
    }

    #[test]
    fn absent_batteries_and_external_power_sources_are_excluded() {
        assert!(parse(&description(&[])).is_none());
        assert!(parse(&description(&[("Type", CFString::new("UPS").as_CFType())])).is_none());
        assert!(parse(&description(&[
            ("Type", CFString::new("InternalBattery").as_CFType()),
            ("Is Present", CFBoolean::false_value().as_CFType()),
        ]))
        .is_none());
    }

    #[test]
    fn unknown_system_values_do_not_become_charge_or_duration() {
        let values = description(&[
            ("Type", CFString::new("InternalBattery").as_CFType()),
            ("Current Capacity", CFNumber::from(96).as_CFType()),
            ("Max Capacity", CFNumber::from(0).as_CFType()),
            ("Time to Empty", CFNumber::from(-1).as_CFType()),
            ("Time to Full Charge", CFNumber::from(65535).as_CFType()),
            (
                "Optimized Battery Charging Engaged",
                CFBoolean::true_value().as_CFType(),
            ),
        ]);
        let sample = parse(&values).unwrap();
        assert_eq!(sample.charge_percent, None);
        assert_eq!(sample.seconds_to_empty, None);
        assert_eq!(sample.seconds_to_full, None);
        assert_eq!(sample.optimized_charging_paused, Some(true));
    }
}
