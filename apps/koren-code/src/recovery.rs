use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const SCHEMA_VERSION: u32 = 1;
const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_SERIALIZED_BYTES: usize = MAX_DOCUMENT_BYTES * 6 + 16 * 1024;
const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SNAPSHOTS: usize = 20;
const MAX_AGE_MS: u128 = 30 * 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecoverySnapshot {
    pub schema_version: u32,
    pub id: String,
    pub workspace: String,
    pub path: String,
    pub contents: String,
    pub base_sha256: String,
    pub revision: u64,
    pub updated_at_ms: u128,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecoverySummary {
    pub id: String,
    pub workspace: String,
    pub path: String,
    pub revision: u64,
    pub updated_at_ms: u128,
    pub byte_length: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryError {
    InvalidSnapshot(String),
    LimitExceeded,
    Io(String),
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSnapshot(message) => formatter.write_str(message),
            Self::LimitExceeded => formatter.write_str("recovery storage limit reached"),
            Self::Io(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for RecoveryError {}

#[derive(Clone, Debug)]
pub struct RecoveryStore {
    directory: PathBuf,
}

pub fn fingerprint_file(workspace: &str, relative_path: &str) -> Result<String, RecoveryError> {
    validate_workspace(workspace)?;
    let root = fs::canonicalize(workspace).map_err(io_error)?;
    let relative = Path::new(relative_path);
    if relative_path.is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(RecoveryError::InvalidSnapshot(
            "invalid workspace-relative document path".into(),
        ));
    }
    let mut candidate = root.clone();
    for component in relative.components() {
        candidate.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&candidate).map_err(io_error)?;
        if metadata.file_type().is_symlink() {
            return Err(RecoveryError::InvalidSnapshot(
                "document path contains a symbolic link".into(),
            ));
        }
    }
    let metadata = fs::metadata(&candidate).map_err(io_error)?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
        return Err(RecoveryError::InvalidSnapshot(
            "document cannot be fingerprinted".into(),
        ));
    }
    let contents = fs::read(candidate).map_err(io_error)?;
    Ok(hex_digest(&Sha256::digest(&contents)))
}

impl RecoveryStore {
    pub fn for_current_user() -> Result<Self, RecoveryError> {
        let base = dirs::data_local_dir().ok_or_else(|| {
            RecoveryError::Io("could not locate the per-user data directory".into())
        })?;
        Ok(Self::new(base.join("koren-code").join("recovery")))
    }

    fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub fn list(&self) -> Result<Vec<RecoverySnapshot>, RecoveryError> {
        self.ensure_directory()?;
        let now = now_ms();
        let mut snapshots = Vec::new();
        for entry in fs::read_dir(&self.directory).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json")
                || entry.file_type().map_err(io_error)?.is_symlink()
                || !entry.file_type().map_err(io_error)?.is_file()
            {
                continue;
            }
            let bytes = match fs::read(entry.path()) {
                Ok(bytes) if bytes.len() <= MAX_SERIALIZED_BYTES => bytes,
                _ => continue,
            };
            let Ok(snapshot) = serde_json::from_slice::<RecoverySnapshot>(&bytes) else {
                continue;
            };
            if self.validate(&snapshot).is_err() {
                continue;
            }
            if now.saturating_sub(snapshot.updated_at_ms) > MAX_AGE_MS {
                let _ = fs::remove_file(entry.path());
                continue;
            }
            snapshots.push(snapshot);
        }
        snapshots.sort_by_key(|snapshot| snapshot.updated_at_ms);
        Ok(snapshots)
    }

    pub fn summaries(&self) -> Result<Vec<RecoverySummary>, RecoveryError> {
        Ok(self
            .list()?
            .into_iter()
            .map(|snapshot| RecoverySummary {
                id: snapshot.id,
                workspace: snapshot.workspace,
                path: snapshot.path,
                revision: snapshot.revision,
                updated_at_ms: snapshot.updated_at_ms,
                byte_length: snapshot.contents.len(),
            })
            .collect())
    }

    pub fn get(&self, id: &str) -> Result<Option<RecoverySnapshot>, RecoveryError> {
        if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(RecoveryError::InvalidSnapshot(
                "invalid recovery identifier".into(),
            ));
        }
        self.ensure_directory()?;
        let path = self.directory.join(format!("{id}.json"));
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(RecoveryError::InvalidSnapshot(
                    "recovery snapshot is not a regular file".into(),
                ));
            }
        }
        let bytes = match fs::read(path) {
            Ok(bytes) if bytes.len() <= MAX_SERIALIZED_BYTES => bytes,
            Ok(_) => return Err(RecoveryError::LimitExceeded),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
        };
        let snapshot: RecoverySnapshot =
            serde_json::from_slice(&bytes).map_err(|error| RecoveryError::Io(error.to_string()))?;
        self.validate(&snapshot)?;
        Ok(Some(snapshot))
    }

    pub fn write(&self, mut snapshot: RecoverySnapshot) -> Result<String, RecoveryError> {
        self.ensure_directory()?;
        if snapshot.schema_version != SCHEMA_VERSION {
            return Err(RecoveryError::InvalidSnapshot(
                "unsupported recovery schema".into(),
            ));
        }
        snapshot.schema_version = SCHEMA_VERSION;
        snapshot.id = snapshot_id(&snapshot.workspace, &snapshot.path);
        snapshot.updated_at_ms = now_ms();
        self.validate(&snapshot)?;
        validate_workspace(&snapshot.workspace)?;

        let final_path = self.directory.join(format!("{}.json", snapshot.id));
        if let Ok(bytes) = fs::read(&final_path) {
            if let Ok(previous) = serde_json::from_slice::<RecoverySnapshot>(&bytes) {
                if previous.revision > snapshot.revision {
                    return Ok(snapshot.id);
                }
            }
        }
        let bytes =
            serde_json::to_vec(&snapshot).map_err(|error| RecoveryError::Io(error.to_string()))?;
        if bytes.len() > MAX_SERIALIZED_BYTES {
            return Err(RecoveryError::LimitExceeded);
        }
        let existing = self.list()?;
        let existing_size: u64 = existing
            .iter()
            .filter(|existing| existing.id != snapshot.id)
            .filter_map(|existing| {
                fs::metadata(self.directory.join(format!("{}.json", existing.id))).ok()
            })
            .map(|metadata| metadata.len())
            .sum();
        let existing_count = existing
            .iter()
            .filter(|item| item.id != snapshot.id)
            .count();
        if existing_count >= MAX_SNAPSHOTS || existing_size + bytes.len() as u64 > MAX_TOTAL_BYTES {
            return Err(RecoveryError::LimitExceeded);
        }
        let temporary = self.directory.join(format!(
            "{}.{}.{}.tmp",
            snapshot.id,
            std::process::id(),
            now_ms()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(io_error)?;
        let result = (|| {
            file.write_all(&bytes).map_err(io_error)?;
            file.sync_all().map_err(io_error)?;
            fs::rename(&temporary, &final_path).map_err(io_error)?;
            Ok::<(), RecoveryError>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map(|()| snapshot.id)
    }

    pub fn remove(&self, id: &str) -> Result<(), RecoveryError> {
        if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(RecoveryError::InvalidSnapshot(
                "invalid recovery identifier".into(),
            ));
        }
        self.ensure_directory()?;
        match fs::remove_file(self.directory.join(format!("{id}.json"))) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(error)),
        }
    }

    pub fn remove_path(&self, workspace: &str, path: &str) -> Result<(), RecoveryError> {
        let relative = Path::new(path);
        if path.is_empty()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(RecoveryError::InvalidSnapshot(
                "invalid workspace-relative document path".into(),
            ));
        }
        let id = snapshot_id(workspace, path);
        self.remove(&id)
    }

    fn validate(&self, snapshot: &RecoverySnapshot) -> Result<(), RecoveryError> {
        let relative = Path::new(&snapshot.path);
        let error =
            if !Path::new(&snapshot.workspace).is_absolute() || snapshot.workspace.len() > 4096 {
                Some("invalid recovery workspace")
            } else if snapshot.schema_version != SCHEMA_VERSION {
                Some("unsupported recovery schema")
            } else if snapshot.id != snapshot_id(&snapshot.workspace, &snapshot.path) {
                Some("invalid recovery identifier")
            } else if snapshot.path.is_empty()
                || snapshot.path.len() > 4096
                || relative
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
            {
                Some("invalid workspace-relative document path")
            } else if snapshot.contents.len() > MAX_DOCUMENT_BYTES {
                Some("recovery document exceeds the 1 MiB limit")
            } else if !valid_hex(&snapshot.base_sha256) {
                return Err(RecoveryError::InvalidSnapshot(format!(
                    "invalid backing-file fingerprint ({} characters)",
                    snapshot.base_sha256.len()
                )));
            } else {
                None
            };
        if let Some(error) = error {
            return Err(RecoveryError::InvalidSnapshot(error.into()));
        }
        Ok(())
    }

    fn ensure_directory(&self) -> Result<(), RecoveryError> {
        if let Ok(metadata) = fs::symlink_metadata(&self.directory) {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(RecoveryError::Io(
                    "recovery directory is not a safe directory".into(),
                ));
            }
        }
        fs::create_dir_all(&self.directory).map_err(io_error)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))
                .map_err(io_error)?;
        }
        Ok(())
    }
}

fn validate_workspace(path: &str) -> Result<(), RecoveryError> {
    let workspace = Path::new(path);
    let metadata = fs::symlink_metadata(workspace).map_err(io_error)?;
    if !workspace.is_absolute() || metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RecoveryError::InvalidSnapshot(
            "recovery workspace is not a local directory".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
fn blank_snapshot(
    workspace: String,
    path: String,
    contents: String,
    base_sha256: String,
    revision: u64,
) -> RecoverySnapshot {
    RecoverySnapshot {
        schema_version: SCHEMA_VERSION,
        id: snapshot_id(&workspace, &path),
        workspace,
        path,
        contents,
        base_sha256,
        revision,
        updated_at_ms: now_ms(),
    }
}

fn snapshot_id(workspace: &str, path: &str) -> String {
    let digest = Sha256::digest(format!("{workspace}\0{path}").as_bytes());
    hex_digest(&digest)
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{:02x}", *byte)).collect()
}

fn valid_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn io_error(error: std::io::Error) -> RecoveryError {
    RecoveryError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{MAX_DOCUMENT_BYTES, RecoveryError, RecoveryStore, blank_snapshot};
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "koren-recovery-{}-{}",
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
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn snapshots_round_trip_and_explicit_discard_removes_them() {
        let fixture = Fixture::new();
        let workspace = fixture.0.join("project");
        fs::create_dir(&workspace).unwrap();
        let store = RecoveryStore::new(fixture.0.join("recovery"));
        let snapshot = blank_snapshot(
            workspace.to_string_lossy().into_owned(),
            "src/main.rs".into(),
            "fn main() { }".into(),
            "a".repeat(64),
            4,
        );
        store.write(snapshot.clone()).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        assert_eq!(store.list().unwrap()[0].contents, snapshot.contents);
        store
            .remove_path(&snapshot.workspace, &snapshot.path)
            .unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn multiple_documents_are_recovered_independently() {
        let fixture = Fixture::new();
        let workspace = fixture.0.join("project");
        fs::create_dir(&workspace).unwrap();
        let store = RecoveryStore::new(fixture.0.join("recovery"));
        for (path, contents, revision) in [
            ("src/main.rs", "main draft", 3),
            ("src/lib.rs", "library draft", 7),
        ] {
            store
                .write(blank_snapshot(
                    workspace.to_string_lossy().into_owned(),
                    path.into(),
                    contents.into(),
                    "a".repeat(64),
                    revision,
                ))
                .unwrap();
        }
        let snapshots = store.list().unwrap();
        assert_eq!(snapshots.len(), 2);
        assert!(
            snapshots
                .iter()
                .any(|snapshot| snapshot.contents == "main draft")
        );
        assert!(
            snapshots
                .iter()
                .any(|snapshot| snapshot.contents == "library draft")
        );
        let main = snapshots
            .iter()
            .find(|snapshot| snapshot.path == "src/main.rs")
            .unwrap();
        store.remove(&main.id).unwrap();
        let remaining = store.list().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].path, "src/lib.rs");
    }

    #[test]
    fn snapshots_survive_a_missing_workspace_until_explicit_recovery_or_cleanup() {
        let fixture = Fixture::new();
        let workspace = fixture.0.join("project");
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join("main.rs"), "base").unwrap();
        let store_path = fixture.0.join("recovery");
        let store = RecoveryStore::new(store_path.clone());
        let snapshot = blank_snapshot(
            workspace.to_string_lossy().into_owned(),
            "main.rs".into(),
            "draft".into(),
            "a".repeat(64),
            2,
        );
        let id = store.write(snapshot).unwrap();
        fs::remove_file(workspace.join("main.rs")).unwrap();
        assert!(super::fingerprint_file(workspace.to_str().unwrap(), "main.rs").is_err());
        fs::remove_dir(&workspace).unwrap();
        assert_eq!(
            RecoveryStore::new(store_path.clone()).list().unwrap().len(),
            1
        );
        let file = fs::read_dir(&store_path)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut saved: super::RecoverySnapshot =
            serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        saved.updated_at_ms = 0;
        fs::write(&file, serde_json::to_vec(&saved).unwrap()).unwrap();
        assert!(
            RecoveryStore::new(store_path.clone())
                .list()
                .unwrap()
                .is_empty()
        );
        assert!(!store_path.join(format!("{id}.json")).exists());
    }

    #[cfg(unix)]
    #[test]
    fn recovery_directory_and_snapshots_are_private_to_the_current_user() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let workspace = fixture.0.join("project");
        fs::create_dir(&workspace).unwrap();
        let store = RecoveryStore::new(fixture.0.join("recovery"));
        let snapshot = blank_snapshot(
            workspace.to_string_lossy().into_owned(),
            "main.rs".into(),
            "draft".into(),
            "a".repeat(64),
            1,
        );
        let id = store.write(snapshot).unwrap();
        assert_eq!(
            fs::metadata(&store.directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(store.directory.join(format!("{id}.json")))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn snapshots_reject_unsafe_paths_and_oversized_contents() {
        let fixture = Fixture::new();
        let workspace = fixture.0.join("project");
        fs::create_dir(&workspace).unwrap();
        let store = RecoveryStore::new(fixture.0.join("recovery"));
        let mut unsafe_path = blank_snapshot(
            workspace.to_string_lossy().into_owned(),
            "../outside.rs".into(),
            "draft".into(),
            "a".repeat(64),
            1,
        );
        unsafe_path.id.clear();
        assert!(matches!(
            store.write(unsafe_path),
            Err(RecoveryError::InvalidSnapshot(_))
        ));
        let large = "x".repeat(MAX_DOCUMENT_BYTES + 1);
        let snapshot = blank_snapshot(
            workspace.to_string_lossy().into_owned(),
            "large.rs".into(),
            large,
            "a".repeat(64),
            1,
        );
        assert!(matches!(
            store.write(snapshot),
            Err(RecoveryError::InvalidSnapshot(_))
        ));
    }

    #[test]
    fn corrupt_and_unsupported_files_are_ignored_without_source_logging() {
        let fixture = Fixture::new();
        let workspace = fixture.0.join("project");
        fs::create_dir(&workspace).unwrap();
        let store = RecoveryStore::new(fixture.0.join("recovery"));
        store.ensure_directory().unwrap();
        fs::write(store.directory.join("broken.json"), b"{not json").unwrap();
        let mut unsupported = blank_snapshot(
            workspace.to_string_lossy().into_owned(),
            "src/main.rs".into(),
            "secret source".into(),
            "a".repeat(64),
            1,
        );
        unsupported.schema_version = 2;
        fs::write(
            store.directory.join("future.json"),
            serde_json::to_vec(&unsupported).unwrap(),
        )
        .unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn backing_file_fingerprint_changes_when_the_file_is_replaced() {
        let fixture = Fixture::new();
        let workspace = fixture.0.join("project");
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join("main.rs"), "before").unwrap();
        let first = super::fingerprint_file(workspace.to_str().unwrap(), "main.rs").unwrap();
        assert!(super::valid_hex(&first), "fingerprint was {}", first);
        fs::write(workspace.join("main.rs"), "after").unwrap();
        let second = super::fingerprint_file(workspace.to_str().unwrap(), "main.rs").unwrap();
        assert_ne!(first, second);
        assert!(super::fingerprint_file(workspace.to_str().unwrap(), "../outside").is_err());
    }
}
