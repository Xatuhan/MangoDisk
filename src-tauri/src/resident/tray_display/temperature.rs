//! Freshness and source-aware temperature presentation for native surfaces.
use super::{
    display_entry::{DisplayEntry, DisplayId},
    labels,
};
use mangodisk_core::system_resources::{metrics::MetricStatus, readings::ResourceReadings};

pub(super) fn temperature_entry(
    values: &ResourceReadings,
    labels: &labels::Labels,
) -> DisplayEntry {
    use mangodisk_platform::system_resources::cpu::temperature::TEMPERATURE_FRESHNESS_MS;
    let reading = &values.cpu_temperature;
    let value = reading.value.as_ref().filter(|value| {
        reading.status == MetricStatus::Ready
            && reading.sampled_at_ms.is_some_and(|sampled| {
                sampled <= values.observed_at_ms
                    && values.observed_at_ms - sampled <= TEMPERATURE_FRESHNESS_MS
            })
            && value.celsius.is_finite()
            && value.celsius > 0.0
            && value.celsius <= 150.0
            && value.sensor_count > 0
    });
    let digits = value
        // Match the panel's positive half-degree rounding instead of Rust's
        // formatting rule, which rounds ties to the nearest even integer.
        .map(|value| format!("{:.0}", value.celsius.round()))
        .unwrap_or_else(|| "—".into());
    let tooltip = if value.is_some() {
        format!("{} {digits}°C", labels.temperature("label"))
    } else {
        let status = if reading.status == MetricStatus::Ready {
            MetricStatus::Stale
        } else {
            reading.status
        };
        format!(
            "{} · {}",
            labels.temperature("label"),
            labels.status(status)
        )
    };
    DisplayEntry {
        frequency: None,
        gpu_device: None,
        id: DisplayId::CpuTemperature,
        tone: Default::default(),
        power_state: None,
        fan_rows: None,
        // Rules are selected by metric ID, so Celsius keeps its own thresholds.
        color_value: value.map(|value| value.celsius.round() as u16),
        marker: "°C".into(),
        text: if value.is_some() {
            format!("CPU {digits}°C")
        } else {
            "CPU —".into()
        },
        digits,
        tooltip,
    }
}

pub(super) fn gpu_temperature_entry(
    values: &ResourceReadings,
    labels: &labels::Labels,
    selected: Option<&str>,
) -> DisplayEntry {
    use mangodisk_platform::system_resources::gpu::temperature::GPU_TEMPERATURE_FRESHNESS_MS;
    let reading = &values.gpu_temperature;
    let mismatched = reading
        .value
        .as_ref()
        .is_some_and(|value| selected.is_some_and(|id| id != value.adapter_id));
    let value = reading.value.as_ref().filter(|value| {
        !mismatched
            && reading.status == MetricStatus::Ready
            && reading.sampled_at_ms.is_some_and(|sampled| {
                sampled <= values.observed_at_ms
                    && values.observed_at_ms - sampled <= GPU_TEMPERATURE_FRESHNESS_MS
            })
            && value.valid()
    });
    let digits = value
        .map(|v| format!("{:.0}", v.celsius.round()))
        .unwrap_or_else(|| "—".into());
    let tooltip = if let Some(value) = value {
        format!(
            "{} {digits}°C · {}",
            labels.gpu_temperature("label"),
            value.adapter_name
        )
    } else {
        format!(
            "{} · {}",
            labels.gpu_temperature("label"),
            labels.status(if mismatched {
                MetricStatus::Loading
            } else if reading.status == MetricStatus::Ready {
                MetricStatus::Stale
            } else {
                reading.status
            })
        )
    };
    DisplayEntry {
        frequency: None,
        gpu_device: None,
        id: DisplayId::GpuTemperature,
        tone: Default::default(),
        power_state: None,
        fan_rows: None,
        color_value: value.map(|value| value.celsius.round() as u16),
        marker: "°C".into(),
        text: if value.is_some() {
            format!("GPU {digits}°C")
        } else {
            "GPU —".into()
        },
        digits,
        tooltip,
    }
}

pub(super) fn disk_temperature_entry(
    values: &ResourceReadings,
    labels: &labels::Labels,
    manual: Option<&str>,
    device: Option<&str>,
) -> DisplayEntry {
    use mangodisk_platform::system_resources::disk_temperature::DISK_TEMPERATURE_FRESHNESS_MS;
    let reading = &values.disk_temperature;
    let selected =
        mangodisk_core::system_resources::disk::temperature_volume(&values.volumes, manual, device)
            .map(|volume| volume.id.as_str());
    let value = reading.value.as_ref().filter(|value| {
        reading.status == MetricStatus::Ready
            && selected == Some(value.volume_id.as_str())
            && value.valid()
            && reading.sampled_at_ms.is_some_and(|sampled| {
                sampled <= values.observed_at_ms
                    && values.observed_at_ms - sampled <= DISK_TEMPERATURE_FRESHNESS_MS
            })
    });
    let digits = value
        .map(|value| format!("{:.0}", value.celsius.round()))
        .unwrap_or_else(|| "—".into());
    let tooltip = if let Some(value) = value {
        format!(
            "{} {digits}°C · {}",
            labels.text("diskTemperature"),
            value.volume_name
        )
    } else {
        format!(
            "{} · {}",
            labels.text("diskTemperature"),
            labels.status(
                if reading.status == MetricStatus::Ready
                    && reading
                        .value
                        .as_ref()
                        .is_some_and(|value| selected != Some(value.volume_id.as_str()))
                {
                    MetricStatus::Loading
                } else if reading.status == MetricStatus::Ready {
                    MetricStatus::Stale
                } else {
                    reading.status
                }
            )
        )
    };
    DisplayEntry {
        frequency: None,
        gpu_device: None,
        id: DisplayId::DiskTemperature,
        tone: Default::default(),
        power_state: None,
        fan_rows: None,
        color_value: value.map(|value| value.celsius.round() as u16),
        marker: "°C".into(),
        text: if value.is_some() {
            format!("DISK {digits}°C")
        } else {
            "DISK —".into()
        },
        digits,
        tooltip,
    }
}
