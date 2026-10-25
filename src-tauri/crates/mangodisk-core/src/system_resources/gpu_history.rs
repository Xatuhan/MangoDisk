//! Retain only observations belonging to one physical GPU.
use super::{
    gpu::GpuUsage,
    metrics::{MetricReading, MetricStatus, Trend, TrendPoint},
};
use mangodisk_platform::system_resources::gpu::{details::GpuActivityKind, GpuAdapterUsage};

#[derive(Default)]
pub(super) struct GpuDeviceHistory {
    adapter_name: Option<String>,
    pub reading: MetricReading<GpuUsage>,
    pub usage: Trend,
    pub renderer: Trend,
    pub tiler: Trend,
}

impl GpuDeviceHistory {
    /// Return whether a native device replacement invalidated previous observations.
    pub fn observe(&mut self, adapter: &GpuAdapterUsage, timestamp_ms: u64) -> bool {
        let replaced = self
            .adapter_name
            .as_ref()
            .is_some_and(|name| name != &adapter.name);
        if self.adapter_name.as_ref() != Some(&adapter.name) {
            *self = Self {
                adapter_name: Some(adapter.name.clone()),
                ..Default::default()
            };
        }
        self.usage.push(TrendPoint {
            sampled_at_ms: timestamp_ms,
            primary: adapter.used_percent,
            secondary: None,
            gap_before: false,
        });
        self.reading = MetricReading::ready(
            GpuUsage {
                adapter_id: adapter.id.clone(),
                adapter_name: adapter.name.clone(),
                used_percent: adapter.used_percent,
                details: None,
            },
            timestamp_ms,
        );
        if let Some(details) = &adapter.details {
            for (kind, history) in [
                (GpuActivityKind::Renderer, &mut self.renderer),
                (GpuActivityKind::Tiler, &mut self.tiler),
            ] {
                if let Some(primary) = details
                    .activities
                    .iter()
                    .filter(|activity| activity.kind == kind)
                    .map(|activity| activity.used_percent)
                    .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
                    .max_by(f64::total_cmp)
                {
                    history.push(TrendPoint {
                        sampled_at_ms: timestamp_ms,
                        primary,
                        secondary: None,
                        gap_before: false,
                    });
                }
            }
        }
        replaced
    }

    pub fn fail(&mut self, status: MetricStatus) {
        self.reading.status = status;
        if matches!(
            status,
            MetricStatus::Unsupported | MetricStatus::Disconnected
        ) {
            self.reading.value = None;
            self.reading.sampled_at_ms = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mangodisk_platform::system_resources::gpu::details::*;

    #[test]
    fn native_replacement_resets_usage_and_engines_without_filling_missing_samples() {
        let sample = |name: &str, detailed: bool| GpuAdapterUsage {
            id: "a".into(),
            name: name.into(),
            used_percent: 30.0,
            details: detailed.then_some(GpuDetails {
                activities: [GpuActivityKind::Renderer, GpuActivityKind::Tiler]
                    .into_iter()
                    .map(|kind| GpuActivity {
                        id: format!("{kind:?}"),
                        kind,
                        name: None,
                        used_percent: 20.0,
                        included_in_summary: false,
                    })
                    .collect(),
                telemetry: Default::default(),
                memory_architecture: GpuMemoryArchitecture::Unified,
                memory_status: GpuMemoryStatus::Unsupported,
                memory: None,
            }),
        };
        let mut history = GpuDeviceHistory::default();
        assert!(!history.observe(&sample("A", true), 1000));
        assert!(!history.observe(&sample("A", false), 3000));
        assert!(!history.observe(&sample("A", true), 17000));
        assert_eq!(history.usage.snapshot(17000).len(), 3);
        for engine in [&mut history.renderer, &mut history.tiler] {
            assert_eq!(
                engine
                    .snapshot(17000)
                    .iter()
                    .map(|point| point.sampled_at_ms)
                    .collect::<Vec<_>>(),
                vec![1000, 17000]
            );
        }
        assert!(history.observe(&sample("Replacement", true), 19000));
        for trend in [
            &mut history.usage,
            &mut history.renderer,
            &mut history.tiler,
        ] {
            let points = trend.snapshot(19000);
            assert_eq!(points.len(), 1);
            assert_eq!(points[0].sampled_at_ms, 19000);
        }
    }
}
