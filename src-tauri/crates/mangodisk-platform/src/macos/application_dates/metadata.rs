use std::{
    ffi::{c_void, CString},
    fs,
    os::unix::ffi::OsStrExt,
    path::Path,
};

use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFType, CFTypeRef, TCFType},
    date::CFDate,
    dictionary::{CFDictionary, CFDictionaryRef},
    string::{CFString, CFStringRef},
};

use crate::{
    ApplicationDate, ApplicationDatePrecision, ApplicationDateReadStatus, ApplicationDateSource,
    ApplicationDateTarget, ApplicationDates,
};

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn MDItemCreate(allocator: *const c_void, path: CFStringRef) -> CFTypeRef;
    fn MDItemCopyAttributes(item: CFTypeRef, names: CFArrayRef) -> CFDictionaryRef;
}

pub(super) fn read(target: &ApplicationDateTarget, now: u64) -> ApplicationDates {
    let mut result = ApplicationDates {
        read_status: ApplicationDateReadStatus::Complete,
        ..ApplicationDates::default()
    };
    if let Some(values) = attributes(&target.path) {
        result.last_used = read_date(
            &values,
            "kMDItemLastUsedDate",
            ApplicationDateSource::SpotlightLastUsed,
            now,
        );
        if result.last_used.is_none() {
            if let Some(dates) =
                attribute(&values, "kMDItemUsedDates").and_then(|value| value.downcast::<CFArray>())
            {
                if dates.len() > 4096 {
                    result.read_status = ApplicationDateReadStatus::Partial;
                }
                result.last_used = used_dates(&dates, now);
            }
        }
        result.installed = read_date(
            &values,
            "kMDItemDateAdded",
            ApplicationDateSource::SpotlightAdded,
            now,
        );
    } else {
        result.read_status = ApplicationDateReadStatus::Partial;
    }
    // ATTR_CMN_ADDEDTIME is the filesystem's added-to-directory fact. A direct syscall avoids
    // NSURL resource-cache work and does not depend on Spotlight publication or UTF-8 paths.
    if let Some(added) = native_added(&target.path)
        .and_then(|value| ApplicationDate::new(value, ApplicationDateSource::DirectoryAdded, now))
    {
        result.installed = Some(added);
    }
    if result.installed.is_none() {
        result.installed = fs::symlink_metadata(&target.path)
            .ok()
            .filter(|metadata| !metadata.file_type().is_symlink())
            .and_then(|metadata| metadata.created().ok())
            .and_then(super::unix_millis)
            .and_then(|value| {
                ApplicationDate::new(value, ApplicationDateSource::BundleCreated, now)
            });
    }
    result
}

fn used_dates(dates: &CFArray, now: u64) -> Option<ApplicationDate> {
    let mut result = None;
    // Retain the recent end of a large chronological history. The partial flag still records
    // that a bounded read cannot prove the newest value if a provider returns an unsorted array.
    for index in dates.len().saturating_sub(4096).max(0)..dates.len() {
        // SAFETY: The index is bounded by the retained array and each element is type-checked.
        let raw = unsafe {
            core_foundation::array::CFArrayGetValueAtIndex(dates.as_concrete_TypeRef(), index)
        };
        if raw.is_null() {
            continue;
        }
        let value = unsafe { CFType::wrap_under_get_rule(raw) };
        if let Some(mut date) = value
            .downcast::<CFDate>()
            .and_then(|date| super::apple_millis(date.abs_time()))
            .and_then(|date| {
                ApplicationDate::new(date, ApplicationDateSource::SpotlightUsedDates, now)
            })
        {
            date.precision = ApplicationDatePrecision::Day;
            date.partial = dates.len() > 4096;
            super::choose_latest(&mut result, date);
        }
    }
    result
}

fn attributes(path: &Path) -> Option<CFDictionary<CFString, CFType>> {
    // Never interrogate a different, lossy spelling of a non-UTF-8 installation path.
    let path = CFString::new(path.to_str()?);
    let keys = CFArray::from_CFTypes(&[
        CFString::new("kMDItemLastUsedDate"),
        CFString::new("kMDItemUsedDates"),
        CFString::new("kMDItemDateAdded"),
    ]);
    // SAFETY: Inputs are retained CF objects; Create/Copy results are transferred into RAII owners.
    let raw = unsafe { MDItemCreate(std::ptr::null(), path.as_concrete_TypeRef()) };
    if raw.is_null() {
        return None;
    }
    let item = unsafe { CFType::wrap_under_create_rule(raw) };
    let values = unsafe { MDItemCopyAttributes(item.as_CFTypeRef(), keys.as_concrete_TypeRef()) };
    (!values.is_null()).then(|| unsafe { CFDictionary::wrap_under_create_rule(values) })
}

fn attribute(values: &CFDictionary<CFString, CFType>, name: &str) -> Option<CFType> {
    values
        .find(CFString::new(name))
        .map(|value| (*value).clone())
}

fn read_date(
    values: &CFDictionary<CFString, CFType>,
    name: &str,
    source: ApplicationDateSource,
    now: u64,
) -> Option<ApplicationDate> {
    let value = attribute(values, name)?.downcast::<CFDate>()?;
    ApplicationDate::new(super::apple_millis(value.abs_time())?, source, now)
}

fn native_added(path: &Path) -> Option<u64> {
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: libc::ATTR_CMN_ADDEDTIME,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut bytes = [0_u8; 32];
    // SAFETY: Darwin packs attributes on four-byte boundaries. Decode bytes explicitly instead
    // of creating an unaligned timespec reference; the supplied buffer exceeds its 20-byte record.
    let status = unsafe {
        libc::getattrlist(
            path.as_ptr(),
            std::ptr::from_mut(&mut attributes).cast(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            libc::FSOPT_NOFOLLOW,
        )
    };
    (status == 0).then(|| decode_added(&bytes)).flatten()
}

fn decode_added(bytes: &[u8]) -> Option<u64> {
    let length = u32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    if length < 20 || length > bytes.len() {
        return None;
    }
    let seconds = i64::from_ne_bytes(bytes.get(4..12)?.try_into().ok()?);
    let nanos = i64::from_ne_bytes(bytes.get(12..20)?.try_into().ok()?);
    if seconds <= 0 || !(0..1_000_000_000).contains(&nanos) {
        return None;
    }
    u64::try_from(seconds)
        .ok()?
        .checked_mul(1000)?
        .checked_add(nanos as u64 / 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_day_histories_keep_the_recent_end_and_expose_the_read_bound() {
        let values = (0..4100)
            .map(|index| CFDate::new(f64::from(index)))
            .collect::<Vec<_>>();
        let dates = CFArray::from_CFTypes(&values)
            .as_CFType()
            .downcast::<CFArray>()
            .unwrap();
        let result = used_dates(&dates, u64::MAX).unwrap();
        assert_eq!(
            result.timestamp_ms,
            super::super::apple_millis(4099.0).unwrap()
        );
        assert_eq!(result.precision, ApplicationDatePrecision::Day);
        assert!(result.partial);
    }

    #[test]
    fn short_day_histories_start_at_zero_and_reject_future_values() {
        let values = [CFDate::new(2.0), CFDate::new(1.0), CFDate::new(100.0)];
        let dates = CFArray::from_CFTypes(&values)
            .as_CFType()
            .downcast::<CFArray>()
            .unwrap();
        let result = used_dates(&dates, super::super::apple_millis(10.0).unwrap()).unwrap();
        assert_eq!(
            result.timestamp_ms,
            super::super::apple_millis(2.0).unwrap()
        );
        assert!(!result.partial);
    }

    #[test]
    fn packed_added_time_validates_lengths_and_nanoseconds() {
        let mut bytes = vec![0; 20];
        bytes[..4].copy_from_slice(&20_u32.to_ne_bytes());
        bytes[4..12].copy_from_slice(&10_i64.to_ne_bytes());
        bytes[12..20].copy_from_slice(&250_000_000_i64.to_ne_bytes());
        assert_eq!(decode_added(&bytes), Some(10_250));
        assert!(decode_added(&bytes[..19]).is_none());
        bytes[12..20].copy_from_slice(&1_000_000_000_i64.to_ne_bytes());
        assert!(decode_added(&bytes).is_none());
    }
}
