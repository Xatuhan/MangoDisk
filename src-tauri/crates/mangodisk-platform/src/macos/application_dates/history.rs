use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::Path,
    time::{Duration, Instant},
};

use rusqlite::{Connection, OpenFlags};

use crate::{ApplicationDate, ApplicationDateSource, PlatformCancellation};

const MAX_SEGMENT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_HISTORY_BYTES: u64 = 16 * 1024 * 1024;
const MAX_HISTORY_IDENTITIES: usize = 4096;

#[derive(Default)]
pub(super) struct HistoryDates {
    pub by_identifier: HashMap<String, ApplicationDate>,
    pub partial: bool,
}

pub(super) fn capture(now: u64, cancellation: &PlatformCancellation) -> HistoryDates {
    let mut result = HistoryDates::default();
    let Ok(home) = super::super::directories::home_directory() else {
        return result;
    };
    let knowledge = home.join("Library/Application Support/Knowledge/knowledgeC.db");
    if super::activity::safe_metadata(&home, &knowledge, &mut result.partial).is_some() {
        if let Err(error) = read_knowledge(&knowledge, now, cancellation, &mut result) {
            result.partial = true;
            log::debug!("application_date_history_unavailable source=knowledge stage=query path={} native_error={} outcome=optional_fallback", crate::diagnostics::text(&knowledge.display()), crate::diagnostics::text(&error));
        }
    }
    let directory = home.join("Library/Biome/streams/restricted/App.InFocus/local");
    if super::activity::safe_metadata(&home, &directory, &mut result.partial).is_some() {
        read_biome(&directory, now, cancellation, &mut result);
    }
    result
}

fn read_knowledge(
    path: &Path,
    now: u64,
    cancellation: &PlatformCancellation,
    result: &mut HistoryDates,
) -> Result<(), rusqlite::Error> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_millis(5))?;
    let deadline = Instant::now() + Duration::from_millis(100);
    let cancellation = cancellation.clone();
    connection.progress_handler(
        1000,
        Some(move || cancellation.is_cancelled() || Instant::now() >= deadline),
    )?;
    let mut statement = connection.prepare(
        "SELECT o.ZVALUESTRING, MAX(o.ZENDDATE) FROM ZOBJECT o
         LEFT JOIN ZSOURCE s ON s.Z_PK=o.ZSOURCE
         WHERE o.ZSTREAMNAME='/app/usage' AND o.ZVALUESTRING IS NOT NULL
         AND s.Z_PK IS NOT NULL AND (s.ZDEVICEID IS NULL OR s.ZDEVICEID='')
         AND o.ZENDDATE>o.ZSTARTDATE AND o.ZENDDATE-o.ZSTARTDATE<=86400 AND o.ZENDDATE<=?1
         GROUP BY o.ZVALUESTRING LIMIT ?2",
    )?;
    // One extra row distinguishes an exhausted query from a truncated result.
    let mut rows = statement.query(rusqlite::params![
        now as f64 / 1000.0 - 978_307_200.0,
        (MAX_HISTORY_IDENTITIES + 1) as i64,
    ])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        if count == MAX_HISTORY_IDENTITIES {
            result.partial = true;
            break;
        }
        count += 1;
        let identifier = row.get::<_, String>(0)?;
        let time = row.get::<_, f64>(1)?;
        if let Some(timestamp) = super::apple_millis(time) {
            record(
                result,
                &identifier,
                timestamp,
                ApplicationDateSource::KnowledgeUsage,
                now,
            );
        }
    }
    Ok(())
}

fn record(
    result: &mut HistoryDates,
    identifier: &str,
    timestamp: u64,
    source: ApplicationDateSource,
    now: u64,
) {
    if !super::valid_identifier(identifier) {
        return;
    }
    if result.by_identifier.len() >= MAX_HISTORY_IDENTITIES
        && !result.by_identifier.contains_key(identifier)
    {
        result.partial = true;
        return;
    }
    let Some(date) = ApplicationDate::new(timestamp, source, now) else {
        return;
    };
    match result.by_identifier.entry(identifier.to_owned()) {
        std::collections::hash_map::Entry::Occupied(mut entry) => {
            if timestamp > entry.get().timestamp_ms {
                entry.insert(date);
            }
        }
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(date);
        }
    }
}

fn read_biome(
    directory: &Path,
    now: u64,
    cancellation: &PlatformCancellation,
    result: &mut HistoryDates,
) {
    let deadline = Instant::now() + Duration::from_millis(100);
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            result.partial = true;
            log::debug!("application_date_history_unavailable source=biome stage=enumerate path={} native_error={} outcome=optional_fallback", crate::diagnostics::text(&directory.display()), crate::diagnostics::text(&error));
            return;
        }
    };
    // Newer segments are more useful when retention or record counts exceed the read budget.
    let mut paths = Vec::new();
    for (index, entry) in entries.take(257).enumerate() {
        // Apply the enumeration bound before filtering names; unrelated entries and read
        // errors also consume the budget and can hide a later, valid segment.
        if index == 256 || cancellation.is_cancelled() || Instant::now() >= deadline {
            result.partial = true;
            break;
        }
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(_) => {
                result.partial = true;
                continue;
            }
        };
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit()))
        {
            paths.push(path);
        }
    }
    paths.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
    let mut bytes = 0;
    for path in paths {
        if cancellation.is_cancelled() || Instant::now() >= deadline || bytes >= MAX_HISTORY_BYTES {
            result.partial = true;
            break;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            result.partial = true;
            continue;
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.len() > MAX_SEGMENT_BYTES || bytes + metadata.len() > MAX_HISTORY_BYTES {
            result.partial = true;
            continue;
        }
        let mut data = Vec::new();
        let read = fs::File::open(&path)
            .and_then(|file| file.take(MAX_SEGMENT_BYTES + 1).read_to_end(&mut data));
        if read.is_err() || data.len() as u64 != metadata.len() {
            result.partial = true;
            continue;
        }
        bytes += data.len() as u64;
        if !read_segment(&data, now, cancellation, deadline, result) {
            result.partial = true;
        }
    }
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn read_segment(
    data: &[u8],
    now: u64,
    cancellation: &PlatformCancellation,
    deadline: Instant,
    result: &mut HistoryDates,
) -> bool {
    if data.get(..4) != Some(b"SEGB") || data.len() < 32 {
        return false;
    }
    let Some(count) = u32_at(data, 4).map(|value| value as usize) else {
        return false;
    };
    if count > (data.len() - 32) / 16 {
        return false;
    }
    let mut start = 32_usize;
    let mut complete = true;
    for index in 0..count {
        if index % 128 == 0 && (cancellation.is_cancelled() || Instant::now() >= deadline) {
            return false;
        }
        let entry = data.len() - (index + 1) * 16;
        let Some(end_offset) = u32_at(data, entry) else {
            return false;
        };
        if end_offset == 0 {
            continue;
        }
        let Some(end) = (end_offset as usize)
            .checked_add(32)
            .filter(|end| *end <= entry && *end >= start + 8)
        else {
            return false;
        };
        if u32_at(data, entry + 4) == Some(1) {
            let payload = &data[start + 8..end];
            if u32_at(data, start) == Some(crc32fast::hash(payload)) {
                match focus_event(payload) {
                    Some(FocusEvent::Application(identifier, timestamp)) => record(
                        result,
                        identifier,
                        timestamp,
                        ApplicationDateSource::BiomeFocus,
                        now,
                    ),
                    Some(FocusEvent::Other) => {}
                    None => complete = false,
                }
            } else {
                complete = false;
            }
        }
        start = (end + 3) & !3;
    }
    complete
}

fn varint(data: &[u8], offset: &mut usize) -> Option<u64> {
    let mut value = 0_u64;
    for shift in (0..70).step_by(7) {
        let byte = *data.get(*offset)?;
        *offset += 1;
        if shift == 63 && byte > 1 {
            return None;
        }
        value |= u64::from(byte & 127) << shift;
        if byte < 128 {
            return Some(value);
        }
    }
    None
}

#[derive(Debug, PartialEq)]
enum FocusEvent<'a> {
    Application(&'a str, u64),
    Other,
}

fn focus_event(data: &[u8]) -> Option<FocusEvent<'_>> {
    let mut offset = 0;
    let (mut kind, mut status, mut timestamp, mut identifier) = (None, None, None, None);
    while offset < data.len() {
        let key = varint(data, &mut offset)?;
        match key & 7 {
            0 => {
                let value = varint(data, &mut offset)?;
                match key >> 3 {
                    2 => kind = Some(value),
                    3 => status = Some(value),
                    _ => {}
                }
            }
            1 => {
                let end = offset.checked_add(8)?;
                if key >> 3 == 4 {
                    timestamp = super::apple_millis(f64::from_le_bytes(
                        data.get(offset..end)?.try_into().ok()?,
                    ));
                }
                data.get(offset..end)?;
                offset = end;
            }
            2 => {
                let length = usize::try_from(varint(data, &mut offset)?).ok()?;
                let end = offset.checked_add(length)?;
                let bytes = data.get(offset..end)?;
                if key >> 3 == 6 {
                    identifier = std::str::from_utf8(bytes).ok();
                }
                offset = end;
            }
            5 => {
                offset = offset.checked_add(4)?;
                data.get(..offset)?;
            }
            _ => return None,
        }
    }
    if kind.is_some_and(|value| value != 1) {
        return Some(FocusEvent::Other);
    }
    (kind == Some(1) && matches!(status, Some(0 | 1)))
        .then_some(FocusEvent::Application(identifier?, timestamp?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> Vec<u8> {
        let mut bytes = vec![16, 1, 24, 1, 33];
        bytes.extend(10.0_f64.to_le_bytes());
        bytes.extend([50, 15]);
        bytes.extend(b"com.example.App");
        bytes
    }

    #[test]
    fn focus_payload_is_bounded_and_rejects_truncation() {
        let bytes = event();
        assert_eq!(
            focus_event(&bytes),
            Some(FocusEvent::Application("com.example.App", 978_307_210_000))
        );
        for end in 0..bytes.len() {
            assert!(focus_event(&bytes[..end]).is_none());
        }
        assert!(focus_event(&[0xff; 20]).is_none());
        assert_eq!(focus_event(&[16, 3]), Some(FocusEvent::Other));
    }

    #[test]
    fn knowledge_history_excludes_other_devices_and_future_records_before_aggregation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("knowledge.db");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE ZSOURCE (Z_PK INTEGER, ZDEVICEID TEXT); CREATE TABLE ZOBJECT (ZSOURCE INTEGER, ZSTREAMNAME TEXT, ZVALUESTRING TEXT, ZSTARTDATE REAL, ZENDDATE REAL);
            INSERT INTO ZSOURCE VALUES (1,NULL),(2,'another-device');
            INSERT INTO ZOBJECT VALUES (1,'/app/usage','com.example.App',10,20),(1,'/app/usage','com.example.App',90,110),(2,'/app/usage','com.example.Remote',10,30);").unwrap();
        let mut result = HistoryDates::default();
        read_knowledge(
            &path,
            978_307_300_000,
            &PlatformCancellation::new(|| false),
            &mut result,
        )
        .unwrap();
        assert_eq!(result.by_identifier.len(), 1);
        assert_eq!(
            result.by_identifier["com.example.App"].timestamp_ms,
            978_307_220_000
        );
    }

    #[test]
    fn knowledge_identity_limit_marks_omitted_history_as_partial() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("knowledge.db");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE ZSOURCE (Z_PK INTEGER, ZDEVICEID TEXT);
            CREATE TABLE ZOBJECT (ZSOURCE INTEGER, ZSTREAMNAME TEXT, ZVALUESTRING TEXT, ZSTARTDATE REAL, ZENDDATE REAL);
            INSERT INTO ZSOURCE VALUES (1,NULL);
            WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<4096)
            INSERT INTO ZOBJECT SELECT 1,'/app/usage','com.example.App'||x,10,20 FROM n;").unwrap();
        let mut result = HistoryDates::default();
        read_knowledge(
            &path,
            978_307_300_000,
            &PlatformCancellation::new(|| false),
            &mut result,
        )
        .unwrap();
        assert_eq!(result.by_identifier.len(), MAX_HISTORY_IDENTITIES);
        assert!(result.partial, "the query omitted an application history");
        connection
            .execute(
                "DELETE FROM ZOBJECT WHERE ZVALUESTRING='com.example.App4096'",
                [],
            )
            .unwrap();
        let mut exact_limit = HistoryDates::default();
        read_knowledge(
            &path,
            978_307_300_000,
            &PlatformCancellation::new(|| false),
            &mut exact_limit,
        )
        .unwrap();
        assert_eq!(exact_limit.by_identifier.len(), MAX_HISTORY_IDENTITIES);
        assert!(
            !exact_limit.partial,
            "a fully read query at the limit is complete"
        );
    }

    #[test]
    fn biome_enumeration_limit_counts_non_segment_entries() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..258 {
            fs::write(root.path().join(format!("unrecognized-{index}")), []).unwrap();
        }
        let mut result = HistoryDates::default();
        read_biome(
            root.path(),
            u64::MAX,
            &PlatformCancellation::new(|| false),
            &mut result,
        );
        assert!(
            result.partial,
            "directory enumeration stopped before exhaustion"
        );
        fs::remove_file(root.path().join("unrecognized-256")).unwrap();
        fs::remove_file(root.path().join("unrecognized-257")).unwrap();
        let mut exact_limit = HistoryDates::default();
        read_biome(
            root.path(),
            u64::MAX,
            &PlatformCancellation::new(|| false),
            &mut exact_limit,
        );
        assert!(
            !exact_limit.partial,
            "exactly 256 entries are read completely"
        );
    }

    #[test]
    fn segment_ignores_deleted_records_and_bad_crc() {
        let payload = event();
        let record_end = 40 + payload.len();
        let mut bytes = vec![0; record_end + 16];
        bytes[..4].copy_from_slice(b"SEGB");
        bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
        bytes[32..36].copy_from_slice(&crc32fast::hash(&payload).to_le_bytes());
        bytes[40..record_end].copy_from_slice(&payload);
        bytes[record_end..record_end + 4]
            .copy_from_slice(&((record_end - 32) as u32).to_le_bytes());
        bytes[record_end + 4..record_end + 8].copy_from_slice(&1_u32.to_le_bytes());
        let mut result = HistoryDates::default();
        let cancellation = PlatformCancellation::new(|| false);
        assert!(read_segment(
            &bytes,
            u64::MAX,
            &cancellation,
            Instant::now() + Duration::from_secs(1),
            &mut result
        ));
        assert_eq!(result.by_identifier.len(), 1);
        bytes[32] ^= 1;
        let mut result = HistoryDates::default();
        assert!(!read_segment(
            &bytes,
            u64::MAX,
            &cancellation,
            Instant::now() + Duration::from_secs(1),
            &mut result
        ));
        assert!(result.by_identifier.is_empty());
        bytes[record_end + 4..record_end + 8].copy_from_slice(&3_u32.to_le_bytes());
        assert!(read_segment(
            &bytes,
            u64::MAX,
            &cancellation,
            Instant::now() + Duration::from_secs(1),
            &mut result
        ));
        assert!(result.by_identifier.is_empty());
    }
}
