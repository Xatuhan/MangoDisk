//! Interval-effective frequency from native PDH; static MHz is never presented as live.
use super::{valid_mhz, CpuFrequency, CpuFrequencySource, CpuIdentity};
use crate::{diagnostics::text, PlatformError, PlatformErrorCode, PlatformResult};
use std::{
    ptr,
    time::{Duration, Instant},
};
use windows_sys::{core::w, Win32::System::Performance::*};
use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};

// Windows SDK pdh.h; windows-sys does not expose this formatting flag.
const PDH_FMT_NOCAP100: u32 = 0x8000;

pub(super) fn identity() -> CpuIdentity {
    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
        .ok();
    let model = key
        .as_ref()
        .and_then(|key| key.get_value::<String, _>("ProcessorNameString").ok())
        .map(|model| model.trim().to_owned())
        .filter(|model| !model.is_empty());
    // Registry ~MHz is an estimate made at boot, not the manufacturer's rated clock.
    let nominal_frequency_mhz = model.as_deref().and_then(brand_frequency_mhz);
    log::info!(
        "cpu_identity source=processor_registry model={} brand_nominal_mhz={nominal_frequency_mhz:?}",
        text(model.as_deref().unwrap_or("unavailable"))
    );
    CpuIdentity {
        model,
        nominal_frequency_mhz,
        frequency_classes: vec![super::CpuFrequencyClass::Average],
    }
}
fn brand_frequency_mhz(model: &str) -> Option<f64> {
    let (prefix, suffix) = model.rsplit_once('@')?;
    if prefix.trim().is_empty() {
        return None;
    }
    let frequency = suffix.trim().to_ascii_lowercase();
    let value = frequency.strip_suffix("ghz")?.trim().parse::<f64>().ok()?;
    valid_mhz(value * 1000.0)
}
struct Query {
    handle: PDH_HQUERY,
    frequency: PDH_HCOUNTER,
    performance: PDH_HCOUNTER,
    previous: Option<Instant>,
}
impl Drop for Query {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.handle);
        }
    }
}
fn native_error(stage: &'static str, code: u32) -> PlatformError {
    PlatformError::new(
        if matches!(code, PDH_CSTATUS_NO_COUNTER | PDH_CSTATUS_NO_OBJECT) {
            PlatformErrorCode::Unsupported
        } else {
            PlatformErrorCode::OperationFailed
        },
        format!("CPU frequency stage={stage} native_code={code:#x}"),
    )
}
fn check(stage: &'static str, code: u32) -> PlatformResult<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(native_error(stage, code))
    }
}
impl Query {
    fn open() -> PlatformResult<Self> {
        let mut handle = ptr::null_mut();
        check("open", unsafe {
            PdhOpenQueryW(ptr::null(), 0, &mut handle)
        })?;
        let mut query = Self {
            handle,
            frequency: ptr::null_mut(),
            performance: ptr::null_mut(),
            previous: None,
        };
        check("add_nominal", unsafe {
            PdhAddEnglishCounterW(
                handle,
                w!("\\Processor Information(_Total)\\Processor Frequency"),
                0,
                &mut query.frequency,
            )
        })?;
        check("add_performance", unsafe {
            PdhAddEnglishCounterW(
                handle,
                w!("\\Processor Information(_Total)\\% Processor Performance"),
                0,
                &mut query.performance,
            )
        })?;
        Ok(query)
    }
    fn counter(&self, counter: PDH_HCOUNTER) -> PlatformResult<f64> {
        let mut value = PDH_FMT_COUNTERVALUE::default();
        // Turbo performance must retain values above 100%; the default formatter caps them.
        check("format", unsafe {
            PdhGetFormattedCounterValue(
                counter,
                PDH_FMT_DOUBLE | PDH_FMT_NOCAP100,
                ptr::null_mut(),
                &mut value,
            )
        })?;
        if !matches!(value.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA) {
            return Err(native_error("status", value.CStatus));
        }
        Ok(unsafe { value.Anonymous.doubleValue })
    }
    fn read(&mut self) -> PlatformResult<Option<CpuFrequency>> {
        check("collect", unsafe { PdhCollectQueryData(self.handle) })?;
        let now = Instant::now();
        let interval = self
            .previous
            .replace(now)
            .map(|previous| now.duration_since(previous).as_millis());
        if !interval.is_some_and(|ms| (100..=5000).contains(&ms)) {
            return Ok(None);
        }
        let nominal = self.counter(self.frequency)?;
        let performance = self.counter(self.performance)?;
        if performance == 0.0 {
            return Ok(None);
        }
        let Some(average_mhz) = effective_mhz(nominal, performance) else {
            return Err(PlatformError::new(
                PlatformErrorCode::OperationFailed,
                "CPU frequency counters contain no valid active observation",
            ));
        };
        Ok(Some(CpuFrequency {
            average_mhz: Some(average_mhz),
            efficiency_mhz: None,
            performance_mhz: None,
            super_mhz: None,
            source: CpuFrequencySource::WindowsPerformance,
            activity: Vec::new(),
        }))
    }
}
fn effective_mhz(nominal: f64, performance: f64) -> Option<f64> {
    valid_mhz(nominal)?;
    if !performance.is_finite() || performance <= 0.0 {
        return None;
    }
    valid_mhz(nominal * performance / 100.0)
}
#[derive(Default)]
pub(super) struct FrequencyReader {
    query: Option<Query>,
    retry_at: Option<Instant>,
    diagnostic: Option<String>,
    last_error: Option<PlatformError>,
}
impl FrequencyReader {
    pub(super) fn pause(&mut self) {
        if let Some(query) = &mut self.query {
            query.previous = None;
        }
    }
    pub(super) fn read(&mut self) -> PlatformResult<Option<CpuFrequency>> {
        if self.retry_at.is_some_and(|at| Instant::now() < at) {
            return Err(self.last_error.clone().unwrap_or_else(super::unsupported));
        }
        let result = (|| {
            if self.query.is_none() {
                self.query = Some(Query::open()?);
            }
            self.query.as_mut().expect("opened frequency query").read()
        })();
        let state = match &result {
            Ok(_) => "ready".to_owned(),
            Err(error) => error.to_string(),
        };
        if self.diagnostic.as_ref() != Some(&state) {
            log::info!("cpu_frequency_observation source=pdh_processor_performance outcome={} retry_seconds=30",text(&state));
            self.diagnostic = Some(state);
        }
        self.last_error = result.as_ref().err().cloned();
        if result.is_err() {
            self.query = None;
            self.retry_at = Some(Instant::now() + Duration::from_secs(30));
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rated_frequency_uses_explicit_brand_clock_instead_of_boot_estimates() {
        assert_eq!(
            brand_frequency_mhz("Intel(R) Core(TM) i9-10900K CPU @ 3.70GHz"),
            Some(3700.0)
        );
        assert_eq!(brand_frequency_mhz("AMD Ryzen 9 7950X"), None);
        assert_eq!(brand_frequency_mhz("CPU @ 0GHz"), None);
        assert_eq!(brand_frequency_mhz("CPU @ NaNGHz"), None);
    }
    #[test]
    fn effective_frequency_preserves_turbo_without_using_load_as_frequency() {
        assert_eq!(effective_mhz(3700.0, 129.0), Some(4773.0));
        assert_eq!(effective_mhz(3700.0, 50.0), Some(1850.0));
        for (base, pct) in [
            (0.0, 100.0),
            (3700.0, 0.0),
            (3700.0, f64::NAN),
            (3700.0, 1000.0),
        ] {
            assert_eq!(effective_mhz(base, pct), None);
        }
    }
}
