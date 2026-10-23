//! Fan summaries and paired rows share one fixed-width menu-bar column.
use super::{
    display_entry::{DisplayEntry, DisplayId, FanRow},
    labels::Labels,
};
use crate::resident::preference_schema::{FanDisplayMode, ResidentPreferences};
use mangodisk_core::system_resources::{fans, metrics::MetricStatus, readings::ResourceReadings};

pub(super) fn entry(
    values: &ResourceReadings,
    labels: &Labels,
    preferences: &ResidentPreferences,
) -> DisplayEntry {
    let reading = &values.fan_speed;
    let now = values.observed_at_ms;
    let sample = fans::current_sample(reading, now);
    let digits = |rpm: Option<f64>| {
        rpm.map(|rpm| format!("{:.0}", rpm.round()))
            .unwrap_or_else(|| "—".into())
    };
    let unavailable = |id: Option<u8>| {
        if sample.is_some_and(|sample| sample.fans.is_empty()) {
            labels.text("noFans").to_string()
        } else {
            let status = if id
                .is_some_and(|id| sample.is_some_and(|s| !s.fans.iter().any(|fan| fan.id == id)))
            {
                MetricStatus::Disconnected
            } else if sample.is_some() {
                MetricStatus::Unsupported
            } else if reading.status == MetricStatus::Ready {
                MetricStatus::Stale
            } else {
                reading.status
            };
            labels.status(status).to_string()
        }
    };
    let name = labels.text(if cfg!(windows) {
        "gpuFanSpeed"
    } else {
        "fanSpeed"
    });
    let mode = if cfg!(windows) && preferences.fan_display == FanDisplayMode::Selected {
        FanDisplayMode::Highest
    } else {
        preferences.fan_display
    };
    let (marker, rpm, rows, details) = match mode {
        FanDisplayMode::Highest | FanDisplayMode::Average => {
            let average = mode == FanDisplayMode::Average;
            let value = if average {
                fans::average_rpm(reading, now)
            } else {
                fans::selected_fan(reading, now, None).and_then(|fan| fan.rpm)
            };
            let mode = labels.text(if average {
                "fanAverage"
            } else {
                "fanAutomatic"
            });
            let coverage = sample
                .filter(|sample| !sample.fans.is_empty())
                .map(|sample| {
                    labels
                        .text("fanValidReadings")
                        .replace(
                            "{valid}",
                            &sample
                                .fans
                                .iter()
                                .filter(|fan| fan.rpm.is_some())
                                .count()
                                .to_string(),
                        )
                        .replace("{total}", &sample.fans.len().to_string())
                });
            let details = format!(
                "{mode} · {}{}",
                if value.is_some() {
                    digits(value)
                } else {
                    unavailable(None)
                },
                coverage
                    .map(|coverage| format!(" · {coverage}"))
                    .unwrap_or_default()
            );
            (
                if cfg!(windows) {
                    "GFAN"
                } else if average {
                    "FAVG"
                } else {
                    "FMAX"
                }
                .into(),
                value,
                None,
                details,
            )
        }
        FanDisplayMode::Selected => {
            // Preferences validate uniqueness and bounds. Sort defensively so a
            // caller's transient order can never swap the two physical rows.
            let mut ids = preferences.fan_ids.clone();
            ids.sort_unstable();
            ids.truncate(2);
            let values: Vec<_> = ids
                .iter()
                .map(|id| fans::selected_fan(reading, now, Some(*id)).and_then(|fan| fan.rpm))
                .collect();
            let details = ids
                .iter()
                .zip(&values)
                .map(|(id, value)| {
                    let fan = labels
                        .text("fanNumber")
                        .replace("{number}", &(u16::from(*id) + 1).to_string());
                    format!(
                        "{fan} · {}",
                        if value.is_some() {
                            digits(*value)
                        } else {
                            unavailable(Some(*id))
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let rows = (ids.len() == 2).then(|| {
                [0, 1].map(|index| FanRow {
                    id: ids[index],
                    text: format!("F{} {}", u16::from(ids[index]) + 1, digits(values[index])),
                    color_value: values[index].map(|value| value.round() as u16),
                    tone: Default::default(),
                })
            });
            (
                ids.first()
                    .map(|id| format!("FAN {}", u16::from(*id) + 1))
                    .unwrap_or_else(|| "FAN".into()),
                values.first().copied().flatten(),
                rows,
                details,
            )
        }
    };
    let value = digits(rpm);
    let text = rows
        .as_ref()
        .map(|rows| format!("{name} {} · {}", rows[0].text, rows[1].text))
        .unwrap_or_else(|| format!("{name} {value}"));
    DisplayEntry {
        frequency: None,
        gpu_device: None,
        id: DisplayId::FanSpeed,
        tone: Default::default(),
        power_state: None,
        fan_rows: rows,
        color_value: rpm.map(|rpm| rpm.round() as u16),
        marker,
        digits: value,
        text,
        tooltip: format!("{name} · {details}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mangodisk_core::system_resources::metrics::MetricReading;
    use mangodisk_platform::system_resources::fans::{FanSample, FanSpeed};

    #[cfg(not(windows))]
    #[test]
    fn paired_rows_keep_identity_order_and_independent_missing_values() {
        let labels = Labels::for_locale("en-US");
        let mut values = ResourceReadings {
            observed_at_ms: 1000,
            ..Default::default()
        };
        values.fan_speed = MetricReading::ready(
            FanSample {
                fans: vec![
                    FanSpeed {
                        id: 0,
                        rpm: Some(0.0),
                    },
                    FanSpeed {
                        id: 1,
                        rpm: Some(2500.0),
                    },
                ],
            },
            1000,
        );
        let preferences = ResidentPreferences {
            fan_display: FanDisplayMode::Selected,
            fan_ids: vec![1, 0],
            ..Default::default()
        };
        let display = entry(&values, &labels, &preferences);
        assert_eq!(
            display
                .fan_rows
                .as_ref()
                .map(|rows| rows.each_ref().map(|row| row.text.as_str())),
            Some(["F1 0", "F2 2500"])
        );
        assert_eq!(
            display.fan_rows.unwrap().map(|row| row.color_value),
            [Some(0), Some(2500)]
        );
        assert!(!display.text.contains("RPM"));
        assert!(!display.tooltip.contains("RPM"));
        values.fan_speed.value.as_mut().unwrap().fans.remove(0);
        let display = entry(&values, &labels, &preferences);
        assert_eq!(
            display
                .fan_rows
                .as_ref()
                .map(|rows| rows.each_ref().map(|row| row.text.as_str())),
            Some(["F1 —", "F2 2500"])
        );
        assert!(display
            .tooltip
            .contains(labels.status(MetricStatus::Disconnected)));
        values.observed_at_ms = 6001;
        assert_eq!(
            entry(&values, &labels, &preferences)
                .fan_rows
                .as_ref()
                .map(|rows| rows.each_ref().map(|row| row.text.as_str())),
            Some(["F1 —", "F2 —"])
        );
    }

    #[test]
    fn summaries_distinguish_zero_partial_no_fans_and_stale_data() {
        let labels = Labels::for_locale("en-US");
        let mut values = ResourceReadings {
            observed_at_ms: 1000,
            ..Default::default()
        };
        values.fan_speed = MetricReading::ready(
            FanSample {
                fans: vec![
                    FanSpeed {
                        id: 0,
                        rpm: Some(0.0),
                    },
                    FanSpeed {
                        id: 1,
                        rpm: Some(2500.0),
                    },
                    FanSpeed { id: 2, rpm: None },
                ],
            },
            1000,
        );
        let mut preferences = ResidentPreferences::default();
        assert_eq!(entry(&values, &labels, &preferences).digits, "2500");
        preferences.fan_display = FanDisplayMode::Average;
        let average = entry(&values, &labels, &preferences);
        assert_eq!(average.digits, "1250");
        assert_eq!(average.marker, if cfg!(windows) { "GFAN" } else { "FAVG" });
        assert!(average.tooltip.contains("2/3"));
        values.fan_speed.value.as_mut().unwrap().fans[1].rpm = None;
        assert_eq!(entry(&values, &labels, &preferences).digits, "0");
        values.fan_speed.value.as_mut().unwrap().fans[0].rpm = None;
        assert_eq!(entry(&values, &labels, &preferences).digits, "—");
        values.fan_speed = MetricReading::ready(FanSample { fans: vec![] }, 1000);
        assert!(entry(&values, &labels, &preferences)
            .tooltip
            .contains(labels.text("noFans")));
        for status in [
            MetricStatus::Unsupported,
            MetricStatus::Failed,
            MetricStatus::Stale,
        ] {
            values.fan_speed.status = status;
            let display = entry(&values, &labels, &preferences);
            assert!(display.tooltip.contains(labels.status(status)));
            assert!(!display.tooltip.contains(labels.text("noFans")));
        }
    }
}
