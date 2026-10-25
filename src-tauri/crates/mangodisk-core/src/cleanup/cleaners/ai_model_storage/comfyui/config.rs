use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use crate::filesystem::permanent_delete::{physical_path_identity_snapshot, PhysicalPathIdentity};
use mangodisk_platform::{current_platform, Platform};
use serde_json::Value;

const MAX_CONFIG_BYTES: u64 = 256 * 1024;
const MAX_TOTAL_CONFIG_BYTES: usize = 2 * 1024 * 1024;
const MAX_ROOTS: usize = 256;
const MAX_INSTALLATIONS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ModelRoot {
    pub path: PathBuf,
    pub diffusers: bool,
}

#[derive(Default)]
pub(super) struct Discovery {
    pub roots: Vec<ModelRoot>,
    pub installations: Vec<PathBuf>,
    pub partial: bool,
}

struct Reader<'a> {
    home: &'a Path,
    cancelled: &'a (dyn Fn() -> bool + Sync),
    seen_configs: HashSet<PathBuf>,
    seen_installs: HashSet<PathBuf>,
    protected: HashSet<PhysicalPathIdentity>,
    bytes: usize,
    result: Discovery,
}

pub(super) fn discover(
    home: &Path,
    projects: &[PathBuf],
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Discovery {
    let mut reader = Reader {
        home,
        cancelled,
        seen_configs: HashSet::new(),
        seen_installs: HashSet::new(),
        protected: protected_identities(home),
        bytes: 0,
        result: Discovery::default(),
    };
    // The current desktop stores shared roots separately from installation records. Legacy
    // Desktop uses basePath and a different YAML file; both may coexist after migration.
    for directory in configuration_directories(home) {
        if let Some(settings) = reader.json(&directory.join("settings.json")) {
            reader.root_array(settings.get("modelsDirs"));
        }
        if let Some(records) = reader.json(&directory.join("installations.json")) {
            if let Some(records) = records.as_array() {
                if records.len() > MAX_INSTALLATIONS {
                    reader.result.partial = true;
                }
                for record in records.iter().take(MAX_INSTALLATIONS) {
                    if let Some(path) = record.get("installPath").and_then(Value::as_str) {
                        reader.installation(path, true);
                    }
                    reader.root_array(record.get("modelDirs"));
                }
            } else {
                reader.result.partial = true;
            }
        }
        if let Some(settings) = reader.json(&directory.join("config.json")) {
            if let Some(path) = settings.get("basePath").and_then(Value::as_str) {
                reader.installation(path, true);
            }
        }
        for name in ["extra_models_config.yaml", "shared_model_paths.yaml"] {
            reader.yaml(&directory.join(name));
        }
    }
    reader.add_root(home.join("ComfyUI-Shared/models"), false);
    // Windows installs on the system drive may keep large data in LocalAppData instead.
    #[cfg(windows)]
    {
        let root = user_data_directory(home, "LOCALAPPDATA", "AppData/Local").join("Comfy-Desktop");
        reader.add_root(root.join("ComfyUI-Shared/models"), false);
        reader.installation_children(&root.join("ComfyUI-Installs"));
    }
    reader.installation_children(&home.join("ComfyUI-Installs"));
    for path in [home.join("ComfyUI"), home.join("Documents/ComfyUI")] {
        reader.manual_installation(&path);
    }
    // Project discovery has already populated this index in the same scan. Reusing those
    // exact roots avoids another recursive home/volume scan for manually installed copies.
    for project in projects.iter().take(10_000) {
        if cancelled() {
            reader.result.partial = true;
            break;
        }
        reader.manual_installation(project);
        reader.manual_installation(&project.join("ComfyUI"));
    }
    reader
        .result
        .roots
        .sort_by_key(|root| current_platform().path_identity_key(&root.path));
    reader.result.roots.dedup_by(|left, right| {
        if current_platform().paths_equal(&left.path, &right.path) {
            // A pipeline-only category is stricter than a general model tree.
            right.diffusers |= left.diffusers;
            true
        } else {
            false
        }
    });
    reader.result
}

fn configuration_directories(home: &Path) -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = home.join("Library/Application Support");
    #[cfg(windows)]
    let base = user_data_directory(home, "APPDATA", "AppData/Roaming");
    #[cfg(not(any(windows, target_os = "macos")))]
    let base = user_data_directory(home, "XDG_CONFIG_HOME", ".config");
    vec![base.join("Comfy Desktop"), base.join("ComfyUI")]
}

#[cfg(not(target_os = "macos"))]
fn user_data_directory(home: &Path, variable: &str, fallback: &str) -> PathBuf {
    // An explicitly supplied home must not accidentally read another user's configuration.
    let current_home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    let same_user =
        current_home.is_some_and(|value| current_platform().paths_equal(home, Path::new(&value)));
    if same_user {
        if let Some(path) = std::env::var_os(variable)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
        {
            return path;
        }
    }
    home.join(fallback)
}

impl Reader<'_> {
    fn read(&mut self, path: &Path) -> Option<String> {
        if (self.cancelled)()
            || self.seen_configs.len() >= 512
            || self.bytes >= MAX_TOTAL_CONFIG_BYTES
        {
            self.result.partial = true;
            return None;
        }
        if !self.seen_configs.insert(path.to_path_buf()) {
            return None;
        }
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                self.failure(path, "metadata", &error);
                return None;
            }
        };
        if !metadata.is_file()
            || current_platform().is_link_like(&metadata)
            || metadata.len() > MAX_CONFIG_BYTES.min((MAX_TOTAL_CONFIG_BYTES - self.bytes) as u64)
            || current_platform().validate_path_no_links(path).is_err()
        {
            self.failure(path, "validate", "unsupported configuration path or size");
            return None;
        }
        let mut bytes = Vec::new();
        match fs::File::open(path)
            .and_then(|file| file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes))
        {
            Ok(_) if bytes.len() as u64 <= MAX_CONFIG_BYTES => {}
            Ok(_) => {
                self.failure(path, "read", "configuration exceeds read limit");
                return None;
            }
            Err(error) => {
                self.failure(path, "read", &error);
                return None;
            }
        }
        self.bytes += bytes.len();
        if self.bytes > MAX_TOTAL_CONFIG_BYTES {
            self.failure(path, "budget", "configuration budget exhausted");
            return None;
        }
        String::from_utf8(bytes)
            .map_err(|_| self.failure(path, "decode", "configuration is not UTF-8"))
            .ok()
    }

    fn json(&mut self, path: &Path) -> Option<Value> {
        let text = self.read(path)?;
        serde_json::from_str(&text)
            .map_err(|_| self.failure(path, "parse_json", "invalid JSON configuration"))
            .ok()
    }

    fn yaml(&mut self, path: &Path) {
        let Some(text) = self.read(path) else {
            return;
        };
        let options = serde_saphyr::options! {
            budget: serde_saphyr::budget! {
                max_depth: 8, max_events: 8192, max_nodes: 4096,
                max_aliases: 32, max_anchors: 32, max_documents: 1,
                max_recorded_anchor_events: 8192, max_recorded_anchor_bytes: 262144,
                max_total_scalar_bytes: 262144,
            },
        };
        let sections: BTreeMap<String, Option<BTreeMap<String, Value>>> =
            match serde_saphyr::from_str_with_options(&text, options) {
                Ok(sections) => sections,
                Err(_) => {
                    self.failure(
                        path,
                        "parse_yaml",
                        "invalid or oversized model path configuration",
                    );
                    return;
                }
            };
        let directory = path.parent().unwrap_or(self.home);
        for section in sections.into_values().flatten() {
            let base = match section.get("base_path") {
                Some(Value::String(value)) => resolve(value, directory, self.home),
                Some(_) => {
                    self.result.partial = true;
                    continue;
                }
                None => Some(directory.to_path_buf()),
            };
            let Some(base) = base else {
                self.result.partial = true;
                continue;
            };
            for (kind, value) in &section {
                if !model_category(kind) {
                    continue;
                }
                let Some(value) = value.as_str() else {
                    self.result.partial = true;
                    continue;
                };
                for entry in value.lines().filter(|entry| !entry.is_empty()) {
                    // ComfyUI expands base_path only. Preserve spaces and '#' in YAML paths.
                    let candidate = Path::new(entry);
                    let candidate = if candidate.is_absolute() {
                        candidate.to_path_buf()
                    } else {
                        base.join(candidate)
                    };
                    self.add_root(normalize(&candidate), kind == "diffusers");
                }
            }
        }
    }

    fn root_array(&mut self, value: Option<&Value>) {
        let Some(value) = value else {
            return;
        };
        let Some(values) = value.as_array() else {
            self.result.partial = true;
            return;
        };
        for value in values.iter().take(MAX_ROOTS + 1) {
            let Some(value) = value.as_str() else {
                self.result.partial = true;
                continue;
            };
            if let Some(path) = configured_path(value) {
                self.add_root(path, false);
            } else {
                self.result.partial = true;
            }
        }
        if values.len() > MAX_ROOTS {
            self.result.partial = true;
        }
    }

    fn installation(&mut self, value: &str, configured: bool) {
        let Some(path) = configured_path(value) else {
            self.result.partial = true;
            return;
        };
        if !self.seen_installs.insert(path.clone()) {
            return;
        }
        if self.seen_installs.len() > MAX_INSTALLATIONS {
            self.result.partial = true;
            return;
        }
        for root in [path.clone(), path.join("ComfyUI")] {
            if configured || is_installation(&root) {
                self.add_root(root.join("models"), false);
                self.yaml(&root.join("extra_model_paths.yaml"));
                self.result.installations.push(root);
            }
        }
    }

    fn manual_installation(&mut self, path: &Path) {
        if is_installation(path) {
            if let Some(path) = path.to_str() {
                self.installation(path, false);
            }
        }
    }

    fn installation_children(&mut self, parent: &Path) {
        if !parent.exists() {
            return;
        }
        if current_platform().validate_path_no_links(parent).is_err() {
            self.result.partial = true;
            return;
        }
        match fs::read_dir(parent) {
            Ok(entries) => {
                for (index, entry) in entries.enumerate() {
                    if index >= MAX_INSTALLATIONS || (self.cancelled)() {
                        self.result.partial = true;
                        break;
                    }
                    if let Ok(entry) = entry {
                        self.manual_installation(&entry.path());
                        self.manual_installation(&entry.path().join("ComfyUI"));
                    } else {
                        self.result.partial = true;
                    }
                }
            }
            Err(error) => self.failure(parent, "enumerate_installs", &error),
        }
    }

    fn add_root(&mut self, path: PathBuf, diffusers: bool) {
        if (self.cancelled)() {
            self.result.partial = true;
            return;
        }
        if self.result.roots.len() >= MAX_ROOTS {
            self.result.partial = true;
            return;
        }
        let path = normalize(&path);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                self.failure(&path, "root_metadata", &error);
                return;
            }
            Ok(metadata) if metadata.is_dir() && !current_platform().is_link_like(&metadata) => {}
            Ok(_) => {
                self.failure(
                    &path,
                    "validate_root",
                    "model root is not a regular directory",
                );
                return;
            }
        }
        if current_platform().validate_path_no_links(&path).is_err()
            || !safe_root(&path, self.home)
            || current_platform()
                .validate_user_selected_cleanup_root(&path)
                .is_err()
        {
            self.failure(&path, "validate_root", "unsafe or linked model directory");
            return;
        }
        let identity = match physical_path_identity_snapshot(&path) {
            Ok(identity) => identity,
            Err(error) => {
                self.failure(&path, "root_identity", &error);
                return;
            }
        };
        if self.protected.contains(&identity) {
            self.failure(
                &path,
                "root_scope",
                "model root aliases a protected user or volume directory",
            );
            return;
        }
        let diffusers = diffusers
            || path
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("diffusers"));
        self.result.roots.push(ModelRoot { path, diffusers });
    }

    fn failure(&mut self, path: &Path, stage: &str, error: &(impl std::fmt::Display + ?Sized)) {
        self.result.partial = true;
        log::warn!(
            "comfyui_model_discovery_incomplete path={} stage={} native_error={} outcome=limited",
            mangodisk_platform::diagnostics::text(&path.display()),
            stage,
            mangodisk_platform::diagnostics::text(error)
        );
    }
}

pub(super) fn safe_root(path: &Path, home: &Path) -> bool {
    path.is_absolute()
        && path.parent().is_some()
        && !home.starts_with(path)
        && !path
            .strip_prefix(home)
            .unwrap_or(path)
            .components()
            .any(|component| {
                let name = component.as_os_str().to_string_lossy().to_ascii_lowercase();
                matches!(
                    name.as_str(),
                    "input"
                        | "output"
                        | "outputs"
                        | "workflows"
                        | "datasets"
                        | "custom_nodes"
                        | ".venv"
                        | ".git"
                )
            })
        && !["Desktop", "Documents", "Downloads", "Library", "AppData"]
            .iter()
            .any(|name| current_platform().paths_equal(path, &home.join(name)))
        && !current_platform().is_protected_deletion_path(path)
}

fn is_installation(path: &Path) -> bool {
    [path.join("main.py"), path.join("folder_paths.py")]
        .iter()
        .all(|path| {
            fs::symlink_metadata(path).is_ok_and(|metadata| {
                metadata.is_file() && !current_platform().is_link_like(&metadata)
            })
        })
        && path.join("comfy").is_dir()
}

pub(super) fn model_category(kind: &str) -> bool {
    // Custom nodes register additional model types (for example ipadapter). The YAML is
    // explicit application configuration, but code, datasets, metadata, and user-data
    // categories never authorize weight discovery even when their values look like paths.
    !kind.is_empty()
        && kind.len() <= 64
        && kind
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && !matches!(
            kind,
            "base_path"
                | "is_default"
                | "custom_nodes"
                | "datasets"
                | "configs"
                | "input"
                | "output"
                | "outputs"
                | "user"
                | "workflows"
                | "download_model_base"
        )
}

fn configured_path(value: &str) -> Option<PathBuf> {
    // JSON paths are resolved by Electron against its working directory, which Finder and
    // shortcuts need not set to the user's home. Never guess a different deletion scope.
    let path = PathBuf::from(value);
    (path.is_absolute() && !value.chars().any(char::is_control)).then(|| normalize(&path))
}

fn protected_identities(home: &Path) -> HashSet<PhysicalPathIdentity> {
    // Case aliases on APFS/NTFS need not share a lexical spelling. Capture these few
    // boundary identities once, rather than canonicalizing every model or folding names
    // on volumes that may actually be case-sensitive.
    let mut paths = home.ancestors().map(Path::to_path_buf).collect::<Vec<_>>();
    paths.extend(
        [
            "Desktop",
            "Documents",
            "Downloads",
            "Library",
            "AppData",
            "Applications",
            "Pictures",
            "Movies",
            "Videos",
            "Music",
            "OneDrive",
        ]
        .iter()
        .map(|name| home.join(name)),
    );
    paths
        .into_iter()
        .filter_map(|path| physical_path_identity_snapshot(&path).ok())
        .collect()
}

fn resolve(value: &str, base: &Path, home: &Path) -> Option<PathBuf> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    let mut expanded = value.to_owned();
    if expanded == "~" || expanded.starts_with("~/") || expanded.starts_with("~\\") {
        expanded = home
            .join(expanded.get(2..).unwrap_or(""))
            .to_str()?
            .to_owned();
    }
    // Only substitute defined environment values. Unknown variables never become a relative
    // directory with a literal '$' or '%' spelling that could authorize an unrelated tree.
    let mut output = String::new();
    let mut chars = expanded.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' {
            let braced = chars.peek() == Some(&'{');
            if braced {
                chars.next();
            }
            let mut key = String::new();
            while chars
                .peek()
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            {
                key.push(chars.next()?);
            }
            if key.is_empty() || (braced && chars.next() != Some('}')) {
                return None;
            }
            output.push_str(&std::env::var(&key).ok()?);
        } else if cfg!(windows) && ch == '%' {
            let key = chars
                .by_ref()
                .take_while(|ch| *ch != '%')
                .collect::<String>();
            output.push_str(&std::env::var(key).ok()?);
        } else {
            output.push(ch);
        }
    }
    let path = PathBuf::from(output);
    Some(normalize(&if path.is_absolute() {
        path
    } else {
        base.join(path)
    }))
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            component => result.push(component.as_os_str()),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_configuration_read_budget_stops_further_file_reads() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().canonicalize().unwrap();
        let mut reader = Reader {
            home: &home,
            cancelled: &|| false,
            seen_configs: HashSet::new(),
            seen_installs: HashSet::new(),
            protected: protected_identities(&home),
            bytes: 0,
            result: Discovery::default(),
        };
        for index in 0..12 {
            let path = home.join(format!("config-{index}.yaml"));
            fs::write(&path, vec![b'#'; MAX_CONFIG_BYTES as usize]).unwrap();
            reader.read(&path);
        }
        assert!(reader.result.partial);
        assert!(
            reader.bytes <= MAX_TOTAL_CONFIG_BYTES,
            "later files must not continue consuming the exhausted read budget"
        );
    }
}
