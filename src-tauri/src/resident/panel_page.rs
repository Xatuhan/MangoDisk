//! Panel navigation is independent of native sampling and display metric IDs.
use mangodisk_core::system_resources::metrics::MetricId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PanelPage {
    Overview,
    #[default]
    Cpu,
    Gpu,
    Memory,
    Network,
    Disk,
    Power,
}

impl PanelPage {
    pub(super) fn available_for_battery(self, present: Option<bool>) -> Self {
        if self == Self::Power && present == Some(false) {
            Self::Cpu
        } else {
            self
        }
    }

    pub fn metric(self) -> Option<MetricId> {
        match self {
            Self::Cpu => Some(MetricId::Cpu),
            Self::Gpu => Some(MetricId::Gpu),
            Self::Memory => Some(MetricId::Memory),
            Self::Network => Some(MetricId::Network),
            Self::Disk => Some(MetricId::Disk),
            Self::Overview | Self::Power => None,
        }
    }
}

impl From<MetricId> for PanelPage {
    fn from(metric: MetricId) -> Self {
        match metric {
            MetricId::Cpu => Self::Cpu,
            MetricId::Gpu => Self::Gpu,
            MetricId::Memory => Self::Memory,
            MetricId::Network => Self::Network,
            MetricId::Disk => Self::Disk,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_confirmed_battery_absence_redirects_the_power_page() {
        for page in [
            PanelPage::Overview,
            PanelPage::Cpu,
            PanelPage::Gpu,
            PanelPage::Memory,
            PanelPage::Network,
            PanelPage::Disk,
            PanelPage::Power,
        ] {
            assert_eq!(page.available_for_battery(None), page);
            assert_eq!(page.available_for_battery(Some(true)), page);
            assert_eq!(
                page.available_for_battery(Some(false)),
                if page == PanelPage::Power {
                    PanelPage::Cpu
                } else {
                    page
                }
            );
        }
    }
}
