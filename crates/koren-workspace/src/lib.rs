//! Explicit, local filesystem access for one open workspace directory.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Classification of a top-level directory entry. Symlinks are reported but never followed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link, which is not followed.
    Symlink,
    /// A filesystem object other than a file, directory, or symlink.
    Other,
}

/// Name and kind of a top-level workspace entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceEntry {
    /// Top-level entry name, without the workspace root prefix.
    pub name: String,
    /// Entry kind determined without following links.
    pub kind: EntryKind,
}

/// Structured workspace operation failures.
#[derive(Debug)]
pub enum WorkspaceError {
    /// No workspace is currently open.
    NotOpen,
    /// The requested root does not exist.
    NotFound(PathBuf),
    /// The root path itself is a symbolic link.
    RootIsSymlink(PathBuf),
    /// The root exists but is not a directory.
    NotDirectory(PathBuf),
    /// An operating system filesystem operation failed.
    Io {
        /// Name of the failed filesystem operation.
        operation: &'static str,
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Operating system error.
        source: io::Error,
    },
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotOpen => f.write_str("no workspace is open"),
            Self::NotFound(path) => write!(f, "workspace path '{}' does not exist", path.display()),
            Self::RootIsSymlink(path) => {
                write!(f, "workspace root '{}' is a symbolic link", path.display())
            }
            Self::NotDirectory(path) => {
                write!(f, "workspace root '{}' is not a directory", path.display())
            }
            Self::Io {
                operation,
                path,
                source,
            } => write!(f, "failed to {operation} '{}': {source}", path.display()),
        }
    }
}

impl std::error::Error for WorkspaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Holds the currently open workspace, if any.
#[derive(Debug, Default)]
pub struct WorkspaceManager {
    root: Option<PathBuf>,
}

impl WorkspaceManager {
    /// Creates a manager with no open workspace.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a local directory. The root itself cannot be a symlink; child symlinks are listed but not followed.
    ///
    /// # Errors
    ///
    /// Returns a structured error for a missing path, symlink root, non-directory,
    /// or failed filesystem operation.
    pub fn open(&mut self, path: impl AsRef<Path>) -> Result<(), WorkspaceError> {
        let requested = path.as_ref();
        let candidate = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|source| WorkspaceError::Io {
                    operation: "read current directory",
                    path: requested.into(),
                    source,
                })?
                .join(requested)
        };
        let metadata = match fs::symlink_metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(WorkspaceError::NotFound(candidate));
            }
            Err(source) => {
                return Err(WorkspaceError::Io {
                    operation: "inspect",
                    path: candidate,
                    source,
                });
            }
        };
        if metadata.file_type().is_symlink() {
            return Err(WorkspaceError::RootIsSymlink(candidate));
        }
        if !metadata.is_dir() {
            return Err(WorkspaceError::NotDirectory(candidate));
        }
        let canonical = fs::canonicalize(&candidate).map_err(|source| WorkspaceError::Io {
            operation: "resolve",
            path: candidate,
            source,
        })?;
        self.root = Some(canonical);
        Ok(())
    }

    /// Returns the canonical root path for an open workspace.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError::NotOpen`] if no workspace is open.
    pub fn root(&self) -> Result<&Path, WorkspaceError> {
        self.root.as_deref().ok_or(WorkspaceError::NotOpen)
    }

    /// Enumerates top-level entries, sorting by name and not following symlinks.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError::NotOpen`] if no workspace is open, or
    /// [`WorkspaceError::Io`] if the directory cannot be read.
    pub fn entries(&self) -> Result<Vec<WorkspaceEntry>, WorkspaceError> {
        let root = self.root()?;
        let read_dir = fs::read_dir(root).map_err(|source| WorkspaceError::Io {
            operation: "read directory",
            path: root.into(),
            source,
        })?;
        let mut entries = Vec::new();
        for item in read_dir {
            let item = item.map_err(|source| WorkspaceError::Io {
                operation: "read entry",
                path: root.into(),
                source,
            })?;
            let name = item.file_name().to_string_lossy().into_owned();
            let path = item.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| WorkspaceError::Io {
                operation: "inspect entry",
                path: path.clone(),
                source,
            })?;
            let file_type = metadata.file_type();
            let kind = if file_type.is_symlink() {
                EntryKind::Symlink
            } else if file_type.is_dir() {
                EntryKind::Directory
            } else if file_type.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            entries.push(WorkspaceEntry { name, kind });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    /// Closes the current workspace. Closing while already closed is an error.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError::NotOpen`] when already closed.
    pub fn close(&mut self) -> Result<(), WorkspaceError> {
        self.root.take().map(|_| ()).ok_or(WorkspaceError::NotOpen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "koren-test-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn opens_lists_and_closes_workspace() {
        let temp = TempDir::new();
        fs::write(temp.0.join("z.txt"), "ok").unwrap();
        fs::create_dir(temp.0.join("a-dir")).unwrap();
        let mut manager = WorkspaceManager::new();
        manager.open(&temp.0).unwrap();
        assert_eq!(manager.root().unwrap(), fs::canonicalize(&temp.0).unwrap());
        let entries = manager.entries().unwrap();
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["a-dir", "z.txt"]
        );
        assert_eq!(entries[0].kind, EntryKind::Directory);
        assert_eq!(entries[1].kind, EntryKind::File);
        manager.close().unwrap();
        assert!(matches!(manager.entries(), Err(WorkspaceError::NotOpen)));
    }

    #[test]
    fn rejects_missing_file_and_symlink_roots() {
        let temp = TempDir::new();
        let mut manager = WorkspaceManager::new();
        assert!(matches!(
            manager.open(temp.0.join("missing")),
            Err(WorkspaceError::NotFound(_))
        ));
        let file = temp.0.join("file");
        fs::write(&file, "x").unwrap();
        assert!(matches!(
            manager.open(&file),
            Err(WorkspaceError::NotDirectory(_))
        ));
        #[cfg(unix)]
        {
            let link = temp.0.join("link");
            std::os::unix::fs::symlink(&temp.0, &link).unwrap();
            assert!(matches!(
                manager.open(&link),
                Err(WorkspaceError::RootIsSymlink(_))
            ));
        }
    }

    #[test]
    fn reports_child_symlinks_without_following_them() {
        #[cfg(unix)]
        {
            let temp = TempDir::new();
            let target = temp.0.join("target");
            fs::create_dir(&target).unwrap();
            std::os::unix::fs::symlink(&target, temp.0.join("linked")).unwrap();
            let mut manager = WorkspaceManager::new();
            manager.open(&temp.0).unwrap();
            assert_eq!(manager.entries().unwrap()[0].kind, EntryKind::Symlink);
        }
    }
}
