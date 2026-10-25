mod config;
#[cfg(test)]
mod tests;

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use mangodisk_platform::{current_platform, Platform};

use super::{ModelCandidate, ModelLayout, ModelRoot, ModelSnapshot, ProviderSpec};
use crate::{
    cleanup::{
        source_selection::SourceScope, CleanupActionKind, CleanupActionReason, CleanupActionResult,
        CleanupActionStatus, ScanItemStatus, ScanRuleResult,
    },
    filesystem::{
        metadata::modified_ms,
        permanent_delete::{
            delete_directory_tree_permanently_with_cancellation, delete_path_permanently,
            physical_path_identity_snapshot, prepare_path_for_permanent_delete,
            PhysicalPathIdentity,
        },
    },
    shared::operation::OperationGuard,
};

pub(super) const CLEANER_ID: &str = "special.ai-model-comfyui";
pub(super) const CLEANER_REVISION: &str = "comfyui-models-v3";
const MAX_ENTRIES: usize = 50_000;
const MAX_MODELS: usize = 10_000;
const MAX_DEPTH: usize = 16;
const SPEC: ProviderSpec = ProviderSpec {
    id: CLEANER_ID,
    root: ModelRoot::Home(&[]),
    layout: ModelLayout::DirectFiles,
    required_stopped_processes: &[],
};
static PREVIEW: OnceLock<Mutex<Option<Preview>>> = OnceLock::new();

#[derive(Clone)]
struct ModelEvidence {
    model: ModelCandidate,
    identity: PhysicalPathIdentity,
    root: config::ModelRoot,
    root_identity: PhysicalPathIdentity,
    is_package: bool,
    file_identities: Vec<PhysicalPathIdentity>,
}

#[derive(Clone)]
struct Preview {
    home: PathBuf,
    projects: Vec<PathBuf>,
    models: Vec<ModelEvidence>,
}

struct WalkBudget<'a> {
    entries: usize,
    models: usize,
    deadline: Instant,
    cancelled: &'a (dyn Fn() -> bool + Sync),
}

impl WalkBudget<'_> {
    fn visit(&mut self, depth: usize) -> Result<(), String> {
        self.entries += 1;
        if (self.cancelled)() {
            return Err("cancelled".into());
        }
        if depth > MAX_DEPTH
            || self.entries > MAX_ENTRIES
            || self.models >= MAX_MODELS
            || Instant::now() >= self.deadline
        {
            return Err("model traversal budget exhausted".into());
        }
        Ok(())
    }
}

pub(super) fn preview(
    home: &Path,
    project_roots: &[String],
    cancelled: &(dyn Fn() -> bool + Sync),
    report_path: &(dyn Fn(&Path) + Sync),
    report_files: &(dyn Fn(&Path, u64, u64) + Sync),
) -> ScanRuleResult {
    let mut projects = super::super::project_root_index::load().unwrap_or_else(|error| {
        log::warn!("comfyui_project_index_unavailable stage=load native_error={} outcome=configured_roots_only", mangodisk_platform::diagnostics::text(&error));
        Vec::new()
    });
    projects.extend(project_roots.iter().map(PathBuf::from));
    projects.sort();
    projects.dedup();
    let (rule, preview) = preview_in(home, projects, cancelled, report_path, report_files);
    set_preview(preview);
    rule
}

fn preview_in(
    home: &Path,
    projects: Vec<PathBuf>,
    cancelled: &(dyn Fn() -> bool + Sync),
    report_path: &(dyn Fn(&Path) + Sync),
    report_files: &(dyn Fn(&Path, u64, u64) + Sync),
) -> (ScanRuleResult, Option<Preview>) {
    let started = Instant::now();
    let discovery = config::discover(home, &projects, cancelled);
    let mut partial = discovery.partial;
    let mut models = Vec::new();
    let mut identities = HashSet::new();
    let mut visited = HashSet::new();
    let mut budget = WalkBudget {
        entries: 0,
        models: 0,
        deadline: Instant::now() + Duration::from_secs(3),
        cancelled,
    };
    // Visit pipeline-only roots first so a general parent cannot classify their
    // incomplete packages as individual weights before traversal deduplication.
    for root in discovery
        .roots
        .iter()
        .filter(|root| root.diffusers)
        .chain(discovery.roots.iter().filter(|root| !root.diffusers))
    {
        if models.iter().any(|model: &ModelEvidence| {
            model.is_package
                && current_platform()
                    .relative_path(&root.path, &model.model.path)
                    .is_some()
        }) {
            continue;
        }
        match scan_root(root, &mut budget, &mut visited) {
            Ok(found) => {
                for model in found {
                    if identities.insert(model.identity) {
                        report_path(&model.model.path);
                        models.push(model);
                    }
                }
            }
            Err(error) => {
                partial = true;
                log::warn!("comfyui_model_scan_incomplete path={} stage=enumerate_models native_error={} outcome=limited", mangodisk_platform::diagnostics::text(&root.path.display()), mangodisk_platform::diagnostics::text(&error));
            }
        }
    }
    let package_files = models
        .iter()
        .filter(|model| model.is_package)
        .flat_map(|model| model.file_identities.iter().copied())
        .collect::<HashSet<_>>();
    models.retain(|model| model.is_package || !package_files.contains(&model.identity));
    for model in &models {
        report_files(&model.model.path, model.model.file_count, model.model.bytes);
    }
    let mut rule = super::model_rule(
        &SPEC,
        models.iter().map(|e| e.model.clone()).collect(),
        Vec::new(),
        started.elapsed().as_millis() as u64,
    );
    let preview = if partial {
        rule.status = ScanItemStatus::Limited;
        rule.selectable = false;
        None
    } else {
        if discovery.roots.is_empty() {
            rule.status = ScanItemStatus::NotApplicable;
            rule.available = false;
        }
        Some(Preview {
            home: home.to_path_buf(),
            projects,
            models,
        })
    };
    log::info!("comfyui_model_scan_finished root_count={} model_count={} visited_entries={} partial={} elapsed_ms={}", discovery.roots.len(), rule.source_count, budget.entries, partial, started.elapsed().as_millis());
    (rule, preview)
}

pub(super) fn limited() -> ScanRuleResult {
    set_preview(None);
    super::unavailable_rule(&SPEC, ScanItemStatus::Limited, 0)
}

fn set_preview(value: Option<Preview>) {
    if let Ok(mut slot) = PREVIEW.get_or_init(|| Mutex::new(None)).lock() {
        *slot = value;
    }
}

fn scan_root(
    root: &config::ModelRoot,
    budget: &mut WalkBudget<'_>,
    visited: &mut HashSet<PhysicalPathIdentity>,
) -> Result<Vec<ModelEvidence>, String> {
    current_platform()
        .validate_path_no_links(&root.path)
        .map_err(|error| error.to_string())?;
    let root_identity =
        physical_path_identity_snapshot(&root.path).map_err(|error| error.to_string())?;
    if visited.contains(&root_identity) {
        return Ok(Vec::new());
    }
    if pipeline_directory(&root.path)? {
        let (model, identity, file_identities) = capture_model(&root.path, budget)?;
        budget.models += 1;
        visited.insert(root_identity);
        return Ok(vec![ModelEvidence {
            model,
            identity,
            root: root.clone(),
            root_identity,
            is_package: true,
            file_identities,
        }]);
    }
    let mut stack = vec![(root.path.clone(), 0, root.diffusers)];
    let mut result = Vec::new();
    while let Some((directory, depth, diffusers)) = stack.pop() {
        budget.visit(depth)?;
        let directory_identity =
            physical_path_identity_snapshot(&directory).map_err(|error| error.to_string())?;
        if !visited.insert(directory_identity) {
            continue;
        }
        for entry in fs::read_dir(&directory).map_err(|error| error.to_string())? {
            budget.visit(depth)?;
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if excluded_name(&name) {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
            if current_platform().is_link_like(&metadata) {
                continue;
            }
            if metadata.is_dir() {
                if pipeline_directory(&path)? {
                    let (model, identity, file_identities) = capture_model(&path, budget)?;
                    budget.models += 1;
                    result.push(ModelEvidence {
                        model,
                        identity,
                        root: root.clone(),
                        root_identity,
                        is_package: true,
                        file_identities,
                    });
                } else {
                    stack.push((path, depth + 1, diffusers || name == "diffusers"));
                }
            } else if metadata.is_file() && !diffusers && weight_file(&path) {
                let (model, identity, file_identities) = capture_model(&path, budget)?;
                budget.models += 1;
                result.push(ModelEvidence {
                    model,
                    identity,
                    root: root.clone(),
                    root_identity,
                    is_package: false,
                    file_identities,
                });
            }
        }
    }
    Ok(result)
}

fn excluded_name(name: &str) -> bool {
    name.starts_with('.')
        || [
            ".part",
            ".partial",
            ".download",
            ".crdownload",
            ".tmp",
            ".incomplete",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
        || matches!(
            name,
            "custom_nodes"
                | "input"
                | "output"
                | "outputs"
                | "user"
                | "workflows"
                | "datasets"
                | "training"
                | "temp"
                | "__pycache__"
                | "put_checkpoints_here"
                | "put_loras_here"
        )
}

fn weight_file(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            [
                "safetensors",
                "sft",
                "ckpt",
                "pt",
                "pt2",
                "pth",
                "bin",
                "pkl",
                "gguf",
                "onnx",
            ]
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

fn pipeline_directory(path: &Path) -> Result<bool, String> {
    let marker = path.join("model_index.json");
    let metadata = match fs::symlink_metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_file()
        || current_platform().is_link_like(&metadata)
        || metadata.len() > 64 * 1024
    {
        return Err("unsupported pipeline model index".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(&marker)
        .and_then(|file| file.take(64 * 1024 + 1).read_to_end(&mut bytes))
        .map_err(|error| error.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("pipeline index exceeds read limit".into());
    }
    let valid = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .is_some_and(|value| {
            value
                .get("_class_name")
                .is_some_and(serde_json::Value::is_string)
        });
    if valid {
        Ok(true)
    } else {
        Err("invalid pipeline model index".into())
    }
}

fn capture_model(
    path: &Path,
    budget: &mut WalkBudget<'_>,
) -> Result<
    (
        ModelCandidate,
        PhysicalPathIdentity,
        Vec<PhysicalPathIdentity>,
    ),
    String,
> {
    let mut file_identities = Vec::new();
    let (snapshot, identity) = snapshot(path, budget, 0, &mut file_identities)?;
    Ok((
        ModelCandidate {
            path: path.to_path_buf(),
            bytes: snapshot.bytes,
            file_count: snapshot.file_count,
            modified_at_ms: snapshot.modified_at_ms,
            fingerprint: snapshot.fingerprint,
        },
        identity,
        file_identities,
    ))
}

fn snapshot(
    path: &Path,
    budget: &mut WalkBudget<'_>,
    depth: usize,
    file_identities: &mut Vec<PhysicalPathIdentity>,
) -> Result<(ModelSnapshot, PhysicalPathIdentity), String> {
    budget.visit(depth)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if current_platform().is_link_like(&metadata) {
        return Err("model contains a link or reparse point".into());
    }
    let identity = physical_path_identity_snapshot(path).map_err(|error| error.to_string())?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(super::metadata_entry_fingerprint(path, &metadata, None)?.as_bytes());
    hasher.update(format!("{identity:?}").as_bytes());
    let mut result = ModelSnapshot::default();
    if metadata.is_file() {
        file_identities.push(identity);
        result.bytes = metadata.len();
        result.file_count = 1;
        result.modified_at_ms = modified_ms(&metadata);
    } else if metadata.is_dir() {
        let mut children = Vec::new();
        for child in fs::read_dir(path).map_err(|error| error.to_string())? {
            budget.visit(depth)?;
            children.push(child.map_err(|error| error.to_string())?);
        }
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let name = child.file_name().to_string_lossy().to_ascii_lowercase();
            // Finder can add this regular metadata file when users browse a model.
            // It belongs to the selected package; directories and links stay excluded.
            let finder_metadata = name == ".ds_store"
                && child
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_file();
            if excluded_name(&name) && !finder_metadata {
                return Err("model package contains unrelated or temporary data".into());
            }
            let (child, _) = snapshot(&child.path(), budget, depth + 1, file_identities)?;
            hasher.update(child.fingerprint.as_bytes());
            result.bytes = result.bytes.saturating_add(child.bytes);
            result.file_count += child.file_count;
            result.modified_at_ms = super::latest(result.modified_at_ms, child.modified_at_ms);
        }
    } else {
        return Err("model contains a special file".into());
    }
    result.fingerprint = hasher.finalize().to_hex().to_string();
    Ok((result, identity))
}

pub(super) fn execute(
    scope: Option<&SourceScope>,
    dry_run: bool,
    operation: &OperationGuard,
) -> CleanupActionResult {
    let preview = PREVIEW
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|slot| slot.clone());
    let Some(preview) = preview else {
        return super::failed_action(CLEANER_ID, 0, CleanupActionReason::PreflightFailed);
    };
    execute_preview(&preview, scope, dry_run, operation)
}

fn execute_preview(
    preview: &Preview,
    scope: Option<&SourceScope>,
    dry_run: bool,
    operation: &OperationGuard,
) -> CleanupActionResult {
    if scope.is_some_and(|scope| {
        scope
            .validate_known_paths(preview.models.iter().map(|e| e.model.path.as_path()))
            .is_err()
    }) {
        return super::failed_action(CLEANER_ID, 0, CleanupActionReason::PreflightFailed);
    }
    let selected = preview
        .models
        .iter()
        .filter(|e| scope.is_none_or(|scope| scope.selects(&e.model.path)))
        .collect::<Vec<_>>();
    let expected_bytes = selected.iter().map(|e| e.model.bytes).sum();
    let discovery = config::discover(&preview.home, &preview.projects, &|| {
        operation.ensure_not_cancelled().is_err()
    });
    if discovery.partial {
        return super::failed_action(
            CLEANER_ID,
            expected_bytes,
            CleanupActionReason::PreflightFailed,
        );
    }
    let roots = discovery
        .roots
        .iter()
        .map(|root| (current_platform().path_identity_key(&root.path), root))
        .collect::<HashMap<_, _>>();
    let mut released = 0;
    let mut affected = 0;
    let mut failed = 0;
    let mut deleted = HashSet::new();
    for evidence in selected {
        let result: Result<_, String> = (|| {
            operation
                .ensure_not_cancelled()
                .map_err(|error| error.to_string())?;
            let key = current_platform().path_identity_key(&evidence.root.path);
            if roots
                .get(&key)
                .is_none_or(|root| root.diffusers != evidence.root.diffusers)
            {
                return Err("model root is no longer configured".into());
            }
            if physical_path_identity_snapshot(&evidence.root.path)
                .map_err(|error| error.to_string())?
                != evidence.root_identity
            {
                return Err("model root changed after preview".into());
            }
            let prepared = prepare_path_for_permanent_delete(&evidence.model.path)
                .map_err(|error| error.to_string())?;
            let mut budget = WalkBudget {
                entries: 0,
                models: 0,
                deadline: Instant::now() + Duration::from_secs(3),
                cancelled: &|| operation.ensure_not_cancelled().is_err(),
            };
            let (current, identity, _) = capture_model(&evidence.model.path, &mut budget)?;
            if identity != evidence.identity
                || prepared.identity() != evidence.identity
                || current != evidence.model
            {
                return Err("model changed after preview".into());
            }
            operation
                .ensure_not_cancelled()
                .map_err(|error| error.to_string())?;
            Ok(prepared)
        })();
        match result {
            Ok(_) if dry_run => {
                affected += evidence.model.file_count;
            }
            Ok(prepared) => {
                let removed = if evidence.is_package {
                    delete_directory_tree_permanently_with_cancellation(
                        prepared,
                        evidence.model.bytes,
                        evidence.model.file_count,
                        &|| operation.ensure_not_cancelled().is_err(),
                    )
                    .map(|outcome| (outcome.released_bytes(), outcome.affected_item_count()))
                } else {
                    delete_path_permanently(
                        prepared,
                        evidence.model.bytes,
                        evidence.model.file_count,
                    )
                    .map(|()| (evidence.model.bytes, evidence.model.file_count))
                };
                match removed {
                    Ok((bytes, files)) => {
                        released += bytes;
                        affected += files;
                        deleted.insert(evidence.model.path.clone());
                    }
                    Err(error) => {
                        released += error.released_bytes();
                        affected += error.affected_item_count();
                        failed += 1;
                        log::warn!("comfyui_model_cleanup_failed operation_id={} path={} stage={} native_code={:?} partial={} released_bytes={} affected_files={} native_error={} outcome=remainder_retained", operation.id(), mangodisk_platform::diagnostics::text(&evidence.model.path.display()), error.stage(), error.native_code(), error.is_partial(), error.released_bytes(), error.affected_item_count(), mangodisk_platform::diagnostics::text(&error));
                    }
                }
            }
            Err(error) => {
                failed += 1;
                log::warn!("comfyui_model_cleanup_failed operation_id={} path={} stage=revalidate native_error={} outcome=skipped", operation.id(), mangodisk_platform::diagnostics::text(&evidence.model.path.display()), mangodisk_platform::diagnostics::text(&error));
            }
        }
    }
    if !deleted.is_empty() {
        let mut remaining = preview.clone();
        remaining
            .models
            .retain(|e| !deleted.contains(&e.model.path));
        set_preview(Some(remaining));
    }
    CleanupActionResult {
        rule_id: CLEANER_ID.into(),
        action_kind: CleanupActionKind::Delete,
        status: if failed > 0 {
            if released > 0 || affected > 0 {
                CleanupActionStatus::Partial
            } else {
                CleanupActionStatus::Failed
            }
        } else if dry_run {
            CleanupActionStatus::Previewed
        } else {
            CleanupActionStatus::Completed
        },
        reason_code: (failed > 0).then_some(CleanupActionReason::ItemsSkipped),
        bytes_expected: expected_bytes,
        released_bytes: released,
        affected_item_count: affected,
        failed_item_count: failed,
        running_processes: Vec::new(),
    }
}
