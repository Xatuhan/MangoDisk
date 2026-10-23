use std::{
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

use super::super::bulk_directory::{
    AlignedBuffer, BulkDirectory, VNODE_TYPE_DIRECTORY, VNODE_TYPE_REGULAR_FILE,
};

use crate::{ApplicationDate, ApplicationDateSource, ApplicationDateTarget, PlatformCancellation};

const MAX_ENTRIES_PER_APPLICATION: usize = 1024;
const MAX_ENTRIES_PER_CAPTURE: usize = 20_000;
const MAX_DEPTH: usize = 4;

pub(super) struct ActivityBudget {
    deadline: Instant,
    pub entries: usize,
    pub partial: bool,
    buffer: AlignedBuffer,
    pub native_pages: usize,
    pub portable_directories: usize,
    by_host: Option<Vec<PathBuf>>,
    by_host_partial: bool,
    protected_data_available: Option<bool>,
}

impl ActivityBudget {
    pub(super) fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_millis(250),
            entries: 0,
            partial: false,
            buffer: AlignedBuffer::new(),
            native_pages: 0,
            portable_directories: 0,
            by_host: None,
            by_host_partial: false,
            protected_data_available: None,
        }
    }
}

pub(super) fn estimate(
    target: &ApplicationDateTarget,
    now: u64,
    cancellation: &PlatformCancellation,
    budget: &mut ActivityBudget,
) -> Option<ApplicationDate> {
    let home = super::super::directories::home_directory().ok()?;
    estimate_in(&home, &target.identifier, now, cancellation, budget)
}

fn estimate_in(
    home: &Path,
    identifier: &str,
    now: u64,
    cancellation: &PlatformCancellation,
    budget: &mut ActivityBudget,
) -> Option<ApplicationDate> {
    if !super::valid_identifier(identifier) || cancellation.is_cancelled() {
        return None;
    }
    let library = home.join("Library");
    let protected_data_available = *budget
        .protected_data_available
        .get_or_insert_with(|| protected_data_available(home));
    let mut strong = None;
    let mut weak = None;
    let mut directories = None;
    let mut partial = false;
    let preferences = [
        library
            .join("Preferences")
            .join(format!("{identifier}.plist")),
        library
            .join("Containers")
            .join(identifier)
            .join("Data/Library/Preferences")
            .join(format!("{identifier}.plist")),
    ];
    for path in preferences {
        if path.starts_with(library.join("Containers")) && !protected_data_available {
            partial = true;
            continue;
        }
        observe(
            home,
            &path,
            ApplicationDateSource::PreferencesModified,
            now,
            &mut strong,
            &mut directories,
            &mut partial,
        );
    }
    // Host-specific preference domains must match exactly. Prefix matching would include ShipIt
    // and other updater domains, turning installation activity into apparent application use.
    if budget.by_host.is_none() {
        let directory = library.join("Preferences/ByHost");
        let mut paths = Vec::new();
        if safe_metadata(home, &directory, &mut budget.by_host_partial).is_some() {
            match fs::read_dir(&directory) {
                Ok(entries) => {
                    for (index, entry) in entries.enumerate() {
                        if index >= 4096 || cancellation.is_cancelled() {
                            budget.by_host_partial = true;
                            break;
                        }
                        match entry {
                            Ok(entry) => paths.push(entry.path()),
                            Err(_) => budget.by_host_partial = true,
                        }
                    }
                }
                Err(_) => budget.by_host_partial = true,
            }
        }
        budget.by_host = Some(paths);
    }
    partial |= budget.by_host_partial;
    for path in budget.by_host.as_ref().into_iter().flatten() {
        if cancellation.is_cancelled() {
            return None;
        }
        if path
            .file_name()
            .is_some_and(|name| exact_host_domain(&name.to_string_lossy(), identifier))
        {
            observe(
                home,
                path,
                ApplicationDateSource::PreferencesModified,
                now,
                &mut strong,
                &mut directories,
                &mut partial,
            );
        }
    }
    let roots = [
        (
            library.join("Application Support").join(identifier),
            ApplicationDateSource::ApplicationDataModified,
        ),
        (
            library
                .join("Saved Application State")
                .join(format!("{identifier}.savedState")),
            ApplicationDateSource::ApplicationDataModified,
        ),
        (
            library
                .join("Containers")
                .join(identifier)
                .join("Data/Library"),
            ApplicationDateSource::ApplicationDataModified,
        ),
        (
            library.join("Caches").join(identifier),
            ApplicationDateSource::CacheModified,
        ),
        (
            library.join("Logs").join(identifier),
            ApplicationDateSource::CacheModified,
        ),
        (
            library.join("WebKit").join(identifier),
            ApplicationDateSource::CacheModified,
        ),
        (
            library.join("HTTPStorages").join(identifier),
            ApplicationDateSource::CacheModified,
        ),
    ];
    // Always collect cheap root facts before optional traversal exhausts its shared budget.
    let roots = roots
        .into_iter()
        .filter_map(|(path, source)| {
            if path.starts_with(library.join("Containers")) && !protected_data_available {
                partial = true;
                return None;
            }
            let metadata = observe(
                home,
                &path,
                source,
                now,
                if source == ApplicationDateSource::CacheModified {
                    &mut weak
                } else {
                    &mut strong
                },
                &mut directories,
                &mut partial,
            )?;
            metadata.is_dir().then_some((path, source))
        })
        .collect::<Vec<_>>();
    let mut visited = 0;
    let mut stack = roots
        .into_iter()
        .rev()
        .map(|(path, source)| (path, source, 0))
        .collect::<Vec<_>>();
    while let Some((path, source, depth)) = stack.pop() {
        if cancellation.is_cancelled() {
            return None;
        }
        if visited >= MAX_ENTRIES_PER_APPLICATION
            || budget.entries >= MAX_ENTRIES_PER_CAPTURE
            || Instant::now() >= budget.deadline
        {
            partial = true;
            break;
        }
        let remaining =
            (MAX_ENTRIES_PER_APPLICATION - visited).min(MAX_ENTRIES_PER_CAPTURE - budget.entries);
        let (entries, complete) = match read_entries(&path, remaining, budget) {
            Ok(entries) => entries,
            Err(error) => {
                partial = true;
                log::debug!("application_date_activity_incomplete path={} stage=read_directory native_code={:?} native_error={} outcome=partial_estimate", crate::diagnostics::text(&path.display()), error.raw_os_error(), crate::diagnostics::text(&error));
                continue;
            }
        };
        partial |= !complete;
        for entry in entries {
            if cancellation.is_cancelled() {
                return None;
            }
            if visited >= MAX_ENTRIES_PER_APPLICATION
                || budget.entries >= MAX_ENTRIES_PER_CAPTURE
                || Instant::now() >= budget.deadline
            {
                partial = true;
                break;
            }
            visited += 1;
            budget.entries += 1;
            // These are installation/update mechanisms, not application interaction evidence.
            let name = entry.name.to_string_lossy().to_ascii_lowercase();
            if name.contains("shipit")
                || name.contains("sparkle")
                || matches!(
                    name.as_str(),
                    "updater"
                        | "updates"
                        | "update"
                        | ".com.apple.containermanagerd.metadata.plist"
                )
            {
                continue;
            }
            // Container Library descendants can contain caches and logs; preserve the weaker
            // provenance throughout that subtree instead of promoting it to application data.
            let entry_source =
                if matches!(name.as_str(), "caches" | "logs" | "webkit" | "httpstorages") {
                    ApplicationDateSource::CacheModified
                } else {
                    source
                };
            let output = if entry_source == ApplicationDateSource::CacheModified {
                &mut weak
            } else {
                &mut strong
            };
            if entry.is_file || entry.is_directory {
                if let Some(timestamp) = entry.modified_at_ms {
                    let source = if entry.is_directory {
                        ApplicationDateSource::DirectoryModified
                    } else {
                        entry_source
                    };
                    if let Some(date) = ApplicationDate::new(timestamp, source, now) {
                        super::choose_latest(
                            if entry.is_directory {
                                &mut directories
                            } else {
                                output
                            },
                            date,
                        );
                    }
                }
                if entry.is_directory {
                    if depth < MAX_DEPTH {
                        stack.push((path.join(entry.name), entry_source, depth + 1));
                    } else {
                        partial = true;
                    }
                }
            }
        }
    }
    budget.partial |= partial;
    let mut result = strong.or(weak).or(directories);
    if let Some(date) = &mut result {
        date.partial = partial;
    }
    result
}

// Container opens can wait for a TCC response in the kernel for minutes, beyond any traversal
// deadline. System Shared File Lists cross the same Full Disk Access boundary but fail promptly
// without prompting. Unknown or missing probe files conservatively disable optional container reads.
fn protected_data_available(home: &Path) -> bool {
    use std::os::unix::fs::OpenOptionsExt;
    let root = home.join("Library/Application Support/com.apple.sharedfilelist");
    [
        "com.apple.LSSharedFileList.RecentApplications.sfl3",
        "com.apple.LSSharedFileList.RecentApplications.sfl2",
    ]
    .iter()
    .any(|name| {
        let path = root.join(name);
        let mut partial = false;
        safe_metadata(home, &path, &mut partial).is_some_and(|metadata| metadata.is_file())
            && fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)
                .is_ok()
    })
}

struct ActivityEntry {
    name: OsString,
    is_file: bool,
    is_directory: bool,
    modified_at_ms: Option<u64>,
}

/// Reuse the native metadata page parser instead of issuing a stat call for every path component
/// of every file. Unsupported volumes retain the same bounded, no-link portable fallback.
fn read_entries(
    path: &Path,
    limit: usize,
    budget: &mut ActivityBudget,
) -> std::io::Result<(Vec<ActivityEntry>, bool)> {
    let mut result = Vec::new();
    let native = BulkDirectory::open(path).and_then(|directory| loop {
        let page = directory.read_page(&mut budget.buffer)?;
        budget.native_pages += 1;
        if page.is_empty() {
            return Ok(true);
        }
        for entry in page {
            if result.len() >= limit {
                return Ok(false);
            }
            if entry.attribute_error != 0 {
                return Err(std::io::Error::from_raw_os_error(
                    entry.attribute_error as i32,
                ));
            }
            result.push(ActivityEntry {
                name: entry.name,
                is_file: entry.object_type == VNODE_TYPE_REGULAR_FILE,
                is_directory: entry.object_type == VNODE_TYPE_DIRECTORY,
                modified_at_ms: entry.modified_at_ms,
            });
        }
    });
    match native {
        Ok(complete) => return Ok((result, complete)),
        Err(error) => {
            budget.portable_directories += 1;
            if budget.portable_directories <= 3 {
                log::debug!("application_date_activity_fallback path={} stage=bulk_metadata native_code={:?} native_error={} outcome=bounded_portable_read", crate::diagnostics::text(&path.display()), error.raw_os_error(), crate::diagnostics::text(&error));
            }
        }
    }
    // A failed O_NOFOLLOW open must never be retried by following the same final symlink.
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "directory became a symbolic link",
        ));
    }
    result.clear();
    let mut complete = true;
    for entry in fs::read_dir(path)? {
        if result.len() >= limit {
            complete = false;
            break;
        }
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        result.push(ActivityEntry {
            name: entry.file_name(),
            is_file: metadata.is_file() && !metadata.file_type().is_symlink(),
            is_directory: metadata.is_dir() && !metadata.file_type().is_symlink(),
            modified_at_ms: metadata.modified().ok().and_then(super::unix_millis),
        });
    }
    Ok((result, complete))
}

fn observe(
    home: &Path,
    path: &Path,
    source: ApplicationDateSource,
    now: u64,
    files: &mut Option<ApplicationDate>,
    directories: &mut Option<ApplicationDate>,
    partial: &mut bool,
) -> Option<fs::Metadata> {
    let metadata = safe_metadata(home, path, partial)?;
    if !metadata.is_file() && !metadata.is_dir() {
        return None;
    }
    if let Some(timestamp) = metadata.modified().ok().and_then(super::unix_millis) {
        let source = if metadata.is_dir() {
            ApplicationDateSource::DirectoryModified
        } else {
            source
        };
        if let Some(date) = ApplicationDate::new(timestamp, source, now) {
            super::choose_latest(
                if metadata.is_dir() {
                    directories
                } else {
                    files
                },
                date,
            );
        }
    }
    Some(metadata)
}

pub(super) fn safe_metadata(root: &Path, path: &Path, partial: &mut bool) -> Option<fs::Metadata> {
    let relative = path.strip_prefix(root).ok()?;
    let mut current = PathBuf::from(root);
    let mut result = None;
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return None;
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if !metadata.file_type().is_symlink() => result = Some(metadata),
            Ok(_) => return None,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    *partial = true;
                }
                return None;
            }
        }
    }
    result
}

fn exact_host_domain(name: &str, identifier: &str) -> bool {
    let Some(host) = name
        .strip_prefix(identifier)
        .and_then(|value| value.strip_prefix('.'))
        .and_then(|value| value.strip_suffix(".plist"))
    else {
        return false;
    };
    host.len() == 36
        && host.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_domains_exclude_updater_prefixes() {
        let uuid = "12345678-1234-1234-1234-123456789abc";
        assert!(exact_host_domain(
            &format!("com.example.App.{uuid}.plist"),
            "com.example.App"
        ));
        assert!(!exact_host_domain(
            &format!("com.example.App.ShipIt.{uuid}.plist"),
            "com.example.App"
        ));
    }

    #[test]
    fn filesystem_estimates_do_not_follow_links_or_shared_vendor_roots() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("Library/Preferences")).unwrap();
        fs::write(other.path().join("secret"), b"unrelated").unwrap();
        std::os::unix::fs::symlink(
            other.path().join("secret"),
            root.path()
                .join("Library/Preferences/com.example.App.plist"),
        )
        .unwrap();
        let cancellation = PlatformCancellation::new(|| false);
        assert!(estimate_in(
            root.path(),
            "com.example.App",
            u64::MAX,
            &cancellation,
            &mut ActivityBudget::new()
        )
        .is_none());
        assert!(estimate_in(
            root.path(),
            "../Preferences",
            u64::MAX,
            &cancellation,
            &mut ActivityBudget::new()
        )
        .is_none());
    }

    #[test]
    fn nested_data_keeps_its_source_and_is_refreshed_without_identity_changes() {
        let root = tempfile::tempdir().unwrap();
        let directory = root
            .path()
            .join("Library/Application Support/com.example.App/nested");
        fs::create_dir_all(&directory).unwrap();
        let file = directory.join("state");
        fs::write(&file, b"state").unwrap();
        let set_time = |seconds| {
            fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_times(
                    fs::FileTimes::new()
                        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(seconds)),
                )
                .unwrap()
        };
        set_time(100);
        let capture = || {
            estimate_in(
                root.path(),
                "com.example.App",
                u64::MAX,
                &PlatformCancellation::new(|| false),
                &mut ActivityBudget::new(),
            )
            .unwrap()
        };
        let before = capture();
        assert_eq!(
            before.source,
            ApplicationDateSource::ApplicationDataModified
        );
        assert_eq!(before.timestamp_ms, 100_000);
        set_time(200);
        assert_eq!(capture().timestamp_ms, 200_000);
    }

    #[test]
    fn container_cache_descendants_do_not_override_preference_evidence() {
        let root = tempfile::tempdir().unwrap();
        let prefs = root.path().join("Library/Preferences");
        let cache = root
            .path()
            .join("Library/Containers/com.example.App/Data/Library/Caches/nested");
        fs::create_dir_all(&prefs).unwrap();
        fs::create_dir_all(&cache).unwrap();
        fs::write(prefs.join("com.example.App.plist"), b"prefs").unwrap();
        fs::write(cache.join("cache"), b"cache").unwrap();
        let mut budget = ActivityBudget::new();
        budget.protected_data_available = Some(true);
        let date = estimate_in(
            root.path(),
            "com.example.App",
            u64::MAX,
            &PlatformCancellation::new(|| false),
            &mut budget,
        )
        .unwrap();
        assert_eq!(date.source, ApplicationDateSource::PreferencesModified);
    }

    #[test]
    fn protected_container_reads_require_a_successful_noninteractive_probe() {
        let root = tempfile::tempdir().unwrap();
        let directory = root
            .path()
            .join("Library/Containers/com.example.App/Data/Library");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("state"), b"state").unwrap();
        assert!(estimate_in(
            root.path(),
            "com.example.App",
            u64::MAX,
            &PlatformCancellation::new(|| false),
            &mut ActivityBudget::new()
        )
        .is_none());
    }

    #[test]
    fn bounded_walk_returns_cheap_evidence_with_partial_provenance() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("Library/Preferences")).unwrap();
        fs::write(
            root.path()
                .join("Library/Preferences/com.example.App.plist"),
            b"not parsed",
        )
        .unwrap();
        fs::create_dir_all(
            root.path()
                .join("Library/Application Support/com.example.App"),
        )
        .unwrap();
        let mut budget = ActivityBudget::new();
        budget.deadline = Instant::now();
        let date = estimate_in(
            root.path(),
            "com.example.App",
            u64::MAX,
            &PlatformCancellation::new(|| false),
            &mut budget,
        )
        .unwrap();
        assert_eq!(date.source, ApplicationDateSource::PreferencesModified);
        assert!(date.partial);
        assert!(budget.partial);
    }
}
