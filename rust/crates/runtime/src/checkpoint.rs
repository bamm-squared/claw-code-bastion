//! Trusted, task-scoped snapshots of candidate proposal state.

use crate::snapshot::CandidateChangeSetId;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_CHECKPOINTS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateCheckpoint {
    pub id: String,
    pub candidate_id: String,
    pub lineage: String,
    pub reason: String,
    pub created_at_ms: u128,
    pub bytes: u64,
    digest: [u8; 32],
    path: PathBuf,
}

#[derive(Debug)]
pub struct CandidateCheckpointStore {
    lineage: String,
    root: PathBuf,
    checkpoints: Vec<CandidateCheckpoint>,
}

impl CandidateCheckpointStore {
    pub fn new(lineage: impl Into<String>, _private_mode: bool) -> io::Result<Self> {
        let root = std::env::temp_dir().join(format!("claw-checkpoints-{}", unique_stamp()));
        fs::create_dir_all(&root)?;
        Ok(Self {
            lineage: lineage.into(),
            root,
            checkpoints: Vec::new(),
        })
    }

    pub fn create(
        &mut self,
        candidate_root: &Path,
        candidate_id: CandidateChangeSetId,
        reason: impl Into<String>,
    ) -> io::Result<CandidateCheckpoint> {
        let candidate_id = candidate_id.to_string();
        if let Some(existing) = self
            .checkpoints
            .iter()
            .find(|checkpoint| checkpoint.candidate_id == candidate_id)
        {
            return Ok(existing.clone());
        }
        let id = format!("checkpoint-{}", unique_stamp());
        let staged = self.root.join(format!("{id}.staged"));
        let path = self.root.join(&id);
        fs::create_dir_all(&staged)?;
        if let Err(error) = copy_tree(candidate_root, &staged) {
            let _ = fs::remove_dir_all(&staged);
            return Err(error);
        }
        let digest = tree_digest(&staged)?;
        let bytes = tree_size(&staged)?;
        fs::rename(&staged, &path)?;
        let checkpoint = CandidateCheckpoint {
            id,
            candidate_id,
            lineage: self.lineage.clone(),
            reason: reason.into(),
            created_at_ms: now_ms(),
            bytes,
            digest,
            path,
        };
        self.checkpoints.push(checkpoint.clone());
        while self.checkpoints.len() > MAX_CHECKPOINTS {
            let removed = self.checkpoints.remove(0);
            let _ = fs::remove_dir_all(removed.path);
        }
        Ok(checkpoint)
    }

    #[must_use]
    pub fn list(&self) -> &[CandidateCheckpoint] {
        &self.checkpoints
    }

    pub fn restore(&self, id: &str, candidate_root: &Path) -> io::Result<()> {
        let checkpoint = self
            .checkpoints
            .iter()
            .find(|checkpoint| checkpoint.id == id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "checkpoint not found"))?;
        if checkpoint.lineage != self.lineage || tree_digest(&checkpoint.path)? != checkpoint.digest
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "checkpoint identity or contents are invalid",
            ));
        }
        let backup = self.root.join(format!("{id}.restore-backup"));
        fs::create_dir_all(&backup)?;
        move_contents(candidate_root, &backup)?;
        if let Err(error) = copy_tree(&checkpoint.path, candidate_root) {
            let _ = clear_contents(candidate_root);
            let _ = move_contents(&backup, candidate_root);
            let _ = fs::remove_dir_all(&backup);
            return Err(error);
        }
        fs::remove_dir_all(backup)
    }

    pub fn clear(&mut self) {
        for checkpoint in self.checkpoints.drain(..) {
            let _ = fs::remove_dir_all(checkpoint.path);
        }
    }
}

impl Drop for CandidateCheckpointStore {
    fn drop(&mut self) {
        self.clear();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
    }
    Ok(())
}

fn copy_entry(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        copy_tree(source, destination)
    } else if metadata.file_type().is_symlink() {
        let target = fs::read_link(source)?;
        validate_link_target(source.parent().unwrap_or(Path::new("")), &target)?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, destination)?;
        #[cfg(not(unix))]
        return Err(io::Error::other("symlink checkpointing unsupported"));
        Ok(())
    } else if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination)?;
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported special file in checkpoint",
        ))
    }
}

fn clear_contents(root: &Path) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_name() != ".git" {
            remove_entry(&entry.path())?;
        }
    }
    Ok(())
}

fn move_contents(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_name() != ".git" {
            fs::rename(entry.path(), destination.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn remove_entry(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn validate_link_target(parent: &Path, target: &Path) -> io::Result<()> {
    if target.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "absolute checkpoint symlink",
        ));
    }
    let mut depth = 0_i32;
    for component in parent.join(target).components() {
        match component {
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            _ => {}
        }
        if depth < 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "checkpoint symlink escapes root",
            ));
        }
    }
    Ok(())
}

fn tree_digest(root: &Path) -> io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    let mut paths = BTreeSet::new();
    collect_paths(root, root, &mut paths)?;
    for relative in paths {
        let path = root.join(&relative);
        hasher.update(relative.to_string_lossy().as_bytes());
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            hasher.update([1]);
        } else if metadata.file_type().is_symlink() {
            hasher.update([2]);
            hasher.update(fs::read_link(path)?.to_string_lossy().as_bytes());
        } else {
            hasher.update([0]);
            hasher.update(fs::read(path)?);
        }
    }
    Ok(hasher.finalize().into())
}

fn collect_paths(root: &Path, current: &Path, paths: &mut BTreeSet<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(io::Error::other)?;
        paths.insert(relative.to_path_buf());
        if entry.file_type()?.is_dir() {
            collect_paths(root, &path, paths)?;
        }
    }
    Ok(())
}

fn tree_size(root: &Path) -> io::Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        total += if metadata.is_dir() {
            tree_size(&entry.path())?
        } else {
            metadata.len()
        };
    }
    Ok(total)
}

fn unique_stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("claw-checkpoint-test-{name}-{}", unique_stamp()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn restore_reconciles_added_and_deleted_files() {
        let root = temp_root("restore");
        let candidate = root.join("candidate");
        fs::create_dir_all(&candidate).unwrap();
        fs::write(candidate.join("kept.txt"), "a").unwrap();
        let mut store = CandidateCheckpointStore::new("task-a", false).unwrap();
        let checkpoint = store
            .create(&candidate, CandidateChangeSetId::new([1; 32]), "rework")
            .unwrap();
        fs::write(candidate.join("kept.txt"), "b").unwrap();
        fs::write(candidate.join("new.txt"), "new").unwrap();
        fs::remove_file(candidate.join("kept.txt")).unwrap();
        store.restore(&checkpoint.id, &candidate).unwrap();
        assert_eq!(fs::read_to_string(candidate.join("kept.txt")).unwrap(), "a");
        assert!(!candidate.join("new.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retention_and_corruption_are_safe() {
        let root = temp_root("retention");
        let candidate = root.join("candidate");
        fs::create_dir_all(&candidate).unwrap();
        let mut store = CandidateCheckpointStore::new("task-a", false).unwrap();
        for index in 0..4 {
            fs::write(candidate.join("state.txt"), index.to_string()).unwrap();
            store
                .create(&candidate, CandidateChangeSetId::new([index; 32]), "rework")
                .unwrap();
        }
        assert_eq!(store.list().len(), MAX_CHECKPOINTS);
        let checkpoint = store.list()[0].clone();
        fs::write(checkpoint.path.join("state.txt"), "corrupt").unwrap();
        assert!(store.restore(&checkpoint.id, &candidate).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
