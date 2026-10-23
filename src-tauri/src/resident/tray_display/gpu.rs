//! Project all-device displays from the same observations used by single-device views.
use super::{
    display_entry::{DisplayEntry, DisplayId, GpuDisplayDevice},
    labels::Labels,
};
use crate::resident::preference_schema::GpuDisplaySelection;
use mangodisk_core::system_resources::{
    metrics::{MetricId, MetricStatus},
    readings::{GpuDeviceReading, ResourceReadings},
};

pub(super) fn entries(
    values: &ResourceReadings,
    labels: &Labels,
    temperature: bool,
    selection: &GpuDisplaySelection,
) -> Vec<DisplayEntry> {
    let selected = match selection {
        GpuDisplaySelection::Selected { adapter_ids } => Some(adapter_ids),
        GpuDisplaySelection::Automatic => None,
    };
    let automatic = values
        .gpu_devices
        .iter()
        .filter(|device| {
            device.usage.status == MetricStatus::Ready
                && device.usage.sampled_at_ms.is_some_and(|at| {
                    at <= values.observed_at_ms
                        && values.observed_at_ms - at <= MetricId::Gpu.freshness_ms()
                })
                && device.usage.value.as_ref().is_some_and(|value| {
                    value.adapter_id == device.adapter_id
                        && value.used_percent.is_finite()
                        && (0.0..=100.0).contains(&value.used_percent)
                })
        })
        .reduce(|current, candidate| {
            if candidate.usage.value.as_ref().unwrap().used_percent
                > current.usage.value.as_ref().unwrap().used_percent
            {
                candidate
            } else {
                current
            }
        })
        .or_else(|| values.gpu_devices.first());
    let mut entries = values
        .gpu_devices
        .iter()
        .filter(|device| match selected {
            Some(ids) => ids.contains(&device.adapter_id),
            None => automatic.is_some_and(|chosen| chosen.adapter_id == device.adapter_id),
        })
        .map(|device| {
            let label = values
                .gpu_adapters
                .iter()
                .position(|adapter| adapter.id == device.adapter_id)
                .map(|index| {
                    if values.gpu_adapters.len() == 1 {
                        "GPU".into()
                    } else {
                        format!("GPU{index}")
                    }
                })
                .unwrap_or_else(|| "GPU?".into());
            entry(device, label, values.observed_at_ms, labels, temperature)
        })
        .collect::<Vec<_>>();
    if let Some(ids) = selected {
        for id in ids.iter().filter(|id| {
            !values
                .gpu_devices
                .iter()
                .any(|device| device.adapter_id == **id)
        }) {
            let device = GpuDeviceReading {
                adapter_id: id.clone(),
                adapter_name: labels.text("savedDisconnected").into(),
                usage: mangodisk_core::system_resources::metrics::MetricReading {
                    status: MetricStatus::Disconnected,
                    value: None,
                    sampled_at_ms: None,
                },
                temperature: mangodisk_core::system_resources::metrics::MetricReading {
                    status: MetricStatus::Disconnected,
                    value: None,
                    sampled_at_ms: None,
                },
                usage_history: Default::default(),
                temperature_history: Default::default(),
            };
            let missing = entry(
                &device,
                "GPU?".into(),
                values.observed_at_ms,
                labels,
                temperature,
            );
            entries.push(missing);
        }
    }
    entries
}

fn entry(
    device: &GpuDeviceReading,
    label: String,
    now: u64,
    labels: &Labels,
    temperature: bool,
) -> DisplayEntry {
    let usage = device.usage.value.as_ref().filter(|value| {
        value.adapter_id == device.adapter_id
            && value.used_percent.is_finite()
            && (0.0..=100.0).contains(&value.used_percent)
    });
    let sensor = device
        .temperature
        .value
        .as_ref()
        .filter(|value| value.adapter_id == device.adapter_id && value.valid());
    let (status, sampled_at) = if temperature {
        (device.temperature.status, device.temperature.sampled_at_ms)
    } else {
        (device.usage.status, device.usage.sampled_at_ms)
    };
    let value = if temperature {
        sensor.map(|value| value.celsius)
    } else {
        usage.map(|value| value.used_percent)
    };
    let native_name = if temperature {
        sensor.map(|value| &value.adapter_name)
    } else {
        usage.map(|value| &value.adapter_name)
    };
    let lifetime = if temperature {
        mangodisk_platform::system_resources::gpu::temperature::GPU_TEMPERATURE_FRESHNESS_MS
    } else {
        MetricId::Gpu.freshness_ms()
    };
    let fresh = sampled_at.is_some_and(|sampled| sampled <= now && now - sampled <= lifetime);
    let value = value.filter(|_| status == MetricStatus::Ready && fresh);
    let digits = value
        .map(|value| format!("{:.0}", value.round()))
        .unwrap_or_else(|| "—".into());
    let unit = if temperature { "°C" } else { "%" };
    let name = if temperature {
        labels.gpu_temperature("label")
    } else {
        labels.text("gpuUsage")
    };
    let tooltip = if value.is_some() {
        format!("{} · {name} {digits}{unit}", device.adapter_name)
    } else {
        let status = if status == MetricStatus::Ready {
            MetricStatus::Stale
        } else {
            status
        };
        format!(
            "{} · {name} · {}",
            device.adapter_name,
            labels.status(status)
        )
    };
    DisplayEntry {
        frequency: None,
        gpu_device: Some(GpuDisplayDevice {
            id: device.adapter_id.clone(),
            name: native_name.unwrap_or(&device.adapter_name).clone(),
            label: label.clone(),
        }),
        id: if temperature {
            DisplayId::GpuTemperature
        } else {
            DisplayId::Gpu
        },
        tone: Default::default(),
        power_state: None,
        fan_rows: None,
        color_value: value.map(|value| value.round() as u16),
        marker: if temperature {
            "°C".into()
        } else {
            "G".into()
        },
        text: format!(
            "{label} {digits}{}",
            if value.is_some() { unit } else { "" }
        ),
        tooltip,
        digits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mangodisk_platform::system_resources::gpu::GpuAdapter;

    #[test]
    fn display_labels_follow_device_count_and_catalogue_positions() {
        let adapters = [
            ("a:0", "GPU0 · Linked GPU"),
            ("a:1", "GPU1 · Linked GPU"),
            ("z:0", "GPU2 · Discrete GPU"),
        ];
        let mut values = ResourceReadings {
            gpu_adapters: adapters
                .iter()
                .map(|(id, name)| GpuAdapter {
                    id: (*id).into(),
                    name: (*name).into(),
                })
                .collect(),
            gpu_devices: adapters
                .iter()
                .map(|(id, name)| GpuDeviceReading {
                    adapter_id: (*id).into(),
                    adapter_name: (*name).into(),
                    usage: Default::default(),
                    usage_history: Default::default(),
                    temperature: Default::default(),
                    temperature_history: Default::default(),
                })
                .collect(),
            ..Default::default()
        };
        let selection = GpuDisplaySelection::Selected {
            adapter_ids: adapters.iter().map(|(id, _)| (*id).into()).collect(),
        };
        let labels = Labels::for_locale("en-US");
        for temperature in [false, true] {
            let entries = entries(&values, &labels, temperature, &selection);
            let actual: Vec<_> = entries
                .iter()
                .map(|entry| entry.gpu_device.as_ref().unwrap().label.as_str())
                .collect();
            assert_eq!(actual, ["GPU0", "GPU1", "GPU2"]);
            let selection = GpuDisplaySelection::Selected {
                adapter_ids: vec!["z:0".into(), "a:1".into()],
            };
            let selected = super::entries(&values, &labels, temperature, &selection);
            let actual: Vec<_> = selected
                .iter()
                .map(|entry| {
                    let device = entry.gpu_device.as_ref().unwrap();
                    (device.id.as_str(), device.label.as_str())
                })
                .collect();
            assert_eq!(actual, [("a:1", "GPU1"), ("z:0", "GPU2")]);
            let single_selected = super::entries(
                &values,
                &labels,
                temperature,
                &GpuDisplaySelection::Selected {
                    adapter_ids: vec!["z:0".into()],
                },
            );
            assert_eq!(
                single_selected[0].gpu_device.as_ref().unwrap().label,
                "GPU2"
            );
        }
        values.gpu_adapters.truncate(1);
        values.gpu_devices.truncate(1);
        for temperature in [false, true] {
            for selection in [
                GpuDisplaySelection::Automatic,
                GpuDisplaySelection::Selected {
                    adapter_ids: vec!["a:0".into()],
                },
            ] {
                let single = entries(&values, &labels, temperature, &selection);
                assert_eq!(single.len(), 1);
                assert_eq!(single[0].gpu_device.as_ref().unwrap().label, "GPU");
            }
        }
    }
}
