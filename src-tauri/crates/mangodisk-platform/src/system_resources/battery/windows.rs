//! Only battery query IOCTLs are issued; no charging policy or device settings are changed.
use super::{
    windows_values::{self, BatteryData},
    BatterySample,
};
use crate::{PlatformError, PlatformErrorCode, PlatformResult};
use std::{
    mem::{size_of, size_of_val},
    ptr,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Devices::DeviceAndDriverInstallation::*,
    Foundation::*,
    Storage::FileSystem::*,
    System::{Power::*, IO::DeviceIoControl},
};

fn native_error(stage: &str, code: u32) -> PlatformError {
    PlatformError::new(
        if matches!(code, ERROR_INVALID_FUNCTION | ERROR_NOT_SUPPORTED) {
            PlatformErrorCode::Unsupported
        } else {
            PlatformErrorCode::Io
        },
        format!("battery stage={stage} native_code={code}"),
    )
}
fn failed(stage: &str) -> PlatformError {
    native_error(stage, unsafe { GetLastError() })
}
struct Device(HANDLE);
impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Inventory(HDEVINFO);
impl Drop for Inventory {
    fn drop(&mut self) {
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

pub(super) fn battery_present() -> PlatformResult<bool> {
    let mut status = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
        return Err(failed("presence"));
    }
    if status.BatteryFlag == 255 {
        return Err(PlatformError::new(
            PlatformErrorCode::InvalidData,
            "battery stage=presence reason=unknown_flags",
        ));
    }
    Ok(status.BatteryFlag & 128 == 0)
}

fn enumerate() -> PlatformResult<Vec<Device>> {
    let raw = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVCLASS_BATTERY,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if raw == -1 {
        return Err(failed("enumerate"));
    }
    let inventory = Inventory(raw);
    let mut result = Vec::new();
    for index in 0..32 {
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        if unsafe {
            SetupDiEnumDeviceInterfaces(
                inventory.0,
                ptr::null(),
                &GUID_DEVCLASS_BATTERY,
                index,
                &mut interface,
            )
        } == 0
        {
            let code = unsafe { GetLastError() };
            if code == ERROR_NO_MORE_ITEMS {
                return Ok(result);
            }
            return Err(native_error("enumerate_interface", code));
        }
        let mut required = 0;
        unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                inventory.0,
                &interface,
                ptr::null_mut(),
                0,
                &mut required,
                ptr::null_mut(),
            );
        }
        if !(8..=65536).contains(&required) {
            return Err(failed("interface_size"));
        }
        // The SDK structure has pointer-width alignment, while DevicePath begins at byte four.
        let mut storage = vec![0u64; (required as usize).div_ceil(8)];
        let detail = storage
            .as_mut_ptr()
            .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        unsafe {
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        }
        if unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                inventory.0,
                &interface,
                detail,
                required,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(failed("interface_path"));
        }
        let handle = unsafe {
            CreateFileW(
                (*detail).DevicePath.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(failed("device_open"));
        }
        result.push(Device(handle));
    }
    Err(PlatformError::new(
        PlatformErrorCode::InvalidData,
        "battery stage=enumerate reason=device_limit",
    ))
}

fn query<I: Copy, O: Default + Copy>(
    device: &Device,
    code: u32,
    input: &I,
    stage: &str,
) -> PlatformResult<O> {
    let mut output = O::default();
    let mut returned = 0;
    if unsafe {
        DeviceIoControl(
            device.0,
            code,
            (input as *const I).cast(),
            size_of_val(input) as u32,
            (&mut output as *mut O).cast(),
            size_of::<O>() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(failed(stage));
    }
    if returned as usize != size_of::<O>() {
        return Err(PlatformError::new(
            PlatformErrorCode::InvalidData,
            format!("battery stage={stage} reason=output_size bytes={returned}"),
        ));
    }
    Ok(output)
}

fn optional<T>(result: PlatformResult<T>) -> PlatformResult<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code() == PlatformErrorCode::Unsupported => Ok(None),
        Err(error) => Err(error),
    }
}

#[derive(Default)]
pub(super) struct Reader {
    devices: Vec<Device>,
    enumerated: Option<Instant>,
    diagnostic: Option<String>,
    capabilities: Option<String>,
}
impl Reader {
    pub(super) fn read(&mut self) -> PlatformResult<BatterySample> {
        let result = self.read_current();
        if let Err(error) = &result {
            let state = format!("{:?}:{}", error.code(), error.diagnostic());
            if self.diagnostic.as_ref() != Some(&state) {
                log::info!(
                    "windows_battery_unavailable {}",
                    crate::diagnostics::text(&state)
                );
                self.diagnostic = Some(state);
            }
            if error.code() != PlatformErrorCode::Unsupported {
                self.devices.clear();
                self.enumerated = None;
            }
        } else {
            self.diagnostic = None;
        }
        result
    }
    fn read_current(&mut self) -> PlatformResult<BatterySample> {
        if self
            .enumerated
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(30))
        {
            self.devices = enumerate()?;
            self.enumerated = Some(Instant::now());
        }
        let mut values = Vec::new();
        for device in &self.devices {
            let tag: u32 = query(device, IOCTL_BATTERY_QUERY_TAG, &0u32, "query_tag")?;
            if tag == 0 {
                continue;
            }
            let mut information = BATTERY_QUERY_INFORMATION {
                BatteryTag: tag,
                InformationLevel: BatteryInformation,
                AtRate: 0,
            };
            let details: BATTERY_INFORMATION = query(
                device,
                IOCTL_BATTERY_QUERY_INFORMATION,
                &information,
                "query_information",
            )?;
            if details.Capabilities & BATTERY_SYSTEM_BATTERY == 0
                || details.Capabilities & BATTERY_IS_SHORT_TERM != 0
            {
                continue;
            }
            information.InformationLevel = BatteryTemperature;
            let temperature = optional(query::<_, u32>(
                device,
                IOCTL_BATTERY_QUERY_INFORMATION,
                &information,
                "query_temperature",
            ))?;
            let status: Option<BATTERY_STATUS> = optional(query(
                device,
                IOCTL_BATTERY_QUERY_STATUS,
                &BATTERY_WAIT_STATUS {
                    BatteryTag: tag,
                    ..Default::default()
                },
                "query_status",
            ))?;
            log::debug!("windows_battery_sample source=battery_ioctl temperature_supported={} rate_supported={} relative={} outcome=queried", temperature.is_some(), status.is_some_and(|status| status.Rate != i32::MIN), details.Capabilities & BATTERY_CAPACITY_RELATIVE != 0);
            values.push(BatteryData {
                temperature,
                capabilities: details.Capabilities,
                power_state: status.map(|status| status.PowerState),
                rate: status.map(|status| status.Rate),
                current_capacity: status.map(|status| status.Capacity),
                full_capacity: Some(details.FullChargedCapacity),
                design_capacity: Some(details.DesignedCapacity),
                cycle_count: Some(details.CycleCount),
                voltage_mv: status.map(|status| status.Voltage),
            });
        }
        let capabilities = format!("batteries={} temperature_readings={} relative_units={} unknown_rates={} status_unavailable={}",
            values.len(), values.iter().filter(|value| value.temperature.is_some()).count(),
            values.iter().filter(|value| value.capabilities & BATTERY_CAPACITY_RELATIVE != 0).count(),
            values.iter().filter(|value| value.rate.is_none_or(|rate| rate == i32::MIN)).count(),
            values.iter().filter(|value| value.power_state.is_none()).count());
        if self.capabilities.as_ref() != Some(&capabilities) {
            log::info!(
                "windows_battery_capability source=battery_ioctl {capabilities} outcome=queried"
            );
            self.capabilities = Some(capabilities);
        }
        let mut sample = windows_values::sample(&values)?;
        let mut status = SYSTEM_POWER_STATUS::default();
        if unsafe { GetSystemPowerStatus(&mut status) } != 0 {
            if let Some(details) = &mut sample.details {
                windows_values::apply_system_status(
                    details,
                    status.BatteryLifePercent,
                    status.BatteryLifeTime,
                );
            }
        }
        Ok(sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a Windows battery driver; compares native power status"]
    fn native_battery_details_match_the_system_percentage() {
        let mut status = SYSTEM_POWER_STATUS::default();
        assert_ne!(unsafe { GetSystemPowerStatus(&mut status) }, 0);
        if status.BatteryFlag & 128 != 0 || status.BatteryFlag == 255 {
            println!("battery_native_check outcome=no_known_battery");
            return;
        }
        let mut reader = Reader::default();
        let sample = reader
            .read()
            .expect("a known battery must expose basic power details");
        let details = sample.details.expect("battery details must be present");
        if status.BatteryLifePercent <= 100 {
            let actual = details
                .charge_percent
                .expect("system percentage must be retained");
            assert!((actual - f64::from(status.BatteryLifePercent)).abs() <= 1.0);
        }
        println!("battery_native_check charge_percent={:?} state={:?} capacity_available={} power_available={}",
            details.charge_percent, details.charge_state, details.capacity.is_some(), sample.battery_power.is_some());
    }
}
