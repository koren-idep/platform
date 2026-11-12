//! Local Git repository inspection and explicit staging/commit operations.
//!
//! This crate uses libgit2 through `git2`, so it does not invoke a shell or Git
//! executable. Local commits are created directly and do not execute Git hooks;
//! hook-based workflows therefore need to be run separately by the user.

use git2::{DiffFormat, DiffOptions, Repository, Status, StatusOptions};
use ignore::WalkBuilder;
use std::{
    fmt,
    path::{Component, Path, PathBuf},
};

/// Maximum rendered text diff size returned by [`GitRepository::diff`].
pub const MAX_DIFF_BYTES: usize = 1024 * 1024;
/// Maximum repositories found by one bounded workspace scan.
pub const MAX_REPOSITORIES: usize = 256;

/// Git integration error.
#[derive(Debug)]
pub enum GitError {
    /// Filesystem or libgit2 operation failed.
    Git(git2::Error),
    /// A requested relative path is outside the repository or malformed.
    InvalidPath,
    /// The requested repository or workspace directory is unavailable.
    NotFound,
    /// The operation was refused because Git reports an unresolved conflict.
    Conflict(String),
    /// The requested diff exceeded the configured output limit.
    DiffTooLarge,
    /// Commit messages must contain non-whitespace text.
    EmptyCommitMessage,
    /// No staged changes are available for a local commit.
    NothingToCommit,
    /// Workspace traversal exceeded its safety bound.
    RepositoryLimit,
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(error) => write!(f, "Git operation failed: {error}"),
            Self::InvalidPath => {
                f.write_str("path must be a repository-relative path without traversal")
            }
            Self::NotFound => f.write_str("Git repository was not found"),
            Self::Conflict(path) => write!(
                f,
                "Git operation conflicts with unresolved changes at {path}"
            ),
            Self::DiffTooLarge => f.write_str("Git diff exceeds the 1 MiB display limit"),
            Self::EmptyCommitMessage => f.write_str("commit message cannot be empty"),
            Self::NothingToCommit => f.write_str("there are no staged changes to commit"),
            Self::RepositoryLimit => {
                f.write_str("workspace contains too many Git repositories to enumerate")
            }
        }
    }
}
impl std::error::Error for GitError {}
impl From<git2::Error> for GitError {
    fn from(value: git2::Error) -> Self {
        Self::Git(value)
    }
}

/// A discovered repository summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryInfo {
    /// Canonical repository working directory.
    pub root: PathBuf,
    /// Current branch name, if HEAD is attached.
    pub branch: Option<String>,
    /// Whether HEAD points to a commit.
    pub has_head: bool,
}

/// One path reported by Git status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangedFile {
    /// Repository-relative path using forward slashes.
    pub path: String,
    /// Whether the index contains a staged change.
    pub staged: bool,
    /// Whether the working tree contains an unstaged change.
    pub unstaged: bool,
    /// Whether the path is untracked.
    pub untracked: bool,
    /// Whether Git reports a deletion.
    pub deleted: bool,
    /// Whether Git reports a conflict.
    pub conflicted: bool,
}

/// A local Git repository handle rooted inside an opened workspace.
pub struct GitRepository {
    repository: Repository,
    root: PathBuf,
}

/// Language-neutral local Git service scoped to one workspace.
pub struct GitService {
    root: PathBuf,
}

impl GitService {
    /// Creates a service for an existing workspace directory.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, GitError> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|_| GitError::NotFound)?;
        if !root.is_dir() {
            return Err(GitError::NotFound);
        }
        Ok(Self { root })
    }

    /// Opens the repository containing `path`, provided its working tree stays in this workspace.
    pub fn open(&self, path: impl AsRef<Path>) -> Result<GitRepository, GitError> {
        let candidate = path.as_ref();
        let absolute = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };
        let repository = Repository::discover(absolute)?;
        let workdir = repository
            .workdir()
            .ok_or(GitError::NotFound)?
            .canonicalize()
            .map_err(|_| GitError::NotFound)?;
        if !workdir.starts_with(&self.root) {
            return Err(GitError::InvalidPath);
        }
        Ok(GitRepository {
            repository,
            root: workdir,
        })
    }

    /// Finds local repositories under the workspace without following symlinks.
    pub fn discover(&self) -> Result<Vec<RepositoryInfo>, GitError> {
        let mut found = Vec::new();
        let mut walker = WalkBuilder::new(&self.root);
        walker
            .hidden(false)
            .follow_links(false)
            .max_depth(Some(128))
            .ignore(false)
            .git_ignore(false)
            .git_global(false)
            .parents(false);
        walker.filter_entry(|entry| {
            !(entry.file_type().is_some_and(|kind| kind.is_dir())
                && matches!(
                    entry.file_name().to_str(),
                    Some(".git" | "target" | "node_modules" | "dist" | "build" | ".cache")
                ))
        });
        let mut visited = 0usize;
        if let Some((repository, workdir)) =
            Repository::open(&self.root).ok().and_then(|repository| {
                repository
                    .workdir()
                    .map(Path::to_path_buf)
                    .map(|workdir| (repository, workdir))
            })
        {
            found.push(repository_info(
                &repository,
                workdir.canonicalize().map_err(|_| GitError::NotFound)?,
            ));
        }
        for entry in walker.build() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            visited += 1;
            if visited > 200_000 {
                return Err(GitError::RepositoryLimit);
            }
            if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
                continue;
            }
            let candidate = entry.path();
            if !candidate.join(".git").exists() {
                continue;
            }
            if let Ok(repository) = Repository::open(candidate) {
                let Some(workdir) = repository.workdir() else {
                    continue;
                };
                let Ok(root) = workdir.canonicalize() else {
                    continue;
                };
                if !root.starts_with(&self.root) {
                    continue;
                }
                let info = repository_info(&repository, root);
                if !found
                    .iter()
                    .any(|value: &RepositoryInfo| value.root == info.root)
                {
                    found.push(info);
                    if found.len() > MAX_REPOSITORIES {
                        return Err(GitError::RepositoryLimit);
                    }
                }
            }
        }
        found.sort_by(|left, right| left.root.cmp(&right.root));
        Ok(found)
    }
}

impl GitRepository {
    /// Returns repository location and branch/HEAD information.
    pub fn info(&self) -> RepositoryInfo {
        repository_info(&self.repository, self.root.clone())
    }

    /// Reads staged, unstaged, untracked, deleted, and conflict status.
    pub fn status(&self) -> Result<Vec<ChangedFile>, GitError> {
        let mut options = StatusOptions::new();
        options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_ignored(false)
            .renames_head_to_index(true)
            .renames_index_to_workdir(true);
        let statuses = self.repository.statuses(Some(&mut options))?;
        let mut files = Vec::with_capacity(statuses.len());
        for item in statuses.iter() {
            let Some(path) = item.path() else { continue };
            let status = item.status();
            let staged = status.intersects(
                Status::INDEX_NEW
                    | Status::INDEX_MODIFIED
                    | Status::INDEX_DELETED
                    | Status::INDEX_RENAMED
                    | Status::INDEX_TYPECHANGE,
            );
            let unstaged = status.intersects(
                Status::WT_MODIFIED
                    | Status::WT_DELETED
                    | Status::WT_RENAMED
                    | Status::WT_TYPECHANGE,
            );
            files.push(ChangedFile {
                path: path.replace('\\', "/"),
                staged,
                unstaged,
                untracked: status.contains(Status::WT_NEW),
                deleted: status.intersects(Status::INDEX_DELETED | Status::WT_DELETED),
                conflicted: status.contains(Status::CONFLICTED),
            });
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(files)
    }

    /// Returns a bounded text diff for one path, optionally comparing the index to HEAD.
    pub fn diff(&self, path: &str, staged: bool) -> Result<String, GitError> {
        let path = validate_relative(path)?;
        let changed = self
            .status()?
            .into_iter()
            .find(|file| file.path == path.to_string_lossy().replace('\\', "/"));
        if changed.as_ref().is_some_and(|file| file.conflicted) {
            return Err(GitError::Conflict(path.to_string_lossy().into_owned()));
        }
        let mut options = DiffOptions::new();
        options
            .pathspec(&path)
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .context_lines(3)
            .max_size(MAX_DIFF_BYTES as i64);
        let diff = if staged {
            let head = self
                .repository
                .head()
                .ok()
                .and_then(|head| head.peel_to_tree().ok());
            self.repository
                .diff_tree_to_index(head.as_ref(), None, Some(&mut options))?
        } else {
            let index = self.repository.index()?;
            self.repository
                .diff_index_to_workdir(Some(&index), Some(&mut options))?
        };
        if diff.deltas().any(|delta| {
            delta.old_file().size() > MAX_DIFF_BYTES as u64
                || delta.new_file().size() > MAX_DIFF_BYTES as u64
        }) {
            return Err(GitError::DiffTooLarge);
        }
        let mut output = Vec::new();
        let mut too_large = false;
        let mut binary = false;
        let print_result = diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
            let origin = line.origin();
            if origin == 'B' {
                binary = true;
                return false;
            }
            let prefix = if matches!(origin, '+' | '-' | ' ') {
                origin as u8
            } else {
                0
            };
            let content = line.content();
            let prefix_len = usize::from(prefix != 0);
            if output
                .len()
                .saturating_add(content.len())
                .saturating_add(prefix_len)
                > MAX_DIFF_BYTES
            {
                too_large = true;
                return false;
            }
            if prefix != 0 {
                output.push(prefix);
            }
            output.extend_from_slice(content);
            true
        });
        if binary {
            return Ok(format!(
                "Binary diff for {} is not displayed.",
                path.display()
            ));
        }
        if too_large {
            return Err(GitError::DiffTooLarge);
        }
        print_result?;
        Ok(String::from_utf8_lossy(&output).into_owned())
    }

    /// Stages exactly one repository-relative path. It never stages other files.
    pub fn stage(&self, path: &str) -> Result<(), GitError> {
        let path = validate_relative(path)?;
        let status = self
            .status()?
            .into_iter()
            .find(|file| file.path == path.to_string_lossy());
        if status.as_ref().is_some_and(|file| file.conflicted) {
            return Err(GitError::Conflict(path.to_string_lossy().into_owned()));
        }
        let mut index = self.repository.index()?;
        if status.as_ref().is_some_and(|file| file.deleted) {
            index.remove_path(&path)?;
        } else {
            index.add_path(&path)?;
        }
        index.write()?;
        Ok(())
    }

    /// Restores exactly one path in the index to its HEAD state, or removes it if newly added.
    pub fn unstage(&self, path: &str) -> Result<(), GitError> {
        let path = validate_relative(path)?;
        let mut index = self.repository.index()?;
        if self.repository.head().is_ok() {
            self.repository.reset_default(
                Some(self.repository.head()?.peel_to_commit()?.as_object()),
                [&path],
            )?;
        } else {
            index.remove_path(&path)?;
            index.write()?;
        }
        Ok(())
    }

    /// Creates a local commit from the index only, without running external hooks.
    pub fn commit(&self, message: &str) -> Result<String, GitError> {
        if message.trim().is_empty() {
            return Err(GitError::EmptyCommitMessage);
        }
        let mut index = self.repository.index()?;
        if index.has_conflicts() {
            return Err(GitError::Conflict("index".into()));
        }
        let tree_id = index.write_tree()?;
        if index.iter().next().is_none()
            || self
                .repository
                .head()
                .ok()
                .and_then(|head| head.peel_to_tree().ok())
                .is_some_and(|head| head.id() == tree_id)
        {
            return Err(GitError::NothingToCommit);
        }
        let tree = self.repository.find_tree(tree_id)?;
        let signature = self.repository.signature()?;
        let parent = self
            .repository
            .head()
            .ok()
            .and_then(|head| head.peel_to_commit().ok());
        let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
        let oid = self.repository.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parents,
        )?;
        Ok(oid.to_string())
    }
}

fn repository_info(repository: &Repository, root: PathBuf) -> RepositoryInfo {
    let head = repository.head().ok();
    RepositoryInfo {
        root,
        branch: head
            .as_ref()
            .and_then(|head| head.shorthand())
            .map(str::to_owned),
        has_head: head.is_some(),
    }
}

fn validate_relative(path: &str) -> Result<PathBuf, GitError> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| match component {
            Component::Normal(name) => name == ".git",
            _ => true,
        })
    {
        return Err(GitError::InvalidPath);
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "koren-git-{}-{}",
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
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn setup() -> (Temp, GitRepository) {
        let temp = Temp::new();
        let repo = Repository::init(&temp.0).unwrap();
        repo.config()
            .unwrap()
            .set_str("user.name", "Koren Tests")
            .unwrap();
        repo.config()
            .unwrap()
            .set_str("user.email", "koren@example.invalid")
            .unwrap();
        let service = GitService::new(&temp.0).unwrap();
        (temp, service.open(".").unwrap())
    }
    #[test]
    fn status_stage_diff_commit_and_unstage_are_path_specific() {
        let (temp, repo) = setup();
        fs::write(temp.0.join("one.txt"), "initial\n").unwrap();
        repo.stage("one.txt").unwrap();
        repo.commit("initial").unwrap();
        assert!(matches!(
            repo.commit("empty"),
            Err(GitError::NothingToCommit)
        ));
        fs::write(temp.0.join("one.txt"), "changed\n").unwrap();
        fs::write(temp.0.join("two.txt"), "new\n").unwrap();
        let status = repo.status().unwrap();
        assert_eq!(status.len(), 2);
        assert!(
            status
                .iter()
                .any(|file| file.path == "one.txt" && file.unstaged)
        );
        let diff = repo.diff("one.txt", false).unwrap();
        assert!(diff.contains("-initial"), "unexpected diff: {diff:?}");
        repo.stage("two.txt").unwrap();
        assert!(
            repo.status()
                .unwrap()
                .iter()
                .any(|file| file.path == "two.txt" && file.staged)
        );
        repo.unstage("two.txt").unwrap();
        assert!(
            repo.status()
                .unwrap()
                .iter()
                .any(|file| file.path == "two.txt" && file.untracked)
        );
        repo.stage("one.txt").unwrap();
        repo.commit("update one").unwrap();
        assert!(
            repo.status()
                .unwrap()
                .iter()
                .any(|file| file.path == "two.txt" && file.untracked)
        );
    }
    #[test]
    fn discovers_multiple_repositories_and_rejects_path_escape() {
        let temp = Temp::new();
        let outer = temp.0.join("outer");
        fs::create_dir_all(outer.join("nested")).unwrap();
        Repository::init(&outer).unwrap();
        Repository::init(outer.join("nested")).unwrap();
        let service = GitService::new(&temp.0).unwrap();
        assert_eq!(service.discover().unwrap().len(), 2);
        let repo = service.open("outer").unwrap();
        assert!(matches!(
            repo.stage("../outside"),
            Err(GitError::InvalidPath)
        ));
        assert!(matches!(
            repo.stage(".git/config"),
            Err(GitError::InvalidPath)
        ));
    }

    #[test]
    fn deletion_can_be_staged_and_unstaged_without_touching_worktree_contents() {
        let (temp, repo) = setup();
        fs::write(temp.0.join("gone.txt"), "tracked\n").unwrap();
        repo.stage("gone.txt").unwrap();
        repo.commit("add file").unwrap();
        fs::remove_file(temp.0.join("gone.txt")).unwrap();
        repo.stage("gone.txt").unwrap();
        assert!(
            repo.status()
                .unwrap()
                .iter()
                .any(|item| item.path == "gone.txt" && item.staged && item.deleted)
        );
        assert!(repo.diff("gone.txt", true).unwrap().contains("-tracked"));
        repo.unstage("gone.txt").unwrap();
        assert!(
            repo.status()
                .unwrap()
                .iter()
                .any(|item| item.path == "gone.txt" && item.unstaged && item.deleted)
        );
        assert!(!temp.0.join("gone.txt").exists());
    }

    #[test]
    fn binary_diff_returns_a_bounded_explanation() {
        let (temp, repo) = setup();
        fs::write(temp.0.join("image.bin"), b"old\0data").unwrap();
        repo.stage("image.bin").unwrap();
        repo.commit("add binary fixture").unwrap();
        fs::write(temp.0.join("image.bin"), b"new\0data").unwrap();
        assert_eq!(
            repo.diff("image.bin", false).unwrap(),
            "Binary diff for image.bin is not displayed."
        );
    }
}
