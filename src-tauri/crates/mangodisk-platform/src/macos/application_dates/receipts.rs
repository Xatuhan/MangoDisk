use std::{
    fs,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use plist::Value;

use crate::{
    run_controlled_command, ApplicationDate, ApplicationDateSource, ApplicationDateTarget,
    ControlledCommandLimits, ControlledEnvironmentPolicy, ControlledExecutable,
    PlatformCancellation,
};

struct Receipt {
    identifier: String,
    version: String,
    prefix: PathBuf,
    date: u64,
    bom: PathBuf,
}

pub(super) struct ReceiptIndex {
    receipts: Vec<Receipt>,
    deadline: Instant,
    commands: usize,
}

impl ReceiptIndex {
    pub(super) fn read(cancellation: &PlatformCancellation) -> Self {
        let mut result = Self {
            receipts: Vec::new(),
            deadline: Instant::now() + Duration::from_millis(300),
            commands: 0,
        };
        let Ok(entries) = fs::read_dir("/var/db/receipts") else {
            return result;
        };
        for entry in entries.take(4096).flatten() {
            if cancellation.is_cancelled() || Instant::now() >= result.deadline {
                break;
            }
            let path = entry.path();
            if path.extension().is_none_or(|value| value != "plist") {
                continue;
            }
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > 64 * 1024
            {
                continue;
            }
            let Some(values) = Value::from_file(&path)
                .ok()
                .and_then(Value::into_dictionary)
            else {
                continue;
            };
            let Some(identifier) = values.get("PackageIdentifier").and_then(Value::as_string)
            else {
                continue;
            };
            let Some(version) = values.get("PackageVersion").and_then(Value::as_string) else {
                continue;
            };
            let Some(date) = values
                .get("InstallDate")
                .and_then(Value::as_date)
                .and_then(|value| super::unix_millis(SystemTime::from(value)))
            else {
                continue;
            };
            let prefix = values
                .get("InstallPrefixPath")
                .and_then(Value::as_string)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"));
            result.receipts.push(Receipt {
                identifier: identifier.to_owned(),
                version: version.to_owned(),
                prefix,
                date,
                bom: path.with_extension("bom"),
            });
        }
        result
    }

    pub(super) fn installation(
        &mut self,
        target: &ApplicationDateTarget,
        now: u64,
        cancellation: &PlatformCancellation,
    ) -> Option<ApplicationDate> {
        if self.commands >= 4 || cancellation.is_cancelled() {
            return None;
        }
        let version = target.version.as_deref()?;
        let identifier_prefix = format!("{}.", target.identifier);
        let candidates = self.receipts.iter().filter(|receipt| {
            receipt.version == version
                && (receipt.identifier == target.identifier
                    || receipt.identifier.starts_with(&identifier_prefix))
        });
        let mut result: Option<ApplicationDate> = None;
        let mut partial = false;
        for receipt in candidates {
            if Instant::now() >= self.deadline || self.commands >= 4 || cancellation.is_cancelled()
            {
                partial = true;
                break;
            }
            let executable = match ControlledExecutable::capture(Path::new("/usr/bin/lsbom")) {
                Ok(executable) => executable,
                Err(error) => {
                    log::debug!("application_date_receipt_incomplete path={} stage=capture_executable native_error={} outcome=verified_evidence_retained", crate::diagnostics::text(&receipt.bom.display()), error.as_str());
                    partial = true;
                    break;
                }
            };
            let Some(bom) = receipt.bom.to_str() else {
                partial = true;
                continue;
            };
            self.commands += 1;
            let arguments = ["-s", "-f", bom];
            let output = run_controlled_command(
                "application_date_receipt_payload",
                &executable,
                &arguments,
                ControlledEnvironmentPolicy::Inherit,
                ControlledCommandLimits {
                    timeout: Duration::from_millis(150),
                    stdout_bytes: 2 * 1024 * 1024,
                    stderr_bytes: 1024,
                },
                &|| cancellation.is_cancelled(),
            );
            // One unreadable or oversized candidate must not erase another receipt's
            // verified evidence, or prevent checking the remaining bounded candidates.
            let output = match output {
                Ok(output) => output,
                Err(error) => {
                    log::debug!("application_date_receipt_incomplete path={} stage=read_payload native_error={} outcome=verified_evidence_retained", crate::diagnostics::text(&receipt.bom.display()), error.as_str());
                    partial = true;
                    continue;
                }
            };
            if !output.status.success() {
                partial = true;
                continue;
            }
            let owns_bundle = String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| owns_info(&receipt.prefix, line, &target.path));
            if owns_bundle {
                if let Some(date) =
                    ApplicationDate::new(receipt.date, ApplicationDateSource::PackageReceipt, now)
                {
                    super::choose_latest(&mut result, date);
                }
            }
        }
        if let Some(date) = &mut result {
            date.partial = partial;
        }
        result
    }
}

fn owns_info(prefix: &Path, entry: &str, bundle: &Path) -> bool {
    let mut normalized = PathBuf::from(prefix);
    if !prefix.is_absolute() {
        return false;
    }
    for component in Path::new(entry).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => normalized.push(value),
            _ => return false,
        }
    }
    normalized == bundle.join("Contents/Info.plist")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_later_receipt_preserves_verified_installation_evidence() {
        let root = tempfile::tempdir().unwrap();
        let make_bom = |name: &str, extra_files: usize| {
            let list = root.path().join(format!("{name}.txt"));
            let bom = root.path().join(format!("{name}.bom"));
            let mut contents = String::from(".\t40755\t0/0\n./Applications\t40755\t0/0\n./Applications/Example.app\t40755\t0/0\n./Applications/Example.app/Contents\t40755\t0/0\n./Applications/Example.app/Contents/Info.plist\t100644\t0/0\t1\t0\n");
            for index in 0..extra_files {
                contents.push_str(&format!(
                    "./Applications/Example.app/Contents/{index:05}{}\t100644\t0/0\t1\t0\n",
                    "x".repeat(220)
                ));
            }
            fs::write(&list, contents).unwrap();
            let output = std::process::Command::new("/usr/bin/mkbom")
                .args(["-i"])
                .arg(list)
                .arg(&bom)
                .output()
                .unwrap();
            assert!(output.status.success());
            assert!(output.stderr.is_empty());
            bom
        };
        let valid = make_bom("valid", 0);
        let oversized = make_bom("oversized", 9000);
        let target = ApplicationDateTarget {
            catalog_identifier: "example".into(),
            identifier: "com.example.App".into(),
            path: PathBuf::from("/Applications/Example.app"),
            version: Some("1".into()),
            identity_is_unique: true,
        };
        let mut index = ReceiptIndex {
            receipts: [valid, oversized]
                .into_iter()
                .map(|bom| Receipt {
                    identifier: target.identifier.clone(),
                    version: "1".into(),
                    prefix: PathBuf::from("/"),
                    date: 100,
                    bom,
                })
                .collect(),
            deadline: Instant::now() + Duration::from_secs(5),
            commands: 0,
        };
        let date = index.installation(&target, 200, &PlatformCancellation::new(|| false));
        assert_eq!(index.commands, 2);
        let date = date.expect("the earlier verified receipt remains available");
        assert_eq!(date.timestamp_ms, 100);
        assert!(date.partial);
        index.receipts.reverse();
        index.commands = 0;
        let date = index
            .installation(&target, 200, &PlatformCancellation::new(|| false))
            .expect("a failed receipt does not prevent checking the next candidate");
        assert_eq!(index.commands, 2);
        assert_eq!(date.timestamp_ms, 100);
        assert!(date.partial);
    }

    #[test]
    fn receipt_payload_requires_the_exact_bundle_info_path() {
        let bundle = Path::new("/Applications/Example.app");
        assert!(owns_info(
            Path::new("/"),
            "./Applications/Example.app/Contents/Info.plist",
            bundle
        ));
        assert!(!owns_info(
            Path::new("/"),
            "./Applications/Other.app/Contents/Info.plist",
            bundle
        ));
        assert!(!owns_info(
            Path::new("/Applications"),
            "../Applications/Example.app/Contents/Info.plist",
            bundle
        ));
        assert!(!owns_info(
            Path::new("/"),
            "/Applications/Example.app/Contents/Info.plist",
            bundle
        ));
    }
}
