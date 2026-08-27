//! Trusted staging and candidate-change handling for isolated tasks.
//!
//! The candidate is hostile after the worker starts. Its Git metadata is never
//! consulted by the trusted host.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalWorkspace {
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UntrustedCandidate {
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedBaseline {
    pub root: PathBuf,
    pub manifest: BaselineManifest,
}

#[derive(Debug)]
pub struct IsolatedWorkspace {
    pub canonical: CanonicalWorkspace,
    pub baseline: TrustedBaseline,
    pub candidate: UntrustedCandidate,
    pub task_root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineEntry {
    pub relative: PathBuf,
    pub kind: EntryKind,
    pub hash: Option<[u8; 32]>,
    pub size: u64,
    pub executable: bool,
    pub symlink_target: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaselineManifest {
    pub entries: BTreeMap<PathBuf, BaselineEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateChangeSetId([u8; 32]);

impl CandidateChangeSetId {
    #[must_use]
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn zero() -> Self {
        Self([0_u8; 32])
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Default for CandidateChangeSetId {
    fn default() -> Self {
        Self::zero()
    }
}

impl fmt::Display for CandidateChangeSetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateChange {
    Add {
        candidate: BaselineEntry,
    },
    Modify {
        baseline: BaselineEntry,
        candidate: BaselineEntry,
    },
    Delete {
        baseline: BaselineEntry,
    },
}

impl CandidateChange {
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Add { candidate } | Self::Modify { candidate, .. } => &candidate.relative,
            Self::Delete { baseline } => &baseline.relative,
        }
    }

    #[must_use]
    pub fn higher_risk(&self) -> bool {
        let path = self.path();
        let text = path.to_string_lossy();
        let sensitive_component = path.components().any(|component| {
            matches!(component, Component::Normal(value) if matches!(
                value.to_str(),
                Some("auth" | "crypto" | "payments" | "deployment" | "scripts" | "install")
            ))
        });
        sensitive_component
            || text.starts_with(".github/")
            || text.starts_with(".gitlab/")
            || matches!(
                text.as_ref(),
                "Cargo.toml"
                    | "Cargo.lock"
                    | "package.json"
                    | "package-lock.json"
                    | "pnpm-lock.yaml"
                    | "yarn.lock"
                    | "pyproject.toml"
                    | "build.rs"
                    | "Makefile"
                    | "Dockerfile"
                    | "Containerfile"
            )
            || matches!(
                self,
                Self::Add { candidate } | Self::Modify { candidate, .. }
                    if candidate.kind == EntryKind::Symlink || candidate.executable
            )
    }

    #[must_use]
    const fn kind_rank(&self) -> u8 {
        match self {
            Self::Add { .. } => 0,
            Self::Modify { .. } => 1,
            Self::Delete { .. } => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CandidateChangeSet {
    pub id: CandidateChangeSetId,
    pub source: Option<CandidateChangeSetId>,
    pub changes: Vec<CandidateChange>,
}

impl CandidateChangeSet {
    #[must_use]
    pub fn new(changes: Vec<CandidateChange>) -> Self {
        Self::new_with_source(changes, None)
    }

    #[must_use]
    pub fn new_with_source(
        mut changes: Vec<CandidateChange>,
        source: Option<CandidateChangeSetId>,
    ) -> Self {
        changes.sort_by(|left, right| {
            (left.path().to_owned(), left.kind_rank())
                .cmp(&(right.path().to_owned(), right.kind_rank()))
        });
        let id = change_set_id(&changes, source);
        Self {
            id,
            source,
            changes,
        }
    }

    #[must_use]
    pub fn with_selected_changes(&self, mut keep: impl FnMut(&CandidateChange) -> bool) -> Self {
        let selected = self
            .changes
            .iter()
            .filter(|change| keep(change))
            .cloned()
            .collect::<Vec<_>>();
        Self::new_with_source(selected, Some(self.id))
    }
}

fn change_set_id(
    changes: &[CandidateChange],
    source: Option<CandidateChangeSetId>,
) -> CandidateChangeSetId {
    let mut hasher = Sha256::new();
    if let Some(source_id) = source {
        hasher.update(source_id.as_bytes());
    }
    for change in changes {
        match change {
            CandidateChange::Add { candidate } => {
                hasher.update(b"A");
                hash_entry(&mut hasher, candidate);
            }
            CandidateChange::Modify {
                baseline,
                candidate,
            } => {
                hasher.update(b"M");
                hash_entry(&mut hasher, baseline);
                hash_entry(&mut hasher, candidate);
            }
            CandidateChange::Delete { baseline } => {
                hasher.update(b"D");
                hash_entry(&mut hasher, baseline);
            }
        }
    }
    CandidateChangeSetId::new(hasher.finalize().into())
}

fn hash_entry(hasher: &mut Sha256, entry: &BaselineEntry) {
    hasher.update(entry.relative.to_string_lossy().as_bytes());
    hasher.update([match entry.kind {
        EntryKind::File => 0,
        EntryKind::Directory => 1,
        EntryKind::Symlink => 2,
    }]);
    hasher.update(entry.size.to_le_bytes());
    hasher.update([u8::from(entry.executable)]);
    if let Some(hash) = entry.hash {
        hasher.update(hash);
    }
    if let Some(target) = &entry.symlink_target {
        hasher.update(target.to_string_lossy().as_bytes());
    }
}

/// Render bounded review text from trusted metadata. This deliberately does
/// not invoke Git, diff drivers, textconv, hooks, or any candidate command.
#[must_use]
pub fn render_change_summary(set: &CandidateChangeSet, max_bytes: usize) -> String {
    let mut output = String::new();
    for change in &set.changes {
        let line = match change {
            CandidateChange::Add { candidate } => format!(
                "ADD {} ({:?}, {} bytes){}\n",
                candidate.relative.display(),
                candidate.kind,
                candidate.size,
                if change.higher_risk() {
                    " [HIGHER RISK]"
                } else {
                    ""
                }
            ),
            CandidateChange::Modify {
                baseline,
                candidate,
            } => format!(
                "MODIFY {} ({:?} {} -> {} bytes){}\n",
                candidate.relative.display(),
                candidate.kind,
                baseline.size,
                candidate.size,
                if change.higher_risk() {
                    " [HIGHER RISK]"
                } else {
                    ""
                }
            ),
            CandidateChange::Delete { baseline } => format!(
                "DELETE {} ({:?}, {} bytes){}\n",
                baseline.relative.display(),
                baseline.kind,
                baseline.size,
                if change.higher_risk() {
                    " [HIGHER RISK]"
                } else {
                    ""
                }
            ),
        };
        if output.len().saturating_add(line.len()) > max_bytes {
            output.push_str("... review summary truncated ...\n");
            break;
        }
        output.push_str(&line);
    }
    output
}

/// Create independent baseline and candidate copies. Only the candidate is
/// intended for worker mounting; the baseline is trusted and never exposed.
pub fn create_disposable_snapshot(source: &Path) -> io::Result<IsolatedWorkspace> {
    let canonical = source.canonicalize()?;
    let task_root = std::env::temp_dir().join(format!("claw-task-{}", unique_stamp()));
    let baseline_root = task_root.join("baseline");
    let candidate_root = task_root.join("candidate");
    fs::create_dir_all(&baseline_root)?;
    fs::create_dir_all(&candidate_root)?;

    let result = (|| {
        let selected = git_files(&canonical).unwrap_or_default();
        if selected.is_empty() {
            copy_tree(&canonical, &baseline_root, &canonical)?;
            copy_tree(&canonical, &candidate_root, &canonical)?;
        } else {
            for relative in selected {
                if is_git_path(&relative) {
                    continue;
                }
                copy_entry(
                    &canonical.join(&relative),
                    &baseline_root.join(&relative),
                    &canonical,
                )?;
                copy_entry(
                    &canonical.join(&relative),
                    &candidate_root.join(&relative),
                    &canonical,
                )?;
            }
        }
        let manifest = build_manifest(&baseline_root)?;
        let _ = initialize_git(&candidate_root);
        Ok(IsolatedWorkspace {
            canonical: CanonicalWorkspace { root: canonical },
            baseline: TrustedBaseline {
                root: baseline_root,
                manifest,
            },
            candidate: UntrustedCandidate {
                root: candidate_root,
            },
            task_root: task_root.clone(),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&task_root);
    }
    result
}

impl IsolatedWorkspace {
    pub fn scan(&self) -> io::Result<CandidateChangeSet> {
        scan_candidate(&self.baseline, &self.candidate)
    }

    /// Cleanup is against only the host-created task root. `remove_dir_all` does
    /// not dereference symlinks in the tree.
    pub fn discard(&self) -> io::Result<()> {
        fs::remove_dir_all(&self.task_root)
    }
}

pub fn scan_candidate(
    baseline: &TrustedBaseline,
    candidate: &UntrustedCandidate,
) -> io::Result<CandidateChangeSet> {
    let candidate_manifest = build_manifest(&candidate.root)?;
    let paths: BTreeSet<_> = baseline
        .manifest
        .entries
        .keys()
        .chain(candidate_manifest.entries.keys())
        .cloned()
        .collect();
    let mut changes = Vec::new();
    for path in paths {
        if is_git_path(&path) {
            continue;
        }
        match (
            baseline.manifest.entries.get(&path),
            candidate_manifest.entries.get(&path),
        ) {
            (None, Some(entry)) => changes.push(CandidateChange::Add {
                candidate: entry.clone(),
            }),
            (Some(entry), None) => changes.push(CandidateChange::Delete {
                baseline: entry.clone(),
            }),
            (Some(old), Some(new)) if old != new => changes.push(CandidateChange::Modify {
                baseline: old.clone(),
                candidate: new.clone(),
            }),
            _ => {}
        }
    }
    Ok(CandidateChangeSet::new(changes))
}

/// Apply a previously reviewed change set. The caller is the trusted UI/CLI;
/// no worker message or project configuration can invoke this function.
pub fn apply_approved_changes(
    set: &CandidateChangeSet,
    workspace: &CanonicalWorkspace,
    baseline: &TrustedBaseline,
    candidate: &UntrustedCandidate,
) -> io::Result<()> {
    if set.source.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "partial change-set application is not enabled in this release",
        ));
    }
    for change in &set.changes {
        let path = validate_relative_path(workspace, change.path())?;
        if is_git_path(change.path()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "authoritative Git metadata cannot be changed",
            ));
        }
        match change {
            CandidateChange::Delete { baseline: expected } => {
                verify_current(&path, expected)?;
                remove_non_directory(&path)?;
            }
            CandidateChange::Add {
                candidate: expected,
            } => {
                if fs::symlink_metadata(&path).is_ok() {
                    return Err(conflict("destination was created outside this task"));
                }
                let source = candidate.root.join(&expected.relative);
                verify_candidate(&source, expected)?;
                install_entry(&source, &path, expected, &workspace.root)?;
            }
            CandidateChange::Modify {
                baseline: expected,
                candidate: replacement,
            } => {
                verify_current(&path, expected)?;
                let source = candidate.root.join(&replacement.relative);
                verify_candidate(&source, replacement)?;
                if expected.kind != replacement.kind {
                    return Err(conflict("object type changed; automatic apply is disabled"));
                }
                install_entry(&source, &path, replacement, &workspace.root)?;
            }
        }
    }
    let _ = baseline;
    Ok(())
}

fn validate_relative_path(workspace: &CanonicalWorkspace, relative: &Path) -> io::Result<PathBuf> {
    if relative.is_absolute()
        || relative.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "path must be relative to workspace",
        ));
    }
    if relative.as_os_str().is_empty() || is_git_path(relative) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid authoritative workspace path",
        ));
    }
    let root = workspace.root.canonicalize()?;
    let destination = root.join(relative);
    let mut parent = destination
        .parent()
        .ok_or_else(|| io::Error::other("missing parent"))?;
    while !parent.exists() {
        parent = parent
            .parent()
            .ok_or_else(|| io::Error::other("workspace parent missing"))?;
    }
    let resolved_parent = parent.canonicalize()?;
    if !resolved_parent.starts_with(&root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "path escapes workspace",
        ));
    }
    Ok(destination)
}

fn verify_current(path: &Path, expected: &BaselineEntry) -> io::Result<()> {
    if entry_for(path, &expected.relative)? != *expected {
        return Err(conflict("canonical workspace changed since task baseline"));
    }
    Ok(())
}

fn verify_candidate(path: &Path, expected: &BaselineEntry) -> io::Result<()> {
    if entry_for(path, &expected.relative)? != *expected {
        return Err(conflict("candidate changed after review"));
    }
    Ok(())
}

fn conflict(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, message)
}

fn install_entry(
    source: &Path,
    destination: &Path,
    entry: &BaselineEntry,
    workspace_root: &Path,
) -> io::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    match entry.kind {
        EntryKind::File => {
            let name = destination
                .file_name()
                .ok_or_else(|| io::Error::other("missing filename"))?
                .to_string_lossy();
            let temporary =
                destination.with_file_name(format!(".claw-apply-{name}-{}", unique_stamp()));
            let mut input = File::open(source)?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            io::copy(&mut input, &mut output)?;
            output.flush()?;
            output.sync_all()?;
            set_executable(&temporary, entry.executable)?;
            fs::rename(temporary, destination)?;
        }
        EntryKind::Symlink => {
            let target = fs::read_link(source)?;
            let parent = destination
                .parent()
                .ok_or_else(|| io::Error::other("missing parent"))?;
            let root = workspace_root.canonicalize()?;
            validate_link_target(parent, &target, &root)?;
            if fs::symlink_metadata(destination).is_ok() {
                fs::remove_file(destination)?;
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, destination)?;
            #[cfg(not(unix))]
            return Err(io::Error::other(
                "symlink apply unsupported on this platform",
            ));
        }
        EntryKind::Directory => fs::create_dir_all(destination)?,
    }
    Ok(())
}

fn remove_non_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        return Err(conflict(
            "directory removal requires an empty implied directory",
        ));
    }
    fs::remove_file(path)
}

fn build_manifest(root: &Path) -> io::Result<BaselineManifest> {
    let mut entries = BTreeMap::new();
    for item in WalkDir::new(root).follow_links(false) {
        let item = item.map_err(|e| io::Error::other(e.to_string()))?;
        let relative = item
            .path()
            .strip_prefix(root)
            .map_err(io::Error::other)?
            .to_path_buf();
        if relative.as_os_str().is_empty() || is_git_path(&relative) {
            continue;
        }
        let entry = entry_for(item.path(), &relative)?;
        entries.insert(relative, entry);
    }
    Ok(BaselineManifest { entries })
}

fn entry_for(path: &Path, relative: &Path) -> io::Result<BaselineEntry> {
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    let (kind, hash, size, target) = if file_type.is_symlink() {
        (EntryKind::Symlink, None, 0, Some(fs::read_link(path)?))
    } else if file_type.is_file() {
        (
            EntryKind::File,
            Some(hash_file(path)?),
            metadata.len(),
            None,
        )
    } else if file_type.is_dir() {
        (EntryKind::Directory, None, 0, None)
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported special file: {}", path.display()),
        ));
    };
    Ok(BaselineEntry {
        relative: relative.to_path_buf(),
        kind,
        hash,
        size,
        executable: file_type.is_file() && is_executable(&metadata),
        symlink_target: target,
    })
}

fn hash_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

fn copy_tree(source: &Path, destination: &Path, root: &Path) -> io::Result<()> {
    for item in WalkDir::new(source).follow_links(false) {
        let item = item.map_err(|e| io::Error::other(e.to_string()))?;
        let relative = item.path().strip_prefix(source).map_err(io::Error::other)?;
        if relative.as_os_str().is_empty() || is_git_path(relative) {
            continue;
        }
        copy_entry(item.path(), &destination.join(relative), root)?;
    }
    Ok(())
}

fn copy_entry(source: &Path, destination: &Path, root: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        fs::create_dir_all(destination)?;
    } else if metadata.file_type().is_symlink() {
        let target = fs::read_link(source)?;
        validate_link_target(source.parent().unwrap_or(root), &target, root)?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, destination)?;
        #[cfg(not(unix))]
        return Err(io::Error::other(
            "symlink staging unsupported on this platform",
        ));
    } else if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination)?;
        set_executable(destination, is_executable(&metadata))?;
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported special file in snapshot",
        ));
    }
    Ok(())
}

fn validate_link_target(link_parent: &Path, target: &Path, root: &Path) -> io::Result<()> {
    if target.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "absolute symlink target is outside snapshot policy",
        ));
    }
    let relative_parent = link_parent.strip_prefix(root).unwrap_or(Path::new(""));
    let mut depth = 0_i32;
    for component in relative_parent.join(target).components() {
        match component {
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            _ => {}
        }
        if depth < 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "symlink target escapes project",
            ));
        }
    }
    Ok(())
}

fn is_git_path(path: &Path) -> bool {
    path.components()
        .next()
        .is_some_and(|component| matches!(component, Component::Normal(value) if value == ".git"))
}
fn unique_stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

fn git_files(source: &Path) -> io::Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .args([
            "-C",
            source.to_string_lossy().as_ref(),
            "ls-files",
            "-co",
            "--exclude-standard",
            "-z",
        ])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("git file enumeration failed"));
    }
    Ok(output
        .stdout
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(path_from_git_bytes)
        .collect())
}

fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(OsString::from_vec(bytes.to_vec()))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

fn initialize_git(root: &Path) -> bool {
    Command::new("git")
        .args(["-C", root.to_string_lossy().as_ref(), "init"])
        .output()
        .is_ok_and(|o| o.status.success())
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}
#[cfg(not(unix))]
fn is_executable(_: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    let mode = if executable {
        permissions.mode() | 0o111
    } else {
        permissions.mode() & !0o111
    };
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions)
}
#[cfg(not(unix))]
fn set_executable(_: &Path, _: bool) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("claw-{name}-{}", unique_stamp()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn candidate_git_metadata_is_ignored() {
        let root = temp("snapshot");
        fs::write(root.join("a.txt"), "a").unwrap();
        let task = create_disposable_snapshot(&root).unwrap();
        fs::create_dir_all(task.candidate.root.join(".git/hooks")).unwrap();
        fs::write(task.candidate.root.join(".git/config"), "malicious").unwrap();
        fs::write(task.candidate.root.join("a.txt"), "b").unwrap();
        let changes = task.scan().unwrap();
        assert_eq!(changes.changes.len(), 1);
        assert_eq!(changes.changes[0].path(), Path::new("a.txt"));
        task.discard().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn apply_refuses_canonical_concurrent_change() {
        let root = temp("apply");
        fs::write(root.join("a.txt"), "a").unwrap();
        let task = create_disposable_snapshot(&root).unwrap();
        fs::write(task.candidate.root.join("a.txt"), "b").unwrap();
        let set = task.scan().unwrap();
        fs::write(root.join("a.txt"), "user change").unwrap();
        assert!(
            apply_approved_changes(&set, &task.canonical, &task.baseline, &task.candidate).is_err()
        );
        task.discard().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn apply_rechecks_reviewed_candidate_hash() {
        let root = temp("review-hash");
        fs::write(root.join("a.txt"), "a").unwrap();
        let task = create_disposable_snapshot(&root).unwrap();
        fs::write(task.candidate.root.join("a.txt"), "reviewed").unwrap();
        let set = task.scan().unwrap();
        fs::write(task.candidate.root.join("a.txt"), "changed after review").unwrap();
        assert!(
            apply_approved_changes(&set, &task.canonical, &task.baseline, &task.candidate).is_err()
        );
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "a");
        task.discard().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn add_and_delete_are_structured_changes() {
        let root = temp("add-delete");
        fs::write(root.join("old.txt"), "old").unwrap();
        let task = create_disposable_snapshot(&root).unwrap();
        fs::remove_file(task.candidate.root.join("old.txt")).unwrap();
        fs::write(task.candidate.root.join("new.txt"), "new").unwrap();
        let set = task.scan().unwrap();
        assert!(set
            .changes
            .iter()
            .any(|change| matches!(change, CandidateChange::Add { .. })));
        assert!(set
            .changes
            .iter()
            .any(|change| matches!(change, CandidateChange::Delete { .. })));
        task.discard().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn external_symlink_is_refused() {
        let root = temp("symlink");
        let outside = root.with_extension("secret");
        fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
        assert!(create_disposable_snapshot(&root).is_err());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(outside);
    }

    #[test]
    fn candidate_change_set_has_stable_identity() {
        let root = temp("change-set-id");
        fs::write(root.join("a.txt"), "a").unwrap();
        let task = create_disposable_snapshot(&root).unwrap();
        fs::write(task.candidate.root.join("a.txt"), "b").unwrap();
        let first = task.scan().unwrap();
        let second = task.scan().unwrap();
        assert_eq!(
            first.id, second.id,
            "scans without mutation should retain identity"
        );
        assert_eq!(first.source, None);
        assert_ne!(first.id, CandidateChangeSetId::zero());
        assert_ne!(
            first.id,
            second
                .with_selected_changes(|change| change.path() == Path::new("does-not-exist"))
                .id
        );
        task.discard().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn candidate_selection_records_parent_identity() {
        let root = temp("selected-changes");
        fs::write(root.join("a.txt"), "a").unwrap();
        let task = create_disposable_snapshot(&root).unwrap();
        fs::write(task.candidate.root.join("a.txt"), "b").unwrap();
        fs::write(task.candidate.root.join("b.txt"), "x").unwrap();
        let full = task.scan().unwrap();
        let partial =
            full.with_selected_changes(|change| !matches!(change, CandidateChange::Delete { .. }));
        assert_eq!(partial.source, Some(full.id));
        assert!(!partial.changes.is_empty());
        assert_ne!(partial.id, full.id);
        task.discard().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
