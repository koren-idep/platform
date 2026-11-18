//! Language-neutral external tool declarations, bounded discovery, and verified managed installations.
//!
//! Declaring or resolving a tool never installs or executes it. Managed installations require
//! explicit user approval, host trust policy validation, and integrity verification before staging.

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

/// Maximum size of a downloaded tool archive (256 MiB).
pub const DEFAULT_MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;
/// Maximum total extracted bytes from one archive (512 MiB).
pub const DEFAULT_MAX_EXTRACT_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum number of entries in a single archive.
pub const DEFAULT_MAX_ARCHIVE_ENTRIES: usize = 8192;

/// Tool category shown in Settings -> Tools & Runtimes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolCategory {
    /// Language Server Protocol server.
    LanguageServer,
    /// Standalone language runtime.
    Runtime,
    /// Compiler toolchain.
    Compiler,
    /// Source code formatter.
    Formatter,
    /// Static linter.
    Linter,
    /// Build or task runner tool.
    BuildTool,
    /// Debug Adapter Protocol server.
    Debugger,
}

/// Supported operating system target.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPlatform {
    /// Linux.
    Linux,
    /// macOS.
    MacOs,
    /// Windows.
    Windows,
}

impl ToolPlatform {
    /// Returns the host operating system platform.
    #[must_use]
    pub const fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Self::Linux
        }
    }
}

/// Supported CPU architecture target.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolArch {
    /// 64-bit x86.
    X86_64,
    /// 64-bit ARM.
    Aarch64,
}

impl ToolArch {
    /// Returns the host CPU architecture.
    #[must_use]
    pub const fn current() -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            Self::Aarch64
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            Self::X86_64
        }
    }
}

/// Origin of a resolved or installed tool executable.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSource {
    /// Detected from PATH or standard system locations.
    System,
    /// Manually chosen by the user.
    Manual,
    /// Downloaded and verified into Koren's managed storage.
    Managed,
}

/// Precedence level that selected the resolved tool.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSelectionScope {
    /// Explicit configuration-level override.
    Configuration,
    /// Workspace-level pinned selection.
    Workspace,
    /// User-level default selection.
    User,
    /// Compatible detected executable fallback.
    Detected,
}

/// Installation lifecycle state machine.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum ToolInstallState {
    /// Tool is not installed in managed storage.
    NotInstalled,
    /// User selected a specific version.
    VersionSelected {
        /// Selected version string.
        version: String,
    },
    /// Awaiting explicit user download approval.
    AwaitingApproval {
        /// Selected version string.
        version: String,
    },
    /// Downloading artifact into temporary staging.
    Downloading {
        /// Selected version string.
        version: String,
    },
    /// Verifying SHA-256 integrity against trusted metadata.
    Verifying {
        /// Selected version string.
        version: String,
    },
    /// Extracting and validating executable in staging directory.
    Staging {
        /// Selected version string.
        version: String,
    },
    /// Verified and atomically placed into managed storage.
    Installed {
        /// Installed version string.
        version: String,
    },
    /// Installation failed and can be retried.
    Failed {
        /// Human-readable error message.
        message: String,
    },
    /// Installation was cancelled by the user.
    Cancelled,
}

/// Acquisition provider category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionKind {
    /// Verified static release archive or binary.
    StaticRelease,
    /// Approved package metadata.
    PackageMetadata,
    /// Local existing installation only.
    Local,
    /// Dedicated toolchain manager (e.g. rustup).
    ToolchainManager,
}

/// One downloadable release candidate for a tool.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolRelease {
    /// Exact pinned version string (never mutable "latest").
    pub version: String,
    /// Target platform.
    pub platform: ToolPlatform,
    /// Target architecture.
    pub arch: ToolArch,
    /// Acquisition provider kind.
    pub kind: AcquisitionKind,
    /// Publisher display name.
    pub publisher: String,
    /// Download URL (must be HTTPS in production).
    pub url: String,
    /// Expected lowercase hex SHA-256 digest from trusted metadata.
    pub sha256: String,
    /// Optional download size in bytes.
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// Optional license identifier (SPDX or summary).
    #[serde(default)]
    pub license: Option<String>,
    /// Relative path inside an archive to the main executable, or executable filename.
    pub executable_rel_path: String,
}

/// Declarative tool metadata contributed by an extension or host.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolDeclaration {
    /// Stable tool identifier (e.g. `lua-language-server`, `lune`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Short description.
    pub description: String,
    /// Tool category.
    pub category: ToolCategory,
    /// Whether the tool is optional for the declaring extension.
    #[serde(default = "default_true")]
    pub optional: bool,
    /// Candidate executable names to search for on PATH / hints.
    pub executables: Vec<String>,
    /// Optional explicit local directories to check (never recursive).
    #[serde(default)]
    pub discovery_hints: Vec<String>,
    /// Supported platforms (empty means all).
    #[serde(default)]
    pub platforms: Vec<ToolPlatform>,
    /// Supported architectures (empty means all).
    #[serde(default)]
    pub architectures: Vec<ToolArch>,
    /// Optional version requirement label.
    #[serde(default)]
    pub version_requirement: Option<String>,
    /// Optional managed release candidates.
    #[serde(default)]
    pub releases: Vec<ToolRelease>,
}

const fn default_true() -> bool {
    true
}

/// Host-controlled trust policy for managed downloads.
#[derive(Clone, Debug)]
pub struct ToolTrustPolicy {
    /// Allowed URL prefixes (e.g. `https://github.com/LuaLS/`).
    pub trusted_url_prefixes: Vec<String>,
    /// Allow `http://127.0.0.1` or `file://` for automated tests only.
    pub allow_local_test_urls: bool,
    /// Maximum download size in bytes.
    pub max_download_bytes: u64,
    /// Maximum extracted bytes.
    pub max_extract_bytes: u64,
}

impl Default for ToolTrustPolicy {
    fn default() -> Self {
        Self {
            trusted_url_prefixes: Vec::new(),
            allow_local_test_urls: false,
            max_download_bytes: DEFAULT_MAX_DOWNLOAD_BYTES,
            max_extract_bytes: DEFAULT_MAX_EXTRACT_BYTES,
        }
    }
}

impl ToolTrustPolicy {
    /// Validates whether a release is permitted by this host policy.
    ///
    /// # Errors
    /// Returns an error when the release uses an untrusted URL, mutable version, or missing digest.
    pub fn validate_release(&self, release: &ToolRelease) -> Result<(), ToolError> {
        if release.version.trim().is_empty()
            || release.version.eq_ignore_ascii_case("latest")
            || release.version.contains(['/', '\\', '\0'])
            || release.version.contains("..")
        {
            return Err(ToolError(
                "release version must be an exact pinned version".into(),
            ));
        }
        if !is_valid_sha256(&release.sha256) {
            return Err(ToolError(
                "release requires a valid 64-character hex SHA-256 digest".into(),
            ));
        }
        let is_https = release.url.starts_with("https://");
        let is_local_test = self.allow_local_test_urls
            && (release.url.starts_with("http://127.0.0.1:") || release.url.starts_with("file://"));
        if !is_https && !is_local_test {
            return Err(ToolError(
                "managed tool downloads require HTTPS or an approved local test URL".into(),
            ));
        }
        if !is_local_test
            && !self
                .trusted_url_prefixes
                .iter()
                .any(|prefix| release.url.starts_with(prefix))
        {
            return Err(ToolError(format!(
                "download URL '{}' is not in the host's trusted source policy",
                release.url
            )));
        }
        if let Some(size) = release.size_bytes
            && size > self.max_download_bytes
        {
            return Err(ToolError("release size exceeds host download limit".into()));
        }
        Ok(())
    }
}

/// Record of an installed managed version stored on disk.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstalledManagedTool {
    /// Tool identifier.
    pub tool_id: String,
    /// Installed version.
    pub version: String,
    /// Path to the verified executable.
    pub executable_path: PathBuf,
    /// SHA-256 of the verified artifact.
    pub artifact_sha256: String,
    /// SHA-256 of the installed executable for tamper/corruption detection.
    pub executable_sha256: String,
    /// Publisher name.
    pub publisher: String,
    /// Original download URL.
    pub source_url: String,
}

/// Persisted selection state for one tool.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolSelection {
    /// Selected source.
    pub source: Option<ToolSource>,
    /// Selected version (for managed tools).
    pub version: Option<String>,
    /// Explicit executable path (for manual or system selections).
    pub executable_path: Option<PathBuf>,
}

/// Persisted tool manager configuration (`tools-state.json`).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolStateFile {
    /// Schema version (1).
    pub schema: u32,
    /// User-level default selections keyed by tool ID.
    #[serde(default)]
    pub user_selections: BTreeMap<String, ToolSelection>,
    /// Workspace-level selections keyed by canonical workspace path then tool ID.
    #[serde(default)]
    pub workspace_selections: BTreeMap<String, BTreeMap<String, ToolSelection>>,
    /// Installed managed tool records.
    #[serde(default)]
    pub managed: Vec<InstalledManagedTool>,
    /// Dismissed optional setup prompts (tool IDs).
    #[serde(default)]
    pub dismissed_prompts: BTreeSet<String>,
}

/// Safe resolved-tool reference returned to callers and extensions.
///
/// Resolving a tool never authorizes its execution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolvedTool {
    /// Tool identifier.
    pub tool_id: String,
    /// Absolute path to the selected executable.
    pub executable_path: PathBuf,
    /// Origin of the executable.
    pub source: ToolSource,
    /// Precedence scope that produced this resolution.
    pub scope: ToolSelectionScope,
    /// Version string if known.
    pub version: Option<String>,
}

/// One discovered local candidate executable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiscoveredExecutable {
    /// Executable path on disk.
    pub path: PathBuf,
    /// Origin (`System` when found via PATH/hints).
    pub source: ToolSource,
}

/// Tool Manager error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolError(pub String);
impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ToolError {}

/// Manages tool declarations, bounded discovery, selection precedence, and verified managed installs.
pub struct ToolManager {
    storage_root: PathBuf,
    state_path: PathBuf,
    state: ToolStateFile,
    declarations: BTreeMap<String, (BTreeSet<String>, ToolDeclaration)>,
    trust_policy: ToolTrustPolicy,
    install_states: BTreeMap<String, ToolInstallState>,
}

impl ToolManager {
    /// Creates or loads a Tool Manager backed by `storage_root` (e.g. XDG data `koren/tools`).
    ///
    /// # Errors
    /// Returns an error if the existing state file is corrupt and cannot be parsed.
    pub fn new(storage_root: PathBuf, trust_policy: ToolTrustPolicy) -> Result<Self, ToolError> {
        let state_path = storage_root.join("tools-state.json");
        let state = load_state_file(&state_path)?;
        Ok(Self {
            storage_root,
            state_path,
            state,
            declarations: BTreeMap::new(),
            trust_policy,
            install_states: BTreeMap::new(),
        })
    }

    /// Returns the managed storage root directory.
    #[must_use]
    pub fn storage_root(&self) -> &Path {
        &self.storage_root
    }

    /// Registers a tool declaration under an owner (extension ID or `"host"`).
    ///
    /// # Errors
    /// Returns an error if the tool ID or executable names are invalid.
    pub fn register_declaration(
        &mut self,
        owner: &str,
        declaration: ToolDeclaration,
    ) -> Result<(), ToolError> {
        validate_tool_id(&declaration.id)?;
        if declaration.name.trim().is_empty() || declaration.executables.is_empty() {
            return Err(ToolError(
                "tool declaration requires a name and at least one executable".into(),
            ));
        }
        if declaration
            .executables
            .iter()
            .any(|e| e.trim().is_empty() || e.contains(['/', '\\', '\0']))
        {
            return Err(ToolError(
                "tool executable names must be bare filenames without path separators".into(),
            ));
        }
        let entry = self
            .declarations
            .entry(declaration.id.clone())
            .or_insert_with(|| (BTreeSet::new(), declaration.clone()));
        entry.0.insert(owner.to_owned());
        entry.1 = declaration;
        Ok(())
    }

    /// Removes an owner's tool declarations (for example when an extension is disabled).
    pub fn unregister_owner(&mut self, owner: &str) {
        self.declarations.retain(|_, (owners, _)| {
            owners.remove(owner);
            !owners.is_empty()
        });
    }

    /// Lists all active tool declarations along with their declaring owners.
    #[must_use]
    pub fn declarations(&self) -> Vec<(&ToolDeclaration, Vec<&str>)> {
        self.declarations
            .values()
            .map(|(owners, decl)| (decl, owners.iter().map(String::as_str).collect()))
            .collect()
    }

    /// Lists installed managed versions for a tool.
    #[must_use]
    pub fn managed_versions(&self, tool_id: &str) -> Vec<&InstalledManagedTool> {
        self.state
            .managed
            .iter()
            .filter(|m| m.tool_id == tool_id)
            .collect()
    }

    /// Returns the current installation state for a tool.
    #[must_use]
    pub fn install_state(&self, tool_id: &str) -> ToolInstallState {
        self.install_states
            .get(tool_id)
            .cloned()
            .unwrap_or(ToolInstallState::NotInstalled)
    }

    /// Dismisses an optional tool setup prompt so the user is not repeatedly interrupted.
    ///
    /// # Errors
    /// Returns an error if persisting the state file fails.
    pub fn dismiss_prompt(&mut self, tool_id: &str) -> Result<(), ToolError> {
        self.state.dismissed_prompts.insert(tool_id.to_owned());
        save_state_file(&self.state_path, &self.state)
    }

    /// Returns whether the setup prompt for `tool_id` was dismissed.
    #[must_use]
    pub fn is_prompt_dismissed(&self, tool_id: &str) -> bool {
        self.state.dismissed_prompts.contains(tool_id)
    }

    /// Selects a manual executable path for `tool_id` in user or workspace scope.
    ///
    /// # Errors
    /// Returns an error if the path is not an existing file or persisting fails.
    pub fn select_manual(
        &mut self,
        tool_id: &str,
        executable_path: PathBuf,
        workspace_key: Option<&str>,
    ) -> Result<(), ToolError> {
        validate_tool_id(tool_id)?;
        if !executable_path.is_absolute() || !executable_path.is_file() {
            return Err(ToolError(
                "manual tool path must be an existing absolute file".into(),
            ));
        }
        let selection = ToolSelection {
            source: Some(ToolSource::Manual),
            version: None,
            executable_path: Some(executable_path),
        };
        self.store_selection(tool_id, selection, workspace_key)
    }

    /// Selects a managed version for `tool_id` in user or workspace scope.
    ///
    /// # Errors
    /// Returns an error if the managed version is not installed or is missing on disk.
    pub fn select_managed(
        &mut self,
        tool_id: &str,
        version: &str,
        workspace_key: Option<&str>,
    ) -> Result<(), ToolError> {
        let installed = self
            .state
            .managed
            .iter()
            .find(|m| m.tool_id == tool_id && m.version == version)
            .ok_or_else(|| ToolError(format!("managed version '{version}' is not installed")))?
            .clone();
        verify_installed_integrity(&installed)?;
        let selection = ToolSelection {
            source: Some(ToolSource::Managed),
            version: Some(version.to_owned()),
            executable_path: Some(installed.executable_path),
        };
        self.store_selection(tool_id, selection, workspace_key)
    }

    /// Selects a discovered system executable for `tool_id` in user or workspace scope.
    ///
    /// # Errors
    /// Returns an error if the path does not exist or persisting fails.
    pub fn select_system(
        &mut self,
        tool_id: &str,
        executable_path: PathBuf,
        workspace_key: Option<&str>,
    ) -> Result<(), ToolError> {
        validate_tool_id(tool_id)?;
        if !executable_path.is_absolute() || !executable_path.is_file() {
            return Err(ToolError(
                "system tool path must be an existing absolute file".into(),
            ));
        }
        let selection = ToolSelection {
            source: Some(ToolSource::System),
            version: None,
            executable_path: Some(executable_path),
        };
        self.store_selection(tool_id, selection, workspace_key)
    }

    fn store_selection(
        &mut self,
        tool_id: &str,
        selection: ToolSelection,
        workspace_key: Option<&str>,
    ) -> Result<(), ToolError> {
        if let Some(ws) = workspace_key {
            self.state
                .workspace_selections
                .entry(ws.to_owned())
                .or_default()
                .insert(tool_id.to_owned(), selection);
        } else {
            self.state
                .user_selections
                .insert(tool_id.to_owned(), selection);
        }
        save_state_file(&self.state_path, &self.state)
    }

    /// Bounded static discovery on `PATH` and declaration hints (never scans entire disks).
    #[must_use]
    pub fn discover_local(&self, tool_id: &str) -> Vec<DiscoveredExecutable> {
        let Some((_, decl)) = self.declarations.get(tool_id) else {
            return Vec::new();
        };
        let mut search_dirs = Vec::new();
        for hint in &decl.discovery_hints {
            let expanded = expand_tilde(hint);
            if expanded.is_dir() {
                search_dirs.push(expanded);
            }
        }
        if let Some(path_env) = std::env::var_os("PATH") {
            search_dirs.extend(std::env::split_paths(&path_env));
        }
        let mut seen = BTreeSet::new();
        let mut found = Vec::new();
        for dir in search_dirs {
            for name in &decl.executables {
                let candidate = dir.join(name);
                if candidate.is_file() && seen.insert(candidate.clone()) {
                    found.push(DiscoveredExecutable {
                        path: candidate,
                        source: ToolSource::System,
                    });
                }
            }
        }
        found
    }

    /// Resolves `tool_id` following strict precedence:
    /// 1. Configuration override
    /// 2. Workspace selection
    /// 3. User default selection
    /// 4. Compatible detected executable
    ///
    /// Never silently switches a pinned version if it becomes missing; returns an explicit error.
    ///
    /// # Errors
    /// Returns an error if a pinned selection is missing/corrupt or no compatible executable is found.
    pub fn resolve(
        &self,
        tool_id: &str,
        config_override: Option<&ToolSelection>,
        workspace_key: Option<&str>,
    ) -> Result<ResolvedTool, ToolError> {
        validate_tool_id(tool_id)?;
        if let Some(sel) = config_override {
            return self.resolve_selection(tool_id, sel, ToolSelectionScope::Configuration);
        }
        if let Some(ws) = workspace_key
            && let Some(sel) = self
                .state
                .workspace_selections
                .get(ws)
                .and_then(|map| map.get(tool_id))
        {
            return self.resolve_selection(tool_id, sel, ToolSelectionScope::Workspace);
        }
        if let Some(sel) = self.state.user_selections.get(tool_id) {
            return self.resolve_selection(tool_id, sel, ToolSelectionScope::User);
        }
        if let Some(first) = self.discover_local(tool_id).into_iter().next() {
            return Ok(ResolvedTool {
                tool_id: tool_id.to_owned(),
                executable_path: first.path,
                source: first.source,
                scope: ToolSelectionScope::Detected,
                version: None,
            });
        }
        Err(ToolError(format!(
            "tool '{tool_id}' is not configured or detected"
        )))
    }

    fn resolve_selection(
        &self,
        tool_id: &str,
        sel: &ToolSelection,
        scope: ToolSelectionScope,
    ) -> Result<ResolvedTool, ToolError> {
        let source = sel
            .source
            .ok_or_else(|| ToolError("selection has no source".into()))?;
        if source == ToolSource::Managed {
            let version = sel
                .version
                .as_deref()
                .ok_or_else(|| ToolError("managed selection missing version".into()))?;
            let installed = self
                .state
                .managed
                .iter()
                .find(|m| m.tool_id == tool_id && m.version == version)
                .ok_or_else(|| {
                    ToolError(format!(
                        "pinned managed version '{version}' of '{tool_id}' is no longer installed"
                    ))
                })?;
            verify_installed_integrity(installed)?;
            return Ok(ResolvedTool {
                tool_id: tool_id.to_owned(),
                executable_path: installed.executable_path.clone(),
                source: ToolSource::Managed,
                scope,
                version: Some(installed.version.clone()),
            });
        }
        let path = sel
            .executable_path
            .as_ref()
            .ok_or_else(|| ToolError("selection missing executable path".into()))?;
        if !path.is_file() {
            return Err(ToolError(format!(
                "configured executable '{}' is missing",
                path.display()
            )));
        }
        Ok(ResolvedTool {
            tool_id: tool_id.to_owned(),
            executable_path: path.clone(),
            source,
            scope,
            version: sel.version.clone(),
        })
    }

    /// Downloads (via approved release metadata), verifies SHA-256, safely extracts/stages, and
    /// atomically installs a managed tool version.
    ///
    /// # Errors
    /// Returns an error if trust validation, download, checksum verification, extraction, or
    /// placement fails. Failed/cancelled installs clean up temporary staging files.
    pub fn install_managed_release(
        &mut self,
        tool_id: &str,
        release: &ToolRelease,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> Result<InstalledManagedTool, ToolError> {
        validate_tool_id(tool_id)?;
        self.trust_policy.validate_release(release)?;
        self.install_states.insert(
            tool_id.to_owned(),
            ToolInstallState::Downloading {
                version: release.version.clone(),
            },
        );

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| ToolError(e.to_string()))?
            .as_nanos();
        let staging_dir = self
            .storage_root
            .join(".staging")
            .join(format!("{tool_id}-{}-{nonce}", release.version));
        fs::create_dir_all(&staging_dir).map_err(|e| ToolError(e.to_string()))?;

        let result = self.run_staged_install(tool_id, release, &staging_dir, cancel);
        if let Err(ref err) = result {
            let _ = fs::remove_dir_all(&staging_dir);
            let next_state = if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                ToolInstallState::Cancelled
            } else {
                ToolInstallState::Failed {
                    message: err.0.clone(),
                }
            };
            self.install_states.insert(tool_id.to_owned(), next_state);
        } else {
            let _ = fs::remove_dir_all(&staging_dir);
            self.install_states.insert(
                tool_id.to_owned(),
                ToolInstallState::Installed {
                    version: release.version.clone(),
                },
            );
        }
        result
    }

    fn run_staged_install(
        &mut self,
        tool_id: &str,
        release: &ToolRelease,
        staging_dir: &Path,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> Result<InstalledManagedTool, ToolError> {
        check_cancelled(cancel)?;
        let artifact_path = staging_dir.join("artifact.bin");
        download_artifact(
            &release.url,
            &artifact_path,
            self.trust_policy.max_download_bytes,
        )?;
        check_cancelled(cancel)?;

        self.install_states.insert(
            tool_id.to_owned(),
            ToolInstallState::Verifying {
                version: release.version.clone(),
            },
        );
        let actual_sha = sha256_file(&artifact_path)?;
        if actual_sha != release.sha256.to_ascii_lowercase() {
            return Err(ToolError(format!(
                "SHA-256 mismatch: expected {}, got {actual_sha}",
                release.sha256
            )));
        }
        check_cancelled(cancel)?;

        self.install_states.insert(
            tool_id.to_owned(),
            ToolInstallState::Staging {
                version: release.version.clone(),
            },
        );
        let extract_dir = staging_dir.join("extracted");
        fs::create_dir_all(&extract_dir).map_err(|e| ToolError(e.to_string()))?;

        let lower_url = release.url.to_ascii_lowercase();
        if lower_url.ends_with(".tar.gz")
            || Path::new(&lower_url)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("tgz"))
        {
            extract_tar_gz_safely(
                &artifact_path,
                &extract_dir,
                self.trust_policy.max_extract_bytes,
                DEFAULT_MAX_ARCHIVE_ENTRIES,
                cancel,
            )?;
        } else {
            let rel = validate_relative_path(Path::new(&release.executable_rel_path))?;
            let dest = extract_dir.join(rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| ToolError(e.to_string()))?;
            }
            fs::copy(&artifact_path, &dest).map_err(|e| ToolError(e.to_string()))?;
        }

        let rel_exec = validate_relative_path(Path::new(&release.executable_rel_path))?;
        let staged_exec = extract_dir.join(&rel_exec);
        if !staged_exec.is_file() {
            return Err(ToolError(format!(
                "expected executable '{}' not found in staged artifact",
                rel_exec.display()
            )));
        }
        set_executable_permissions(&staged_exec)?;

        let final_dir = self.storage_root.join(tool_id).join(&release.version);
        if final_dir.exists() {
            fs::remove_dir_all(&final_dir).map_err(|e| ToolError(e.to_string()))?;
        }
        if let Some(parent) = final_dir.parent() {
            fs::create_dir_all(parent).map_err(|e| ToolError(e.to_string()))?;
        }
        fs::rename(&extract_dir, &final_dir).map_err(|e| ToolError(e.to_string()))?;
        let final_exec = final_dir.join(&rel_exec);
        let exec_sha = sha256_file(&final_exec)?;

        let record = InstalledManagedTool {
            tool_id: tool_id.to_owned(),
            version: release.version.clone(),
            executable_path: final_exec,
            artifact_sha256: actual_sha,
            executable_sha256: exec_sha,
            publisher: release.publisher.clone(),
            source_url: release.url.clone(),
        };
        self.state
            .managed
            .retain(|m| !(m.tool_id == tool_id && m.version == release.version));
        self.state.managed.push(record.clone());
        save_state_file(&self.state_path, &self.state)?;
        Ok(record)
    }

    /// Removes a managed version. Warns/refuses if currently selected by user or workspace unless
    /// `force` is true. Never deletes system-installed executables.
    ///
    /// # Errors
    /// Returns an error if the version is in use (and `!force`) or filesystem removal fails.
    pub fn remove_managed_version(
        &mut self,
        tool_id: &str,
        version: &str,
        force: bool,
    ) -> Result<(), ToolError> {
        let in_use_user = self.state.user_selections.get(tool_id).is_some_and(|s| {
            s.source == Some(ToolSource::Managed) && s.version.as_deref() == Some(version)
        });
        let in_use_ws = self.state.workspace_selections.values().any(|map| {
            map.get(tool_id).is_some_and(|s| {
                s.source == Some(ToolSource::Managed) && s.version.as_deref() == Some(version)
            })
        });
        if (in_use_user || in_use_ws) && !force {
            return Err(ToolError(format!(
                "managed version '{version}' of '{tool_id}' is currently selected; confirm removal to proceed"
            )));
        }
        let target_dir = self.storage_root.join(tool_id).join(version);
        if target_dir.exists() {
            fs::remove_dir_all(&target_dir).map_err(|e| ToolError(e.to_string()))?;
        }
        self.state
            .managed
            .retain(|m| !(m.tool_id == tool_id && m.version == version));
        save_state_file(&self.state_path, &self.state)
    }
}

fn verify_installed_integrity(installed: &InstalledManagedTool) -> Result<(), ToolError> {
    if !installed.executable_path.is_file() {
        return Err(ToolError(format!(
            "managed executable '{}' is missing",
            installed.executable_path.display()
        )));
    }
    let current_sha = sha256_file(&installed.executable_path)?;
    if current_sha != installed.executable_sha256 {
        return Err(ToolError(format!(
            "managed executable '{}' was modified outside Koren",
            installed.executable_path.display()
        )));
    }
    Ok(())
}

fn check_cancelled(cancel: Option<&Arc<AtomicBool>>) -> Result<(), ToolError> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        Err(ToolError("installation cancelled".into()))
    } else {
        Ok(())
    }
}

fn download_artifact(url: &str, dest: &Path, max_bytes: u64) -> Result<(), ToolError> {
    if let Some(file_path) = url.strip_prefix("file://") {
        let meta = fs::metadata(file_path).map_err(|e| ToolError(e.to_string()))?;
        if meta.len() > max_bytes {
            return Err(ToolError("artifact exceeds maximum download size".into()));
        }
        fs::copy(file_path, dest).map_err(|e| ToolError(e.to_string()))?;
        return Ok(());
    }
    let proto = if url.starts_with("https://") {
        "--proto=https"
    } else {
        "--proto=http"
    };
    let output = Command::new("curl")
        .arg(proto)
        .arg("--tlsv1.2")
        .arg("--fail")
        .arg("--silent")
        .arg("--show-error")
        .arg("--location")
        .arg("--connect-timeout")
        .arg("15")
        .arg("--max-filesize")
        .arg(max_bytes.to_string())
        .arg("--output")
        .arg(dest)
        .arg(url)
        .output()
        .map_err(|e| ToolError(format!("failed to invoke curl for download: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ToolError(format!("download failed: {}", stderr.trim())));
    }
    Ok(())
}

fn extract_tar_gz_safely(
    archive_path: &Path,
    dest_dir: &Path,
    max_extract_bytes: u64,
    max_entries: usize,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<(), ToolError> {
    let file = File::open(archive_path).map_err(|e| ToolError(e.to_string()))?;
    let mut decoder = GzDecoder::new(file);
    let mut header = [0u8; 512];
    let mut total_bytes: u64 = 0;
    let mut entries = 0usize;

    loop {
        check_cancelled(cancel)?;
        let n = read_exact_or_eof(&mut decoder, &mut header)?;
        if n == 0 || header.iter().all(|&b| b == 0) {
            break;
        }
        if n < 512 {
            return Err(ToolError("truncated tar header in archive".into()));
        }
        entries += 1;
        if entries > max_entries {
            return Err(ToolError("archive exceeds maximum entry count".into()));
        }
        let typeflag = header[156];
        if matches!(typeflag, b'1' | b'2') {
            return Err(ToolError(
                "archive contains disallowed link or symlink entry".into(),
            ));
        }
        let raw_name = cstr_bytes(&header[0..100]);
        let raw_prefix = cstr_bytes(&header[345..500]);
        let full_rel = if raw_prefix.is_empty() {
            raw_name
        } else {
            format!("{raw_prefix}/{raw_name}")
        };
        let rel_path = validate_relative_path(Path::new(&full_rel))?;
        let size = parse_octal(&header[124..136])?;
        total_bytes = total_bytes
            .checked_add(size)
            .ok_or_else(|| ToolError("archive size overflow".into()))?;
        if total_bytes > max_extract_bytes {
            return Err(ToolError("extracted archive exceeds size limit".into()));
        }

        let target = dest_dir.join(&rel_path);
        if typeflag == b'5' {
            fs::create_dir_all(&target).map_err(|e| ToolError(e.to_string()))?;
        } else if typeflag == 0 || typeflag == b'0' {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| ToolError(e.to_string()))?;
            }
            let mut out = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|e| ToolError(e.to_string()))?;
            let mut remaining = size;
            let mut buf = [0u8; 8192];
            while remaining > 0 {
                let to_read = usize::try_from(remaining.min(buf.len() as u64)).unwrap_or(buf.len());
                decoder
                    .read_exact(&mut buf[..to_read])
                    .map_err(|e| ToolError(format!("corrupt archive payload: {e}")))?;
                out.write_all(&buf[..to_read])
                    .map_err(|e| ToolError(e.to_string()))?;
                remaining -= to_read as u64;
            }
            let padding = (512 - (size % 512)) % 512;
            if padding > 0 {
                let pad_len = usize::try_from(padding).unwrap_or(0);
                decoder
                    .read_exact(&mut header[..pad_len])
                    .map_err(|e| ToolError(format!("corrupt archive padding: {e}")))?;
            }
        } else {
            // Skip metadata/extended headers safely.
            let blocks = size.div_ceil(512);
            let mut skip_buf = [0u8; 512];
            for _ in 0..blocks {
                decoder
                    .read_exact(&mut skip_buf)
                    .map_err(|e| ToolError(e.to_string()))?;
            }
        }
    }
    Ok(())
}

fn read_exact_or_eof(reader: &mut impl Read, buf: &mut [u8]) -> Result<usize, ToolError> {
    let mut offset = 0;
    while offset < buf.len() {
        match reader.read(&mut buf[offset..]) {
            Ok(0) => return Ok(offset),
            Ok(n) => offset += n,
            Err(e) => return Err(ToolError(format!("archive read error: {e}"))),
        }
    }
    Ok(offset)
}

fn cstr_bytes(slice: &[u8]) -> String {
    let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    String::from_utf8_lossy(&slice[..end]).into_owned()
}

fn parse_octal(slice: &[u8]) -> Result<u64, ToolError> {
    let text = cstr_bytes(slice);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(trimmed, 8).map_err(|_| ToolError("invalid octal size in archive".into()))
}

fn validate_relative_path(path: &Path) -> Result<PathBuf, ToolError> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(ToolError(
            "archive path must be relative and non-empty".into(),
        ));
    }
    let mut clean = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ToolError("archive path contains unsafe traversal".into()));
            }
        }
    }
    if clean.as_os_str().is_empty() {
        return Err(ToolError("archive path resolves to empty".into()));
    }
    Ok(clean)
}

fn set_executable_permissions(path: &Path) -> Result<(), ToolError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)
            .map_err(|e| ToolError(e.to_string()))?
            .permissions();
        perms.set_mode(perms.mode() | 0o755);
        fs::set_permissions(path, perms).map_err(|e| ToolError(e.to_string()))?;
    }
    let _ = path;
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, ToolError> {
    let mut file = File::open(path).map_err(|e| ToolError(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).map_err(|e| ToolError(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        let _ = fmt::write(&mut hex, format_args!("{b:02x}"));
    }
    Ok(hex)
}

fn is_valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn validate_tool_id(id: &str) -> Result<(), ToolError> {
    if id.is_empty()
        || id.len() > 128
        || !id.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'_')
        })
    {
        return Err(ToolError(format!("invalid tool ID '{id}'")));
    }
    Ok(())
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

fn load_state_file(path: &Path) -> Result<ToolStateFile, ToolError> {
    match fs::read_to_string(path) {
        Ok(raw) => {
            let state: ToolStateFile =
                serde_json::from_str(&raw).map_err(|e| ToolError(e.to_string()))?;
            if state.schema != 1 {
                return Err(ToolError(format!(
                    "unsupported tool state schema {}",
                    state.schema
                )));
            }
            Ok(state)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ToolStateFile {
            schema: 1,
            ..ToolStateFile::default()
        }),
        Err(e) => Err(ToolError(e.to_string())),
    }
}

fn save_state_file(path: &Path, state: &ToolStateFile) -> Result<(), ToolError> {
    let parent = path
        .parent()
        .ok_or_else(|| ToolError("tool state path has no parent".into()))?;
    fs::create_dir_all(parent).map_err(|e| ToolError(e.to_string()))?;
    let serialized = serde_json::to_string_pretty(state).map_err(|e| ToolError(e.to_string()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| ToolError(e.to_string()))?
        .as_nanos();
    let tmp = parent.join(format!(".tools-state-{}-{nonce}.tmp", std::process::id()));
    let res = (|| -> std::io::Result<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(serialized.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res.map_err(|e| ToolError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("koren-tools-{label}-{nonce}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn build_tar_gz(entries: &[(&str, &[u8], u8)]) -> Vec<u8> {
        let mut tar = Vec::new();
        for (name, data, typeflag) in entries {
            let mut header = [0u8; 512];
            let name_bytes = name.as_bytes();
            header[..name_bytes.len()].copy_from_slice(name_bytes);
            header[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
            header[156] = *typeflag;
            header[257..263].copy_from_slice(b"ustar\0");
            tar.extend_from_slice(&header);
            if !data.is_empty() {
                tar.extend_from_slice(data);
                let pad = (512 - (data.len() % 512)) % 512;
                tar.extend(std::iter::repeat_n(0u8, pad));
            }
        }
        tar.extend(std::iter::repeat_n(0u8, 1024));
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn resolution_precedence_and_in_use_removal_protection() {
        let root = temp_dir("precedence");
        let storage = root.join("tools");
        let manual_bin = root.join("custom-lune");
        let hint_dir = root.join("hints");
        fs::create_dir_all(&hint_dir).unwrap();
        let hinted_bin = hint_dir.join("lune");
        fs::write(&manual_bin, "#!/bin/sh\necho manual\n").unwrap();
        fs::write(&hinted_bin, "#!/bin/sh\necho hinted\n").unwrap();

        let mut mgr = ToolManager::new(storage, ToolTrustPolicy::default()).unwrap();
        mgr.register_declaration(
            "org.example.lua",
            ToolDeclaration {
                id: "lune".into(),
                name: "Lune".into(),
                description: "Luau runtime".into(),
                category: ToolCategory::Runtime,
                optional: true,
                executables: vec!["lune".into()],
                discovery_hints: vec![hint_dir.to_string_lossy().into_owned()],
                platforms: vec![],
                architectures: vec![],
                version_requirement: None,
                releases: vec![],
            },
        )
        .unwrap();

        let detected = mgr.resolve("lune", None, Some("/ws")).unwrap();
        assert_eq!(detected.scope, ToolSelectionScope::Detected);
        assert_eq!(detected.executable_path, hinted_bin);

        mgr.select_manual("lune", manual_bin.clone(), None).unwrap();
        let user_sel = mgr.resolve("lune", None, Some("/ws")).unwrap();
        assert_eq!(user_sel.scope, ToolSelectionScope::User);
        assert_eq!(user_sel.executable_path, manual_bin);

        mgr.select_system("lune", hinted_bin.clone(), Some("/ws"))
            .unwrap();
        let ws_sel = mgr.resolve("lune", None, Some("/ws")).unwrap();
        assert_eq!(ws_sel.scope, ToolSelectionScope::Workspace);
        assert_eq!(ws_sel.executable_path, hinted_bin);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn managed_install_verifies_sha256_rejects_traversal_and_tracks_removal() {
        let root = temp_dir("install");
        let storage = root.join("tools");
        let archive_path = root.join("tool.tar.gz");
        let tar_bytes = build_tar_gz(&[("bin/mytool", b"#!/bin/sh\necho ok\n", b'0')]);
        fs::write(&archive_path, &tar_bytes).unwrap();
        let good_sha = sha256_file(&archive_path).unwrap();

        let mut policy = ToolTrustPolicy::default();
        policy.allow_local_test_urls = true;
        let mut mgr = ToolManager::new(storage, policy).unwrap();

        let bad_release = ToolRelease {
            version: "1.0.0".into(),
            platform: ToolPlatform::current(),
            arch: ToolArch::current(),
            kind: AcquisitionKind::StaticRelease,
            publisher: "Koren Test".into(),
            url: format!("file://{}", archive_path.display()),
            sha256: "0".repeat(64),
            size_bytes: Some(tar_bytes.len() as u64),
            license: Some("MIT".into()),
            executable_rel_path: "bin/mytool".into(),
        };
        assert!(
            mgr.install_managed_release("mytool", &bad_release, None)
                .is_err()
        );
        assert!(matches!(
            mgr.install_state("mytool"),
            ToolInstallState::Failed { .. }
        ));

        let good_release = ToolRelease {
            sha256: good_sha,
            ..bad_release
        };
        let installed = mgr
            .install_managed_release("mytool", &good_release, None)
            .unwrap();
        assert!(installed.executable_path.is_file());
        mgr.select_managed("mytool", "1.0.0", None).unwrap();
        assert!(
            mgr.remove_managed_version("mytool", "1.0.0", false)
                .is_err()
        );
        mgr.remove_managed_version("mytool", "1.0.0", true).unwrap();
        assert!(mgr.managed_versions("mytool").is_empty());

        let evil_archive = root.join("evil.tar.gz");
        fs::write(&evil_archive, build_tar_gz(&[("../escape", b"bad", b'0')])).unwrap();
        let evil_sha = sha256_file(&evil_archive).unwrap();
        let evil_release = ToolRelease {
            version: "1.0.1".into(),
            url: format!("file://{}", evil_archive.display()),
            sha256: evil_sha,
            executable_rel_path: "bin/mytool".into(),
            ..good_release
        };
        assert!(
            mgr.install_managed_release("mytool", &evil_release, None)
                .is_err()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cancellation_tamper_detection_and_shared_owner_cleanup() {
        let root = temp_dir("recovery");
        let storage = root.join("tools");
        let archive_path = root.join("tool.tar.gz");
        let tar_bytes = build_tar_gz(&[("bin/mytool", b"#!/bin/sh\necho ok\n", b'0')]);
        fs::write(&archive_path, &tar_bytes).unwrap();
        let sha = sha256_file(&archive_path).unwrap();

        let policy = ToolTrustPolicy {
            allow_local_test_urls: true,
            ..ToolTrustPolicy::default()
        };
        let mut mgr = ToolManager::new(storage, policy).unwrap();
        let decl = ToolDeclaration {
            id: "mytool".into(),
            name: "My Tool".into(),
            description: "Shared tool".into(),
            category: ToolCategory::Runtime,
            optional: true,
            executables: vec!["mytool".into()],
            discovery_hints: vec![],
            platforms: vec![],
            architectures: vec![],
            version_requirement: None,
            releases: vec![],
        };
        mgr.register_declaration("org.ext.a", decl.clone()).unwrap();
        mgr.register_declaration("org.ext.b", decl).unwrap();
        mgr.unregister_owner("org.ext.a");
        assert_eq!(mgr.declarations().len(), 1);
        assert_eq!(mgr.declarations()[0].1, vec!["org.ext.b"]);

        let release = ToolRelease {
            version: "2.0.0".into(),
            platform: ToolPlatform::current(),
            arch: ToolArch::current(),
            kind: AcquisitionKind::StaticRelease,
            publisher: "Koren Test".into(),
            url: format!("file://{}", archive_path.display()),
            sha256: sha,
            size_bytes: Some(tar_bytes.len() as u64),
            license: Some("MIT".into()),
            executable_rel_path: "bin/mytool".into(),
        };

        let cancel = Arc::new(AtomicBool::new(true));
        assert!(
            mgr.install_managed_release("mytool", &release, Some(&cancel))
                .is_err()
        );
        assert_eq!(mgr.install_state("mytool"), ToolInstallState::Cancelled);

        let installed = mgr
            .install_managed_release("mytool", &release, None)
            .unwrap();
        mgr.select_managed("mytool", "2.0.0", None).unwrap();
        assert!(mgr.resolve("mytool", None, None).is_ok());

        // Tampering with managed executable must be detected without silent fallback.
        fs::write(&installed.executable_path, b"tampered").unwrap();
        assert!(mgr.resolve("mytool", None, None).is_err());

        mgr.unregister_owner("org.ext.b");
        assert!(mgr.declarations().is_empty());

        let _ = fs::remove_dir_all(root);
    }
}
