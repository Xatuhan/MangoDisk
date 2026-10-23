//! Validated font measurements; live readings never determine window geometry.
use super::super::tray_display::display_entry::DisplayId;
use crate::resident::preference_schema::FloatingFontSize;
#[cfg(any(windows, target_os = "macos", target_os = "linux", test))]
use crate::resident::preference_schema::FloatingPreferences;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ItemMeasurement {
    pub id: DisplayId,
    pub gpu_device_id: Option<String>,
    pub label_width: f64,
    pub value_width: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayoutMeasurement {
    pub font_size: FloatingFontSize,
    pub bold: bool,
    pub items: Option<Vec<ItemMeasurement>>,
}

impl LayoutMeasurement {
    pub fn valid(&self) -> bool {
        self.items
            .as_ref()
            .is_none_or(|items| valid_measurements(items))
    }

    #[cfg(any(windows, target_os = "macos", target_os = "linux", test))]
    pub(super) fn matches(
        &self,
        preferences: &FloatingPreferences,
        entries: &[super::DisplayEntry],
    ) -> bool {
        self.font_size == preferences.font_size
            && self.bold == preferences.bold
            && self
                .items
                .as_ref()
                .is_none_or(|items| matches(items, entries))
    }
}

impl ItemMeasurement {
    #[cfg(any(windows, target_os = "macos", target_os = "linux", test))]
    pub(super) fn width(&self) -> f64 {
        // Match the metric's horizontal padding and label gap.
        super::geometry::item_width(self.label_width, self.value_width)
    }

    fn valid(&self) -> bool {
        self.id != DisplayId::App
            && self
                .gpu_device_id
                .as_ref()
                .is_none_or(|id| !id.is_empty() && id.len() <= 1024 && !id.contains('\0'))
            && [
                (self.label_width, 1.0, 240.0),
                (self.value_width, 1.0, 120.0),
            ]
            .into_iter()
            .all(|(width, min, max)| width.is_finite() && (min..=max).contains(&width))
    }
}

fn valid_measurements(items: &[ItemMeasurement]) -> bool {
    items.len() <= 128 && items.iter().all(ItemMeasurement::valid)
}

#[cfg(any(windows, target_os = "macos", target_os = "linux", test))]
pub(super) fn matches(items: &[ItemMeasurement], entries: &[super::DisplayEntry]) -> bool {
    valid_measurements(items)
        && items.len() == entries.len()
        && items.iter().zip(entries).all(|(item, entry)| {
            item.id == entry.id
                && item.gpu_device_id.as_deref()
                    == entry.gpu_device.as_ref().map(|device| device.id.as_str())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> ItemMeasurement {
        ItemMeasurement {
            id: DisplayId::Cpu,
            gpu_device_id: None,
            label_width: 21.0,
            value_width: 35.0,
        }
    }

    #[test]
    fn stale_typography_cannot_apply_to_a_new_font_or_bold_setting() {
        let mut preferences = FloatingPreferences::default();
        let request = LayoutMeasurement {
            font_size: preferences.font_size,
            bold: false,
            items: None,
        };
        assert!(request.valid());
        assert!(request.matches(&preferences, &[]));
        preferences.bold = true;
        assert!(!request.matches(&preferences, &[]));
        preferences.bold = false;
        preferences.font_size = FloatingFontSize::ExtraLarge;
        assert!(!request.matches(&preferences, &[]));
    }

    #[test]
    fn bounds_reject_nonfinite_and_oversized_measurements() {
        let mut value = item();
        assert!(value.valid());
        assert_eq!(value.width(), 65.0);
        for width in [f64::NAN, f64::INFINITY, -1.0, 241.0] {
            value.label_width = width;
            assert!(!value.valid());
        }
    }

    #[test]
    fn stale_metric_or_gpu_identity_cannot_resize_a_different_snapshot() {
        let entry = super::super::DisplayEntry {
            id: DisplayId::Cpu,
            frequency: None,
            gpu_device: None,
            tone: Default::default(),
            power_state: None,
            fan_rows: None,
            color_value: None,
            marker: "C".into(),
            digits: "12".into(),
            text: "CPU 12%".into(),
            tooltip: "CPU 12%".into(),
        };
        let mut value = item();
        assert!(matches(
            std::slice::from_ref(&value),
            std::slice::from_ref(&entry)
        ));
        assert!(!matches(&[], std::slice::from_ref(&entry)));
        value.id = DisplayId::Gpu;
        assert!(!matches(
            std::slice::from_ref(&value),
            std::slice::from_ref(&entry)
        ));
        value.id = DisplayId::Cpu;
        value.gpu_device_id = Some("another-device".into());
        assert!(!matches(&[value], &[entry]));
    }
}
