use std::collections::HashMap;
use std::fs;
use std::path::Path;

use kittypaw_core::error::{KittypawError, Result};
use kittypaw_core::types::now_timestamp;
use kittypaw_core::workspace::{ChangeType, FileChange, FileChangeStatus, FileEntry, Workspace};
use similar::TextDiff;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::security::validate_path;

pub struct WorkspaceManager {
    workspaces: HashMap<String, Workspace>,
    /// Pending file changes keyed by change ID
    changes: HashMap<String, FileChange>,
}

impl WorkspaceManager {
    pub fn new() -> Self {
        Self {
            workspaces: HashMap::new(),
            changes: HashMap::new(),
        }
    }

    /// Validates `path` exists and is a directory, then registers it as a workspace.
    pub fn open(&mut self, path: &str) -> Result<Workspace> {
        let p = Path::new(path);
        if !p.exists() {
            return Err(KittypawError::Sandbox(format!(
                "Path does not exist: {path}"
            )));
        }
        if !p.is_dir() {
            return Err(KittypawError::Sandbox(format!(
                "Path is not a directory: {path}"
            )));
        }

        let canonical = p.canonicalize().map_err(KittypawError::Io)?;
        let root_path = canonical.to_string_lossy().to_string();

        // Return existing workspace if already opened for this path
        if let Some(existing) = self.workspaces.values().find(|w| w.root_path == root_path) {
            return Ok(existing.clone());
        }

        let id = Uuid::new_v4().to_string();
        let name = canonical
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root_path.clone());

        let now = now_timestamp();
        let ws = Workspace {
            id: id.clone(),
            name,
            root_path,
            created_at: now,
        };

        self.workspaces.insert(id, ws.clone());
        Ok(ws)
    }

    /// Lists all files and directories in the workspace (skips hidden entries).
    pub fn list_files(&self, workspace_id: &str) -> Result<Vec<FileEntry>> {
        let ws = self.get_workspace(workspace_id)?;
        let root = Path::new(&ws.root_path);

        let mut entries = Vec::new();

        for entry in WalkDir::new(root)
            .min_depth(1)
            .into_iter()
            .filter_entry(|e| !is_hidden(e))
            .filter_map(|e| e.ok())
        {
            let rel_path = entry
                .path()
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            let metadata = entry
                .metadata()
                .map_err(|e| KittypawError::Io(std::io::Error::other(e.to_string())))?;

            let modified = metadata
                .modified()
                .ok()
                .and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs().to_string())
                })
                .unwrap_or_default();

            entries.push(FileEntry {
                path: rel_path,
                size: metadata.len(),
                modified,
                is_dir: metadata.is_dir(),
            });
        }

        Ok(entries)
    }

    /// Reads the content of a file relative to the workspace root.
    pub fn read_file(&self, workspace_id: &str, rel_path: &str) -> Result<String> {
        let ws = self.get_workspace(workspace_id)?;
        let root = Path::new(&ws.root_path);
        let abs_path = validate_path(root, rel_path)?;
        fs::read_to_string(&abs_path).map_err(KittypawError::Io)
    }

    /// Creates a diff and returns a pending FileChange (does NOT write to disk).
    pub fn write_file(
        &mut self,
        workspace_id: &str,
        rel_path: &str,
        content: &str,
    ) -> Result<FileChange> {
        let ws = self.get_workspace(workspace_id)?.clone();
        let root = Path::new(&ws.root_path);
        let abs_path = validate_path(root, rel_path)?;

        let (old_content, change_type) = if abs_path.exists() {
            let old = fs::read_to_string(&abs_path).map_err(KittypawError::Io)?;
            (old, ChangeType::Modify)
        } else {
            (String::new(), ChangeType::Create)
        };

        let new_content_owned = content.to_string();
        let diff = TextDiff::from_lines(&old_content, &new_content_owned)
            .unified_diff()
            .header("a/original", "b/modified")
            .to_string();

        let change = FileChange {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.to_string(),
            path: rel_path.to_string(),
            change_type,
            diff,
            new_content: content.to_string(),
            status: FileChangeStatus::Pending,
        };

        self.changes.insert(change.id.clone(), change.clone());
        Ok(change)
    }

    /// Writes a pending FileChange to disk and marks it as Applied.
    pub fn apply_change(&mut self, change: &FileChange) -> Result<()> {
        let change_id = change.id.clone();
        let stored = self
            .changes
            .get(&change_id)
            .ok_or_else(|| KittypawError::Sandbox(format!("Change not found: {}", change_id)))?
            .clone();

        let ws = self.get_workspace(&stored.workspace_id)?;
        let root = ws.root_path.clone();

        let abs_path = validate_path(Path::new(&root), &stored.path)?;

        if let Some(parent) = abs_path.parent() {
            fs::create_dir_all(parent).map_err(KittypawError::Io)?;
        }
        fs::write(&abs_path, &stored.new_content).map_err(KittypawError::Io)?;

        if let Some(c) = self.changes.get_mut(&change_id) {
            c.status = FileChangeStatus::Applied;
        }
        Ok(())
    }

    /// Marks a pending change as Rejected without writing to disk.
    pub fn reject_change(&mut self, change_id: &str) -> Result<()> {
        let change = self
            .changes
            .get_mut(change_id)
            .ok_or_else(|| KittypawError::Sandbox(format!("Change not found: {change_id}")))?;
        change.status = FileChangeStatus::Rejected;
        Ok(())
    }

    // --- helpers ---

    fn get_workspace(&self, workspace_id: &str) -> Result<&Workspace> {
        self.workspaces
            .get(workspace_id)
            .ok_or_else(|| KittypawError::Sandbox(format!("Workspace not found: {workspace_id}")))
    }
}

impl Default for WorkspaceManager {
    fn default() -> Self {
        Self::new()
    }
}

fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| s.starts_with('.'))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_manager_with_temp() -> (WorkspaceManager, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = WorkspaceManager::new();
        mgr.open(dir.path().to_str().unwrap()).unwrap();
        (mgr, dir)
    }

    #[test]
    fn test_open_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = WorkspaceManager::new();
        let ws = mgr.open(dir.path().to_str().unwrap()).unwrap();
        assert!(!ws.id.is_empty());
        assert!(!ws.name.is_empty());
    }

    #[test]
    fn test_open_nonexistent_fails() {
        let mut mgr = WorkspaceManager::new();
        let result = mgr.open("/nonexistent/path/does/not/exist");
        assert!(result.is_err());
    }

    #[test]
    fn test_open_same_path_returns_same_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = WorkspaceManager::new();
        let ws1 = mgr.open(dir.path().to_str().unwrap()).unwrap();
        let ws2 = mgr.open(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(ws1.id, ws2.id);
    }

    #[test]
    fn test_list_files() {
        let (mut mgr, dir) = make_manager_with_temp();
        fs::write(dir.path().join("hello.txt"), "hello").unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();
        fs::write(dir.path().join("subdir/world.txt"), "world").unwrap();

        let ws = mgr.open(dir.path().to_str().unwrap()).unwrap();
        let files = mgr.list_files(&ws.id).unwrap();
        let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"hello.txt"));
        assert!(paths.contains(&"subdir"));
        assert!(paths.contains(&"subdir/world.txt"));
    }

    #[test]
    fn test_hidden_files_excluded() {
        let (mut mgr, dir) = make_manager_with_temp();
        fs::write(dir.path().join(".hidden"), "secret").unwrap();
        fs::write(dir.path().join("visible.txt"), "visible").unwrap();

        let ws = mgr.open(dir.path().to_str().unwrap()).unwrap();
        let files = mgr.list_files(&ws.id).unwrap();
        let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert!(!paths.contains(&".hidden"));
        assert!(paths.contains(&"visible.txt"));
    }

    #[test]
    fn test_read_file() {
        let (mut mgr, dir) = make_manager_with_temp();
        fs::write(dir.path().join("test.txt"), "content here").unwrap();

        let ws = mgr.open(dir.path().to_str().unwrap()).unwrap();
        let content = mgr.read_file(&ws.id, "test.txt").unwrap();
        assert_eq!(content, "content here");
    }

    #[test]
    fn test_write_file_creates_pending_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = WorkspaceManager::new();
        let ws = mgr.open(dir.path().to_str().unwrap()).unwrap();

        let change = mgr.write_file(&ws.id, "new.txt", "new content").unwrap();
        assert!(matches!(change.status, FileChangeStatus::Pending));
        assert!(matches!(change.change_type, ChangeType::Create));
        assert_eq!(change.new_content, "new content");
        // File should NOT exist yet
        assert!(!dir.path().join("new.txt").exists());
    }

    #[test]
    fn test_apply_change_writes_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = WorkspaceManager::new();
        let ws = mgr.open(dir.path().to_str().unwrap()).unwrap();

        let change = mgr.write_file(&ws.id, "applied.txt", "applied").unwrap();
        mgr.apply_change(&change).unwrap();

        let content = fs::read_to_string(dir.path().join("applied.txt")).unwrap();
        assert_eq!(content, "applied");
    }

    #[test]
    fn test_reject_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = WorkspaceManager::new();
        let ws = mgr.open(dir.path().to_str().unwrap()).unwrap();

        let change = mgr.write_file(&ws.id, "rejected.txt", "nope").unwrap();
        mgr.reject_change(&change.id).unwrap();

        // File should still not exist
        assert!(!dir.path().join("rejected.txt").exists());
    }

    #[test]
    fn test_path_traversal_rejected() {
        let (mut mgr, dir) = make_manager_with_temp();
        let ws = mgr.open(dir.path().to_str().unwrap()).unwrap();
        let result = mgr.read_file(&ws.id, "../../../etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_modify_creates_diff() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = WorkspaceManager::new();
        let ws = mgr.open(dir.path().to_str().unwrap()).unwrap();

        fs::write(dir.path().join("file.txt"), "old content\n").unwrap();
        let change = mgr.write_file(&ws.id, "file.txt", "new content\n").unwrap();
        assert!(matches!(change.change_type, ChangeType::Modify));
        assert!(!change.diff.is_empty());
    }
}
