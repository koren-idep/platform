//! Demand-driven workspace navigation with explicit relative path boundaries.

use koren_core::{EventQueue, PlatformEvent};
use std::{
    ffi::OsString,
    fs, io,
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

/// Classification of a directory entry. Symlinks are surfaced but never traversed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link, which cannot be traversed by workspace-relative operations.
    Symlink,
    /// A filesystem object other than a file, directory, or symlink.
    Other,
}

/// Metadata for one entry in a workspace directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceEntry {
    /// Top-level entry name, preserving non-UTF-8 names on supported filesystems.
    pub name: OsString,
    /// Path relative to the workspace root.
    pub path: PathBuf,
    /// Entry kind determined without following links.
    pub kind: EntryKind,
    /// File length in bytes; directory lengths are filesystem-specific.
    pub size_bytes: u64,
    /// Modification time when the filesystem provides it.
    pub modified: Option<SystemTime>,
}

/// Root information for the currently open workspace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceMetadata {
    /// Canonical absolute workspace root.
    pub root: PathBuf,
    /// Last path component of the root.
    pub name: OsString,
}

/// Limits for demand-driven directory enumeration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceOptions {
    /// Maximum entries returned by a single directory listing.
    pub max_entries_per_directory: usize,
}

impl Default for WorkspaceOptions {
    fn default() -> Self {
        Self {
            max_entries_per_directory: 10_000,
        }
    }
}

/// Structured workspace operation failures.
#[derive(Debug)]
pub enum WorkspaceError {
    /// No workspace is currently open.
    NotOpen,
    /// A workspace is already open in this manager.
    AlreadyOpen,
    /// The requested root or relative path does not exist.
    NotFound(PathBuf),
    /// The root path itself is a symbolic link.
    RootIsSymlink(PathBuf),
    /// A symbolic link occurs in a workspace-relative path.
    SymlinkNotAllowed(PathBuf),
    /// The root or requested navigation target is not a directory.
    NotDirectory(PathBuf),
    /// A document target is not a regular file.
    NotFile(PathBuf),
    /// A workspace-relative path is absolute or attempts to escape the root.
    InvalidRelativePath(PathBuf),
    /// The requested destination already exists.
    AlreadyExists(PathBuf),
    /// Explicit confirmation is required for a delete operation.
    ConfirmationRequired,
    /// Directory has more entries than the configured per-listing limit.
    DirectoryLimitExceeded {
        /// Relative path of the directory that exceeded the limit.
        path: PathBuf,
        /// Configured maximum number of entries.
        limit: usize,
    },
    /// A directory listing limit of zero is invalid.
    InvalidLimit,
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
            Self::AlreadyOpen => f.write_str("a workspace is already open"),
            Self::NotFound(path) => write!(f, "workspace path '{}' does not exist", path.display()),
            Self::RootIsSymlink(path) => {
                write!(f, "workspace root '{}' is a symbolic link", path.display())
            }
            Self::SymlinkNotAllowed(path) => write!(
                f,
                "workspace path '{}' crosses a symbolic link",
                path.display()
            ),
            Self::NotDirectory(path) => write!(f, "'{}' is not a directory", path.display()),
            Self::NotFile(path) => write!(f, "'{}' is not a regular file", path.display()),
            Self::InvalidRelativePath(path) => write!(
                f,
                "'{}' is not a safe workspace-relative path",
                path.display()
            ),
            Self::AlreadyExists(path) => write!(f, "'{}' already exists", path.display()),
            Self::ConfirmationRequired => f.write_str("filesystem deletion requires confirmation"),
            Self::DirectoryLimitExceeded { path, limit } => write!(
                f,
                "directory '{}' exceeds the entry limit of {limit}",
                path.display()
            ),
            Self::InvalidLimit => f.write_str("directory entry limit must be greater than zero"),
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

/// Owns one open workspace, navigation limits, and queued workspace events.
#[derive(Debug)]
pub struct WorkspaceManager {
    root: Option<PathBuf>,
    options: WorkspaceOptions,
    events: EventQueue,
}

impl Default for WorkspaceManager {
    fn default() -> Self {
        Self {
            root: None,
            options: WorkspaceOptions::default(),
            events: EventQueue::new(),
        }
    }
}

impl WorkspaceManager {
    /// Creates a manager with default navigation limits.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a manager with custom navigation limits.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError::InvalidLimit`] when the entry limit is zero.
    pub fn with_options(options: WorkspaceOptions) -> Result<Self, WorkspaceError> {
        if options.max_entries_per_directory == 0 {
            return Err(WorkspaceError::InvalidLimit);
        }
        Ok(Self {
            options,
            ..Self::default()
        })
    }

    /// Opens a local directory without scanning its contents. The root itself cannot be a symlink.
    ///
    /// Relative paths are resolved against the process working directory. This
    /// performs a point-in-time symlink check; it is not a security sandbox
    /// against concurrent hostile filesystem modifications.
    ///
    /// # Errors
    ///
    /// Returns a structured error for a missing path, symlink root, non-directory,
    /// an already-open manager, or failed filesystem operation.
    pub fn open(&mut self, path: impl AsRef<Path>) -> Result<(), WorkspaceError> {
        if self.root.is_some() {
            return Err(WorkspaceError::AlreadyOpen);
        }
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
        let root = canonical;
        self.root = Some(root.clone());
        self.events.push(PlatformEvent::WorkspaceOpened { root });
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

    /// Returns root metadata for an open workspace.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError::NotOpen`] if no workspace is open.
    pub fn metadata(&self) -> Result<WorkspaceMetadata, WorkspaceError> {
        let root = self.root()?;
        let name = root.file_name().unwrap_or(root.as_os_str()).to_os_string();
        Ok(WorkspaceMetadata {
            root: root.to_path_buf(),
            name,
        })
    }

    /// Enumerates the workspace root in deterministic name order.
    ///
    /// # Errors
    ///
    /// Returns an error if no workspace is open, the entry limit is exceeded,
    /// or the directory cannot be read.
    pub fn entries(&self) -> Result<Vec<WorkspaceEntry>, WorkspaceError> {
        self.list_dir(Path::new("."))
    }

    /// Enumerates one directory relative to the workspace root, without recursion.
    ///
    /// The path may contain `.` components, but absolute paths and parent
    /// components are rejected. Symlinks are listed as entries and never
    /// traversed. Enumeration reads at most the configured limit plus one item.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid or missing paths, symbolic link traversal,
    /// a non-directory target, an exceeded listing limit, or filesystem errors.
    pub fn list_dir(
        &self,
        relative: impl AsRef<Path>,
    ) -> Result<Vec<WorkspaceEntry>, WorkspaceError> {
        let relative = normalize_relative(relative.as_ref())?;
        let directory = self.resolve_relative(&relative, true)?;
        let read_dir = fs::read_dir(&directory).map_err(|source| WorkspaceError::Io {
            operation: "read directory",
            path: directory.clone(),
            source,
        })?;
        let mut entries = Vec::new();
        for item in read_dir {
            if entries.len() == self.options.max_entries_per_directory {
                return Err(WorkspaceError::DirectoryLimitExceeded {
                    path: relative.clone(),
                    limit: self.options.max_entries_per_directory,
                });
            }
            let item = item.map_err(|source| WorkspaceError::Io {
                operation: "read directory entry",
                path: directory.clone(),
                source,
            })?;
            let name = item.file_name();
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
            let relative_path = if relative.as_os_str().is_empty() {
                PathBuf::from(&name)
            } else {
                relative.join(&name)
            };
            entries.push(WorkspaceEntry {
                name,
                path: relative_path,
                kind,
                size_bytes: metadata.len(),
                modified: metadata.modified().ok(),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    /// Resolves an existing regular file while enforcing the workspace-relative path policy.
    ///
    /// No symlink component is traversed. Canonical containment is checked as
    /// an additional guard, but filesystem races still require separate policy.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid, missing, symlinked, out-of-root, or
    /// non-file paths, and for filesystem errors.
    pub fn resolve_file(&self, relative: impl AsRef<Path>) -> Result<PathBuf, WorkspaceError> {
        let relative = normalize_relative(relative.as_ref())?;
        let path = self.resolve_relative(&relative, false)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| map_path_error(error, &path))?;
        if !metadata.is_file() {
            return Err(WorkspaceError::NotFile(path));
        }
        let canonical = fs::canonicalize(&path).map_err(|source| WorkspaceError::Io {
            operation: "resolve file",
            path: path.clone(),
            source,
        })?;
        let root = self.root()?;
        if !canonical.starts_with(root) {
            return Err(WorkspaceError::InvalidRelativePath(relative));
        }
        Ok(canonical)
    }

    /// Returns no-follow metadata for one existing workspace entry.
    ///
    /// # Errors
    /// Returns a path or filesystem error for missing, invalid, or symlinked entries.
    pub fn entry_metadata(
        &self,
        relative: impl AsRef<Path>,
    ) -> Result<WorkspaceEntry, WorkspaceError> {
        let relative = normalize_relative(relative.as_ref())?;
        if relative.as_os_str().is_empty() {
            return Err(WorkspaceError::InvalidRelativePath(relative));
        }
        let path = self.resolve_relative(&relative, false)?;
        entry_for_path(&path, relative)
    }

    /// Creates a new empty file without overwriting an existing destination.
    ///
    /// # Errors
    /// Returns an error for invalid or symlinked parent paths, an existing destination,
    /// or an operating system failure. Path checks are point-in-time checks and cannot
    /// prevent hostile concurrent filesystem replacement.
    pub fn create_file(
        &mut self,
        relative: impl AsRef<Path>,
    ) -> Result<WorkspaceEntry, WorkspaceError> {
        let relative = normalize_new_path(relative.as_ref())?;
        let path = self.resolve_new_path(&relative)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|source| {
                if source.kind() == io::ErrorKind::AlreadyExists {
                    WorkspaceError::AlreadyExists(relative.clone())
                } else {
                    WorkspaceError::Io {
                        operation: "create file",
                        path: path.clone(),
                        source,
                    }
                }
            })?;
        drop(file);
        let entry = entry_for_path(&path, relative.clone())?;
        self.events.push(PlatformEvent::WorkspaceEntryChanged {
            operation: "created".into(),
            path: relative,
            previous_path: None,
        });
        Ok(entry)
    }

    /// Creates one new directory without replacing an existing entry.
    ///
    /// # Errors
    /// Returns an error for invalid or symlinked parents, an existing destination,
    /// or an operating system failure.
    pub fn create_directory(
        &mut self,
        relative: impl AsRef<Path>,
    ) -> Result<WorkspaceEntry, WorkspaceError> {
        let relative = normalize_new_path(relative.as_ref())?;
        let path = self.resolve_new_path(&relative)?;
        fs::create_dir(&path).map_err(|source| {
            if source.kind() == io::ErrorKind::AlreadyExists {
                WorkspaceError::AlreadyExists(relative.clone())
            } else {
                WorkspaceError::Io {
                    operation: "create directory",
                    path: path.clone(),
                    source,
                }
            }
        })?;
        let entry = entry_for_path(&path, relative.clone())?;
        self.events.push(PlatformEvent::WorkspaceEntryChanged {
            operation: "created".into(),
            path: relative,
            previous_path: None,
        });
        Ok(entry)
    }

    /// Renames or moves an entry within the workspace without overwriting a destination.
    ///
    /// Directory moves into their own descendants are rejected. Open-document state is
    /// managed by `koren-documents`; hosts must update buffers after a successful move.
    ///
    /// # Errors
    /// Returns an error for invalid paths, symlinks, missing sources, existing destinations,
    /// or filesystem failures. This operation is not transactional across concurrent changes.
    pub fn rename(
        &mut self,
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> Result<WorkspaceEntry, WorkspaceError> {
        let source = normalize_new_path(source.as_ref())?;
        let destination = normalize_new_path(destination.as_ref())?;
        let source_path = self.resolve_relative(&source, false)?;
        let source_metadata = fs::symlink_metadata(&source_path)
            .map_err(|error| map_path_error(error, &source_path))?;
        if source_metadata.file_type().is_symlink() {
            return Err(WorkspaceError::SymlinkNotAllowed(source.clone()));
        }
        let destination_path = self.resolve_new_path(&destination)?;
        if source_metadata.is_dir() && destination_path.starts_with(&source_path) {
            return Err(WorkspaceError::InvalidRelativePath(destination));
        }
        match fs::symlink_metadata(&destination_path) {
            Ok(_) => return Err(WorkspaceError::AlreadyExists(destination)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source_error) => {
                return Err(WorkspaceError::Io {
                    operation: "inspect destination",
                    path: destination_path,
                    source: source_error,
                });
            }
        }
        fs::rename(&source_path, &destination_path).map_err(|source_error| WorkspaceError::Io {
            operation: "rename entry",
            path: source_path,
            source: source_error,
        })?;
        let entry = entry_for_path(&destination_path, destination.clone())?;
        self.events.push(PlatformEvent::WorkspaceEntryChanged {
            operation: "renamed".into(),
            path: destination,
            previous_path: Some(source),
        });
        Ok(entry)
    }

    /// Deletes an entry after explicit caller confirmation.
    ///
    /// Directories are removed recursively only when `recursive` is true. Hosts should
    /// check dirty document buffers before invoking this operation. Deletion is not
    /// transactional and cannot be rolled back after partial recursive failure.
    ///
    /// # Errors
    /// Returns an error for missing/unsafe paths, absent confirmation, or filesystem failure.
    pub fn delete(
        &mut self,
        relative: impl AsRef<Path>,
        recursive: bool,
        confirmed: bool,
    ) -> Result<(), WorkspaceError> {
        if !confirmed {
            return Err(WorkspaceError::ConfirmationRequired);
        }
        let relative = normalize_new_path(relative.as_ref())?;
        let path = self.resolve_relative(&relative, false)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| map_path_error(error, &path))?;
        if metadata.file_type().is_symlink() {
            return Err(WorkspaceError::SymlinkNotAllowed(relative));
        }
        let result = if metadata.is_dir() {
            if recursive {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_dir(&path)
            }
        } else {
            fs::remove_file(&path)
        };
        result.map_err(|source| WorkspaceError::Io {
            operation: "delete entry",
            path,
            source,
        })?;
        self.events.push(PlatformEvent::WorkspaceEntryChanged {
            operation: "deleted".into(),
            path: relative,
            previous_path: None,
        });
        Ok(())
    }

    /// Drains workspace lifecycle events in FIFO order.
    pub fn drain_events(&mut self) -> Vec<PlatformEvent> {
        self.events.drain()
    }

    /// Closes the current workspace. Closing while already closed is an error.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError::NotOpen`] when already closed.
    pub fn close(&mut self) -> Result<(), WorkspaceError> {
        let root = self.root.take().ok_or(WorkspaceError::NotOpen)?;
        self.events.push(PlatformEvent::WorkspaceClosed { root });
        Ok(())
    }

    fn resolve_relative(
        &self,
        relative: &Path,
        directory: bool,
    ) -> Result<PathBuf, WorkspaceError> {
        let root = self.root()?;
        let mut candidate = root.to_path_buf();
        let components = relative.components().collect::<Vec<_>>();
        for (index, component) in components.iter().enumerate() {
            let Component::Normal(value) = component else {
                return Err(WorkspaceError::InvalidRelativePath(relative.to_path_buf()));
            };
            candidate.push(value);
            let metadata = fs::symlink_metadata(&candidate)
                .map_err(|error| map_path_error(error, &candidate))?;
            if metadata.file_type().is_symlink() {
                return Err(WorkspaceError::SymlinkNotAllowed(candidate));
            }
            let is_final = index + 1 == components.len();
            if !is_final && !metadata.is_dir() {
                return Err(WorkspaceError::NotDirectory(candidate));
            }
            if is_final && directory && !metadata.is_dir() {
                return Err(WorkspaceError::NotDirectory(candidate));
            }
        }
        Ok(candidate)
    }

    fn resolve_new_path(&self, relative: &Path) -> Result<PathBuf, WorkspaceError> {
        let parent = relative.parent().unwrap_or_else(|| Path::new(""));
        let parent = if parent.as_os_str().is_empty() {
            self.root()?.to_path_buf()
        } else {
            self.resolve_relative(parent, true)?
        };
        Ok(parent.join(
            relative
                .file_name()
                .ok_or_else(|| WorkspaceError::InvalidRelativePath(relative.into()))?,
        ))
    }
}

fn normalize_new_path(path: &Path) -> Result<PathBuf, WorkspaceError> {
    let path = normalize_relative(path)?;
    if path.as_os_str().is_empty() || path.file_name().is_none() {
        return Err(WorkspaceError::InvalidRelativePath(path));
    }
    Ok(path)
}

fn entry_for_path(path: &Path, relative: PathBuf) -> Result<WorkspaceEntry, WorkspaceError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| WorkspaceError::Io {
        operation: "inspect entry",
        path: path.to_path_buf(),
        source,
    })?;
    let kind = if metadata.file_type().is_symlink() {
        EntryKind::Symlink
    } else if metadata.is_dir() {
        EntryKind::Directory
    } else if metadata.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    Ok(WorkspaceEntry {
        name: relative
            .file_name()
            .unwrap_or(relative.as_os_str())
            .to_os_string(),
        path: relative,
        kind,
        size_bytes: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn normalize_relative(path: &Path) -> Result<PathBuf, WorkspaceError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => normalized.push(value),
            _ => return Err(WorkspaceError::InvalidRelativePath(path.to_path_buf())),
        }
    }
    Ok(normalized)
}

fn map_path_error(error: io::Error, path: &Path) -> WorkspaceError {
    if error.kind() == io::ErrorKind::NotFound {
        WorkspaceError::NotFound(path.to_path_buf())
    } else {
        WorkspaceError::Io {
            operation: "inspect",
            path: path.to_path_buf(),
            source: error,
        }
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
                "koren-workspace-test-{}-{}",
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
    fn opens_lazily_lists_deterministically_and_closes() {
        let temp = TempDir::new();
        fs::write(temp.0.join("z file.txt"), "ok").unwrap();
        fs::create_dir(temp.0.join("a-dir")).unwrap();
        let mut manager = WorkspaceManager::new();
        manager.open(&temp.0).unwrap();
        assert!(matches!(
            manager.open(&temp.0),
            Err(WorkspaceError::AlreadyOpen)
        ));
        assert_eq!(
            manager.metadata().unwrap().root,
            fs::canonicalize(&temp.0).unwrap()
        );
        let entries = manager.entries().unwrap();
        assert_eq!(
            entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
            [OsString::from("a-dir"), OsString::from("z file.txt")]
        );
        assert_eq!(entries[0].kind, EntryKind::Directory);
        assert_eq!(entries[1].kind, EntryKind::File);
        manager.close().unwrap();
        assert!(matches!(manager.entries(), Err(WorkspaceError::NotOpen)));
        assert!(matches!(manager.close(), Err(WorkspaceError::NotOpen)));
        assert_eq!(
            manager.drain_events(),
            vec![
                PlatformEvent::WorkspaceOpened {
                    root: fs::canonicalize(&temp.0).unwrap()
                },
                PlatformEvent::WorkspaceClosed {
                    root: fs::canonicalize(&temp.0).unwrap()
                },
            ]
        );
    }

    #[test]
    fn nested_navigation_handles_unicode_and_relative_boundaries() {
        let temp = TempDir::new();
        fs::create_dir(temp.0.join("資料")).unwrap();
        fs::write(temp.0.join("資料/hello world.rs"), "fn main() {}").unwrap();
        let mut manager = WorkspaceManager::new();
        manager.open(&temp.0).unwrap();
        let nested = manager.list_dir("資料").unwrap();
        assert_eq!(nested[0].path, PathBuf::from("資料/hello world.rs"));
        assert!(manager.resolve_file("資料/hello world.rs").is_ok());
        assert!(matches!(
            manager.list_dir("../"),
            Err(WorkspaceError::InvalidRelativePath(_))
        ));
        assert!(matches!(
            manager.list_dir(temp.0.join("資料")),
            Err(WorkspaceError::InvalidRelativePath(_))
        ));
        assert!(matches!(
            manager.resolve_file("missing.txt"),
            Err(WorkspaceError::NotFound(_))
        ));
    }

    #[test]
    fn symbolic_links_are_listed_but_not_traversed() {
        #[cfg(unix)]
        {
            let temp = TempDir::new();
            let outside = TempDir::new();
            fs::create_dir(temp.0.join("nested")).unwrap();
            fs::write(outside.0.join("private.txt"), "private").unwrap();
            std::os::unix::fs::symlink(&outside.0, temp.0.join("link")).unwrap();
            let mut manager = WorkspaceManager::new();
            manager.open(&temp.0).unwrap();
            assert!(
                manager
                    .entries()
                    .unwrap()
                    .iter()
                    .any(|e| e.name == "link" && e.kind == EntryKind::Symlink)
            );
            assert!(matches!(
                manager.list_dir("link"),
                Err(WorkspaceError::SymlinkNotAllowed(_))
            ));
            assert!(matches!(
                manager.resolve_file("link/private.txt"),
                Err(WorkspaceError::SymlinkNotAllowed(_))
            ));
        }
    }

    #[test]
    fn rejects_a_symbolic_link_as_workspace_root() {
        #[cfg(unix)]
        {
            let temp = TempDir::new();
            let target = temp.0.join("target");
            fs::create_dir(&target).unwrap();
            let link = temp.0.join("root-link");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let mut manager = WorkspaceManager::new();
            assert!(matches!(
                manager.open(link),
                Err(WorkspaceError::RootIsSymlink(_))
            ));
        }
    }

    #[test]
    fn listing_limit_is_configurable_and_explicit() {
        let temp = TempDir::new();
        fs::write(temp.0.join("a"), "").unwrap();
        fs::write(temp.0.join("b"), "").unwrap();
        let options = WorkspaceOptions {
            max_entries_per_directory: 1,
        };
        let mut manager = WorkspaceManager::with_options(options).unwrap();
        manager.open(&temp.0).unwrap();
        assert!(matches!(
            manager.entries(),
            Err(WorkspaceError::DirectoryLimitExceeded { limit: 1, .. })
        ));
        assert!(matches!(
            WorkspaceManager::with_options(WorkspaceOptions {
                max_entries_per_directory: 0
            }),
            Err(WorkspaceError::InvalidLimit)
        ));
    }

    #[test]
    fn filesystem_mutations_are_bounded_to_root_and_never_overwrite() {
        let temp = TempDir::new();
        let outside = TempDir::new();
        let mut manager = WorkspaceManager::new();
        manager.open(&temp.0).unwrap();
        manager.create_directory("src").unwrap();
        manager.create_file("src/main.rs").unwrap();
        fs::write(outside.0.join("keep.txt"), "safe").unwrap();

        assert!(matches!(
            manager.create_file("src/main.rs"),
            Err(WorkspaceError::AlreadyExists(_))
        ));
        assert!(matches!(
            manager.rename("src/main.rs", "../outside.rs"),
            Err(WorkspaceError::InvalidRelativePath(_))
        ));
        assert!(matches!(
            manager.rename("src/main.rs", "src/missing/file.rs"),
            Err(WorkspaceError::NotFound(_))
        ));
        assert_eq!(
            manager.entry_metadata("src/main.rs").unwrap().kind,
            EntryKind::File
        );
        manager.rename("src/main.rs", "src/lib.rs").unwrap();
        assert!(!temp.0.join("src/main.rs").exists());
        assert!(temp.0.join("src/lib.rs").exists());
        assert!(matches!(
            manager.delete("src", true, false),
            Err(WorkspaceError::ConfirmationRequired)
        ));
        manager.delete("src", true, true).unwrap();
        assert!(!temp.0.join("src").exists());
        assert_eq!(
            fs::read_to_string(outside.0.join("keep.txt")).unwrap(),
            "safe"
        );
    }

    #[cfg(unix)]
    #[test]
    fn filesystem_mutations_reject_symlink_parents() {
        use std::os::unix::fs::symlink;
        let temp = TempDir::new();
        let outside = TempDir::new();
        fs::create_dir(temp.0.join("linked")).unwrap();
        symlink(&outside.0, temp.0.join("escape")).unwrap();
        let mut manager = WorkspaceManager::new();
        manager.open(&temp.0).unwrap();
        assert!(matches!(
            manager.create_file("escape/pwned"),
            Err(WorkspaceError::SymlinkNotAllowed(_))
        ));
        assert!(!outside.0.join("pwned").exists());
    }
}
