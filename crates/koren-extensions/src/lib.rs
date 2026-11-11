//! Versioned, frontend-independent extension manifests, lifecycle, and contributions.
//!
//! Discovery only examines immediate child directories under an explicit installation root.
//! It never searches an open workspace and never executes extension code. Hosts choose when to
//! enable and activate an extension. Activation contributions are staged and committed as one
//! transaction so an activation error cannot leak commands or UI registrations.

use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

/// Current stable extension API version, independent of application releases.
pub const EXTENSION_API_VERSION: &str = "1.0.0";
/// Filename read from an explicitly configured extension installation directory.
pub const MANIFEST_FILE: &str = "koren-extension.toml";

/// Capabilities an extension must declare before registering corresponding contributions.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Register commands in the shared command surface.
    Commands,
    /// Read workspace root metadata through host-provided context.
    WorkspaceRead,
    /// Read the active document through host-provided context.
    DocumentRead,
    /// Request document edits through the host's revision-checked document service.
    DocumentEdit,
    /// Subscribe to selected platform events.
    Events,
    /// Contribute typed menu or editor actions.
    UiContributions,
    /// Contribute language identifiers and filename associations.
    LanguageDefinitions,
    /// Declare an external language-server process configuration (host approval is separate).
    LanguageServer,
    /// Publish language-neutral diagnostics.
    Diagnostics,
}

/// Conditions a host may use to decide when to activate an enabled extension.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationConditions {
    /// Activate after the host is ready.
    #[serde(default)]
    pub on_startup: bool,
    /// Activate after a workspace is opened.
    #[serde(default)]
    pub on_workspace_open: bool,
    /// Activate when one of these language identifiers is in use.
    #[serde(default)]
    pub languages: Vec<String>,
    /// Activate when a file with one of these extensions is opened.
    #[serde(default)]
    pub file_extensions: Vec<String>,
}

/// Language metadata contributed by an extension.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageContribution {
    /// Stable language identifier.
    pub id: String,
    /// User-facing language name.
    pub name: String,
    /// Filename suffixes without a leading dot.
    #[serde(default)]
    pub extensions: Vec<String>,
}

/// Declarative external LSP process configuration; starting it still requires host approval.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageServerContribution {
    /// Stable configuration identifier within the extension.
    pub id: String,
    /// Executable name or absolute path. Never shell parsed.
    pub command: String,
    /// Direct process arguments.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// Language identifiers handled by this server.
    pub languages: Vec<String>,
    /// Filename suffixes handled by this server.
    #[serde(default)]
    pub file_extensions: Vec<String>,
    /// Explicit process environment overrides; approval screens reveal names only.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

/// Versioned extension metadata loaded from `koren-extension.toml`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionManifest {
    /// Manifest schema version. Currently only `1` is accepted.
    pub schema_version: u32,
    /// Reverse-DNS extension identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Publisher or author.
    pub publisher: String,
    /// Extension release version.
    pub version: String,
    /// Human-readable summary.
    pub description: String,
    /// Lua or other host-supported entry point, relative to the extension directory.
    pub entry_point: PathBuf,
    /// Extension API version this extension targets.
    pub api_version: String,
    /// Declared capabilities. Undeclared capabilities cannot be contributed.
    #[serde(default)]
    pub capabilities: BTreeSet<Capability>,
    /// Optional activation triggers.
    #[serde(default)]
    pub activation: ActivationConditions,
    /// Optional language metadata.
    #[serde(default)]
    pub languages: Vec<LanguageContribution>,
    /// Optional LSP process declarations. The host must separately approve each configuration.
    #[serde(default)]
    pub language_servers: Vec<LanguageServerContribution>,
}

impl ExtensionManifest {
    /// Parses and validates a manifest against the host's extension API version.
    ///
    /// # Errors
    /// Returns an error when TOML is malformed or the manifest fails validation.
    pub fn parse(source: &str) -> Result<Self, ExtensionError> {
        let manifest: Self = toml::from_str(source)
            .map_err(|error| ExtensionError::InvalidManifest(error.to_string()))?;
        let host_api = Version::parse(EXTENSION_API_VERSION)
            .map_err(|error| ExtensionError::InvalidManifest(error.to_string()))?;
        manifest.validate(&host_api)?;
        Ok(manifest)
    }

    /// Validates metadata and API compatibility without reading its entry point.
    ///
    /// # Errors
    /// Returns an error when metadata is invalid or the API version is incompatible.
    pub fn validate(&self, host_api: &Version) -> Result<(), ExtensionError> {
        if self.schema_version != 1 {
            return Err(ExtensionError::InvalidManifest(format!(
                "unsupported manifest schema version {}",
                self.schema_version
            )));
        }
        validate_identifier(&self.id).map_err(ExtensionError::InvalidManifest)?;
        for (field, value) in [
            ("name", self.name.as_str()),
            ("publisher", self.publisher.as_str()),
            ("description", self.description.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(ExtensionError::InvalidManifest(format!(
                    "{field} must not be empty"
                )));
            }
        }
        Version::parse(&self.version).map_err(|error| {
            ExtensionError::InvalidManifest(format!("invalid extension version: {error}"))
        })?;
        let api = Version::parse(&self.api_version).map_err(|error| {
            ExtensionError::InvalidManifest(format!("invalid extension API version: {error}"))
        })?;
        if api.major != host_api.major || api > *host_api {
            return Err(ExtensionError::IncompatibleApi {
                extension: api.to_string(),
                host: host_api.to_string(),
            });
        }
        if !safe_relative_path(&self.entry_point) {
            return Err(ExtensionError::InvalidManifest(
                "entry_point must be a relative path without parent or root components".into(),
            ));
        }
        if !self.languages.is_empty()
            && !self.capabilities.contains(&Capability::LanguageDefinitions)
        {
            return Err(ExtensionError::MissingCapability(
                Capability::LanguageDefinitions,
            ));
        }
        if !self.language_servers.is_empty()
            && !self.capabilities.contains(&Capability::LanguageServer)
        {
            return Err(ExtensionError::MissingCapability(
                Capability::LanguageServer,
            ));
        }
        let mut ids = BTreeSet::new();
        for language in &self.languages {
            validate_language_id(&language.id).map_err(ExtensionError::InvalidManifest)?;
            if language.name.trim().is_empty() || !ids.insert(language.id.as_str()) {
                return Err(ExtensionError::InvalidManifest(
                    "language names must be non-empty and identifiers unique".into(),
                ));
            }
            validate_extensions(&language.extensions)?;
        }
        ids.clear();
        for server in &self.language_servers {
            validate_language_id(&server.id).map_err(|_| {
                ExtensionError::InvalidManifest(format!(
                    "'{}' is not a valid language-server identifier",
                    server.id
                ))
            })?;
            if server.command.trim().is_empty()
                || server.languages.is_empty()
                || !ids.insert(server.id.as_str())
            {
                return Err(ExtensionError::InvalidManifest(
                    "language servers need a unique id, command, and language".into(),
                ));
            }
            validate_extensions(&server.file_extensions)?;
            if server
                .environment
                .keys()
                .any(|key| key.is_empty() || key.contains('=') || key.contains('\0'))
                || server.command.contains('\0')
                || server
                    .arguments
                    .iter()
                    .any(|argument| argument.contains('\0'))
            {
                return Err(ExtensionError::InvalidManifest(
                    "language-server commands and environment names contain invalid characters"
                        .into(),
                ));
            }
        }
        Ok(())
    }
}

fn validate_extensions(extensions: &[String]) -> Result<(), ExtensionError> {
    if extensions.iter().any(|suffix| {
        suffix.is_empty()
            || suffix.starts_with('.')
            || suffix.contains('/')
            || suffix.contains('\\')
            || suffix.trim() != suffix
    }) {
        return Err(ExtensionError::InvalidManifest(
            "file extensions must be non-empty suffixes without dots or path separators".into(),
        ));
    }
    Ok(())
}

fn validate_identifier(value: &str) -> Result<(), String> {
    let segments: Vec<_> = value.split('.').collect();
    if segments.len() < 2
        || segments.iter().any(|segment| {
            segment.is_empty()
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                || segment.starts_with('-')
                || segment.ends_with('-')
        })
    {
        return Err(format!(
            "'{value}' is not a valid lower-case reverse-DNS identifier"
        ));
    }
    Ok(())
}

fn validate_language_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || !value.as_bytes()[0].is_ascii_lowercase()
        || value.ends_with('-')
    {
        return Err(format!(
            "'{value}' is not a valid lower-case language identifier"
        ));
    }
    Ok(())
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Extension lifecycle state exposed to hosts and management interfaces.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionState {
    /// Found and compatible, but not enabled.
    Disabled,
    /// Enabled and ready for host-selected activation.
    Installed,
    /// Activation callback is running.
    Activating,
    /// Active and owns the listed contributions.
    Active,
    /// Activation failed; no contributions were retained.
    Failed,
}

/// A UI-safe command invocation context; it contains no process or filesystem handles.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandContext {
    /// Current workspace root when one is open.
    pub workspace_root: Option<PathBuf>,
    /// Active document identifier when one is open.
    pub active_document_id: Option<u64>,
    /// Active workspace-relative document path when one is open.
    pub active_document_path: Option<PathBuf>,
    /// Active document revision at invocation time.
    pub active_document_revision: Option<u64>,
    /// Active document text snapshot at invocation time.
    pub active_document_contents: Option<String>,
}

/// Extension command callback interface.
pub type CommandCallback =
    Arc<dyn Fn(&CommandContext, &[String]) -> Result<String, String> + Send + Sync + 'static>;

/// Typed menu location for a contributed command.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuLocation {
    /// Application Tools menu.
    Tools,
    /// Application Navigate menu.
    Navigate,
    /// Document editor context menu.
    EditorContext,
}

/// A menu item invoking one extension-owned command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MenuContribution {
    /// Extension-qualified command identifier.
    pub command_id: String,
    /// User-facing menu label.
    pub label: String,
    /// Menu placement.
    pub location: MenuLocation,
}

/// Typed diagnostic provider declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticProviderContribution {
    /// Extension-qualified provider identifier.
    pub id: String,
    /// User-facing name.
    pub name: String,
}

/// All registrations owned by one extension activation.
#[derive(Clone, Default)]
pub struct ExtensionContributions {
    /// Registered commands.
    pub commands: BTreeMap<String, RegisteredCommand>,
    /// Typed menu items.
    pub menus: Vec<MenuContribution>,
    /// Event names requested for subscription.
    pub events: BTreeSet<String>,
    /// Language definitions from the manifest.
    pub languages: Vec<LanguageContribution>,
    /// LSP process declarations from the manifest.
    pub language_servers: Vec<LanguageServerContribution>,
    /// Diagnostic provider declarations.
    pub diagnostic_providers: Vec<DiagnosticProviderContribution>,
}

/// A command and its callback registered by an extension.
#[derive(Clone)]
pub struct RegisteredCommand {
    /// Localized display label.
    pub title: String,
    /// Short command description.
    pub description: String,
    /// Extension callback.
    pub callback: CommandCallback,
}

impl std::fmt::Debug for RegisteredCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisteredCommand")
            .field("title", &self.title)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

/// Staging area given to a single activation callback.
pub struct ContributionBuilder<'a> {
    extension_id: &'a str,
    capabilities: &'a BTreeSet<Capability>,
    values: ExtensionContributions,
}

impl<'a> ContributionBuilder<'a> {
    /// Creates an activation staging area constrained by a manifest's declared capabilities.
    #[must_use]
    pub fn new(manifest: &'a ExtensionManifest) -> Self {
        Self {
            extension_id: &manifest.id,
            capabilities: &manifest.capabilities,
            values: ExtensionContributions {
                languages: manifest.languages.clone(),
                language_servers: manifest.language_servers.clone(),
                ..ExtensionContributions::default()
            },
        }
    }

    /// Registers an extension-owned command using a namespaced identifier.
    ///
    /// # Errors
    /// Returns an error when the capability is undeclared or the contribution is invalid.
    pub fn command(
        &mut self,
        id: &str,
        title: impl Into<String>,
        description: impl Into<String>,
        callback: CommandCallback,
    ) -> Result<String, ExtensionError> {
        self.require(Capability::Commands)?;
        validate_local_id(id)?;
        let title = title.into();
        let description = description.into();
        if title.trim().is_empty() || description.trim().is_empty() {
            return Err(ExtensionError::InvalidContribution(
                "command title and description must not be empty".into(),
            ));
        }
        let full_id = format!("{}.{}", self.extension_id, id);
        if self
            .values
            .commands
            .insert(
                full_id.clone(),
                RegisteredCommand {
                    title,
                    description,
                    callback,
                },
            )
            .is_some()
        {
            return Err(ExtensionError::DuplicateContribution(full_id));
        }
        Ok(full_id)
    }

    /// Adds a menu action that must reference a command registered in this activation.
    ///
    /// # Errors
    /// Returns an error when the capability is undeclared or the command/label is invalid.
    pub fn menu(
        &mut self,
        command_id: &str,
        label: impl Into<String>,
        location: MenuLocation,
    ) -> Result<(), ExtensionError> {
        self.require(Capability::UiContributions)?;
        let full_id = if command_id.starts_with(&format!("{}.", self.extension_id)) {
            command_id.to_owned()
        } else {
            format!("{}.{}", self.extension_id, command_id)
        };
        if !self.values.commands.contains_key(&full_id) {
            return Err(ExtensionError::InvalidContribution(format!(
                "menu item references unregistered command '{full_id}'"
            )));
        }
        let label = label.into();
        if label.trim().is_empty() {
            return Err(ExtensionError::InvalidContribution(
                "menu label must not be empty".into(),
            ));
        }
        self.values.menus.push(MenuContribution {
            command_id: full_id,
            label,
            location,
        });
        Ok(())
    }

    /// Subscribes to a selected event name understood by the host.
    ///
    /// # Errors
    /// Returns an error when the capability is undeclared or the event is unsupported.
    pub fn subscribe(&mut self, event: &str) -> Result<(), ExtensionError> {
        self.require(Capability::Events)?;
        if !matches!(
            event,
            "workspace_opened"
                | "document_opened"
                | "document_changed"
                | "document_saved"
                | "document_closed"
        ) {
            return Err(ExtensionError::InvalidContribution(format!(
                "unsupported event '{event}'"
            )));
        }
        self.values.events.insert(event.into());
        Ok(())
    }

    /// Registers a language-neutral diagnostic provider.
    ///
    /// # Errors
    /// Returns an error when the capability is undeclared or the provider is invalid.
    pub fn diagnostic_provider(
        &mut self,
        id: &str,
        name: impl Into<String>,
    ) -> Result<(), ExtensionError> {
        self.require(Capability::Diagnostics)?;
        validate_local_id(id)?;
        let name = name.into();
        if name.trim().is_empty() {
            return Err(ExtensionError::InvalidContribution(
                "diagnostic provider name must not be empty".into(),
            ));
        }
        self.values
            .diagnostic_providers
            .push(DiagnosticProviderContribution {
                id: format!("{}.{}", self.extension_id, id),
                name,
            });
        Ok(())
    }

    fn require(&self, capability: Capability) -> Result<(), ExtensionError> {
        if self.capabilities.contains(&capability) {
            Ok(())
        } else {
            Err(ExtensionError::MissingCapability(capability))
        }
    }

    /// Returns the staged, typed contributions without activating them in a manager.
    #[must_use]
    pub fn into_contributions(self) -> ExtensionContributions {
        self.values
    }
}

fn validate_local_id(id: &str) -> Result<(), ExtensionError> {
    if id.is_empty()
        || id.split('.').any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
    {
        return Err(ExtensionError::InvalidContribution(format!(
            "'{id}' is not a valid lower-case command identifier"
        )));
    }
    Ok(())
}

/// Public metadata returned by extension listing.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExtensionSummary {
    /// Reverse-DNS identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Publisher.
    pub publisher: String,
    /// Release version.
    pub version: String,
    /// Description.
    pub description: String,
    /// Whether explicitly enabled.
    pub enabled: bool,
    /// Lifecycle state.
    pub state: ExtensionState,
    /// Last activation error, if any.
    pub error: Option<String>,
}

struct ExtensionRecord {
    manifest: ExtensionManifest,
    path: PathBuf,
    enabled: bool,
    state: ExtensionState,
    error: Option<String>,
    contributions: ExtensionContributions,
}

/// Manager-local extension registry. It does not load code or communicate with the desktop.
#[derive(Default)]
pub struct ExtensionManager {
    extensions: BTreeMap<String, ExtensionRecord>,
}

impl std::fmt::Debug for ExtensionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionManager")
            .field("extensions", &self.list())
            .finish()
    }
}

impl ExtensionManager {
    /// Creates an empty manager.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Discovers direct extension children under a configured installation directory.
    ///
    /// Workspace paths must never be passed implicitly; callers own the trusted root.
    ///
    /// # Errors
    /// Returns an error when discovery, manifest validation, entry-point validation, or
    /// duplicate detection fails. No extensions are registered on failure.
    pub fn discover_installed(
        &mut self,
        root: &Path,
    ) -> Result<Vec<ExtensionSummary>, ExtensionError> {
        let root = fs::canonicalize(root).map_err(|error| ExtensionError::Io {
            path: root.to_path_buf(),
            message: error.to_string(),
        })?;
        let entries = fs::read_dir(&root).map_err(|error| ExtensionError::Io {
            path: root.clone(),
            message: error.to_string(),
        })?;
        let mut paths = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .file_type()
                    .ok()
                    .filter(std::fs::FileType::is_dir)
                    .map(|_| entry.path())
            })
            .collect::<Vec<_>>();
        paths.sort();
        let mut staged = Vec::new();
        let mut staged_ids = BTreeMap::<String, PathBuf>::new();
        for directory in paths {
            let manifest_path = directory.join(MANIFEST_FILE);
            if !manifest_path.is_file() {
                continue;
            }
            let source =
                fs::read_to_string(&manifest_path).map_err(|error| ExtensionError::Io {
                    path: manifest_path.clone(),
                    message: error.to_string(),
                })?;
            let manifest = ExtensionManifest::parse(&source)?;
            let extension_root =
                fs::canonicalize(&directory).map_err(|error| ExtensionError::Io {
                    path: directory.clone(),
                    message: error.to_string(),
                })?;
            let entry = extension_root.join(&manifest.entry_point);
            let resolved_entry = fs::canonicalize(&entry)
                .map_err(|_| ExtensionError::MissingEntryPoint(entry.clone()))?;
            if !resolved_entry.starts_with(&extension_root) || !resolved_entry.is_file() {
                return Err(ExtensionError::MissingEntryPoint(entry));
            }
            if let Some(existing) = self.extensions.get(&manifest.id) {
                return Err(ExtensionError::DuplicateExtension {
                    id: manifest.id.clone(),
                    first: existing.path.clone(),
                    second: directory,
                });
            }
            if let Some(first) = staged_ids.get(&manifest.id) {
                return Err(ExtensionError::DuplicateExtension {
                    id: manifest.id.clone(),
                    first: first.clone(),
                    second: directory,
                });
            }
            staged_ids.insert(manifest.id.clone(), directory);
            staged.push((manifest, extension_root));
        }
        for (manifest, path) in staged {
            let id = manifest.id.clone();
            self.extensions.insert(
                id,
                ExtensionRecord {
                    manifest,
                    path,
                    enabled: false,
                    state: ExtensionState::Disabled,
                    error: None,
                    contributions: ExtensionContributions::default(),
                },
            );
        }
        Ok(self.list())
    }

    /// Registers one already validated extension from an explicit trusted installation path.
    ///
    /// # Errors
    /// Returns an error when the manifest, entry point, or extension identifier is invalid.
    pub fn register_installed(
        &mut self,
        manifest: ExtensionManifest,
        path: PathBuf,
    ) -> Result<(), ExtensionError> {
        let host_api = Version::parse(EXTENSION_API_VERSION)
            .map_err(|error| ExtensionError::InvalidManifest(error.to_string()))?;
        manifest.validate(&host_api)?;
        if self.extensions.contains_key(&manifest.id) {
            return Err(ExtensionError::DuplicateId(manifest.id));
        }
        let entry = path.join(&manifest.entry_point);
        if !entry.is_file() {
            return Err(ExtensionError::MissingEntryPoint(entry));
        }
        self.extensions.insert(
            manifest.id.clone(),
            ExtensionRecord {
                manifest,
                path,
                enabled: false,
                state: ExtensionState::Disabled,
                error: None,
                contributions: ExtensionContributions::default(),
            },
        );
        Ok(())
    }

    /// Returns extension summaries in deterministic identifier order.
    #[must_use]
    pub fn list(&self) -> Vec<ExtensionSummary> {
        self.extensions
            .values()
            .map(|record| ExtensionSummary {
                id: record.manifest.id.clone(),
                name: record.manifest.name.clone(),
                publisher: record.manifest.publisher.clone(),
                version: record.manifest.version.clone(),
                description: record.manifest.description.clone(),
                enabled: record.enabled,
                state: record.state.clone(),
                error: record.error.clone(),
            })
            .collect()
    }

    /// Returns a manifest and installation path without activating extension code.
    ///
    /// # Errors
    /// Returns an error when no extension has the requested identifier.
    pub fn manifest(&self, id: &str) -> Result<(&ExtensionManifest, &Path), ExtensionError> {
        self.extensions
            .get(id)
            .map(|record| (&record.manifest, record.path.as_path()))
            .ok_or_else(|| ExtensionError::NotFound(id.into()))
    }

    /// Enables an extension for explicit or event-driven activation.
    ///
    /// # Errors
    /// Returns an error when no extension has the requested identifier.
    pub fn enable(&mut self, id: &str) -> Result<(), ExtensionError> {
        let record = self.record_mut(id)?;
        if record.state == ExtensionState::Active {
            return Ok(());
        }
        record.enabled = true;
        record.state = ExtensionState::Installed;
        record.error = None;
        Ok(())
    }

    /// Runs an activation callback and atomically takes ownership of its contributions.
    ///
    /// # Errors
    /// Returns an error when the extension is disabled, missing, or activation fails.
    pub fn activate<F>(&mut self, id: &str, activate: F) -> Result<(), ExtensionError>
    where
        F: FnOnce(&ExtensionManifest, &mut ContributionBuilder<'_>) -> Result<(), String>,
    {
        let record = self.record_mut(id)?;
        if !record.enabled {
            return Err(ExtensionError::Disabled(id.into()));
        }
        if record.state == ExtensionState::Active {
            return Ok(());
        }
        record.state = ExtensionState::Activating;
        let manifest = record.manifest.clone();
        let mut staging = ContributionBuilder::new(&manifest);
        let outcome = catch_unwind(AssertUnwindSafe(|| activate(&manifest, &mut staging)));
        let record = self.record_mut(id)?;
        match outcome {
            Ok(Ok(())) => {
                record.contributions = staging.values;
                record.state = ExtensionState::Active;
                record.error = None;
                Ok(())
            }
            Ok(Err(error)) => {
                record.contributions = ExtensionContributions::default();
                record.state = ExtensionState::Failed;
                record.error = Some(error.clone());
                Err(ExtensionError::ActivationFailed {
                    id: id.into(),
                    message: error,
                })
            }
            Err(_) => {
                let error = "activation panicked".to_owned();
                record.contributions = ExtensionContributions::default();
                record.state = ExtensionState::Failed;
                record.error = Some(error.clone());
                Err(ExtensionError::ActivationFailed {
                    id: id.into(),
                    message: error,
                })
            }
        }
    }

    /// Removes all owned contributions and leaves the extension enabled but inactive.
    ///
    /// # Errors
    /// Returns an error when no extension has the requested identifier.
    pub fn deactivate(&mut self, id: &str) -> Result<(), ExtensionError> {
        let record = self.record_mut(id)?;
        record.contributions = ExtensionContributions::default();
        record.state = if record.enabled {
            ExtensionState::Installed
        } else {
            ExtensionState::Disabled
        };
        record.error = None;
        Ok(())
    }

    /// Disables an extension and removes all of its registrations.
    ///
    /// # Errors
    /// Returns an error when no extension has the requested identifier.
    pub fn disable(&mut self, id: &str) -> Result<(), ExtensionError> {
        let record = self.record_mut(id)?;
        record.contributions = ExtensionContributions::default();
        record.enabled = false;
        record.state = ExtensionState::Disabled;
        record.error = None;
        Ok(())
    }

    /// Returns the contributions owned by an active extension.
    ///
    /// # Errors
    /// Returns an error when the extension is missing or not active.
    pub fn contributions(&self, id: &str) -> Result<&ExtensionContributions, ExtensionError> {
        let record = self
            .extensions
            .get(id)
            .ok_or_else(|| ExtensionError::NotFound(id.into()))?;
        if record.state != ExtensionState::Active {
            return Err(ExtensionError::NotActive(id.into()));
        }
        Ok(&record.contributions)
    }

    /// Invokes an active extension command with the host's current safe context.
    ///
    /// # Errors
    /// Returns an error when the command is missing or its callback fails.
    pub fn invoke_command(
        &self,
        id: &str,
        context: &CommandContext,
        args: &[String],
    ) -> Result<String, ExtensionError> {
        for record in self.extensions.values() {
            if record.state == ExtensionState::Active {
                if let Some(command) = record.contributions.commands.get(id) {
                    return (command.callback)(context, args).map_err(|message| {
                        ExtensionError::CommandFailed {
                            id: id.into(),
                            message,
                        }
                    });
                }
            }
        }
        Err(ExtensionError::NotFound(id.into()))
    }

    fn record_mut(&mut self, id: &str) -> Result<&mut ExtensionRecord, ExtensionError> {
        self.extensions
            .get_mut(id)
            .ok_or_else(|| ExtensionError::NotFound(id.into()))
    }
}

/// Errors from manifest validation, discovery, lifecycle, and contribution registration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExtensionError {
    /// Manifest syntax or content is invalid.
    InvalidManifest(String),
    /// Extension targets an unsupported API version.
    IncompatibleApi {
        /// API version requested by the extension.
        extension: String,
        /// API version supplied by the host.
        host: String,
    },
    /// Extension attempts an undeclared capability.
    MissingCapability(Capability),
    /// An extension entry point was not present.
    MissingEntryPoint(PathBuf),
    /// Identifier already exists.
    DuplicateId(String),
    /// Two discovered installations declare the same identifier.
    DuplicateExtension {
        /// Duplicate extension identifier.
        id: String,
        /// First installation directory.
        first: PathBuf,
        /// Second installation directory.
        second: PathBuf,
    },
    /// No extension has the requested identifier.
    NotFound(String),
    /// Operation requires an enabled extension.
    Disabled(String),
    /// Operation requires a successfully activated extension.
    NotActive(String),
    /// Activation callback failed; its staged contributions were discarded.
    ActivationFailed {
        /// Extension that failed activation.
        id: String,
        /// Activation error summary.
        message: String,
    },
    /// An extension contribution is invalid or refers to a missing command.
    InvalidContribution(String),
    /// Contribution identifier was already registered in the activation.
    DuplicateContribution(String),
    /// A command callback failed.
    CommandFailed {
        /// Extension-owned command identifier.
        id: String,
        /// Callback error summary.
        message: String,
    },
    /// Filesystem discovery failed.
    Io {
        /// Filesystem path involved in discovery.
        path: PathBuf,
        /// Operating-system error summary.
        message: String,
    },
}

impl std::fmt::Display for ExtensionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidManifest(message) => write!(f, "invalid extension manifest: {message}"),
            Self::IncompatibleApi { extension, host } => write!(
                f,
                "extension API {extension} is incompatible with host API {host}"
            ),
            Self::MissingCapability(capability) => {
                write!(f, "extension did not declare capability {capability:?}")
            }
            Self::MissingEntryPoint(path) => write!(
                f,
                "extension entry point '{}' does not exist",
                path.display()
            ),
            Self::DuplicateId(id) => write!(f, "extension '{id}' is already registered"),
            Self::DuplicateExtension { id, first, second } => write!(
                f,
                "extension '{id}' is installed in both '{}' and '{}'",
                first.display(),
                second.display()
            ),
            Self::NotFound(id) => write!(f, "extension or command '{id}' was not found"),
            Self::Disabled(id) => write!(f, "extension '{id}' is disabled"),
            Self::NotActive(id) => write!(f, "extension '{id}' is not active"),
            Self::ActivationFailed { id, message } => {
                write!(f, "extension '{id}' activation failed: {message}")
            }
            Self::InvalidContribution(message) => {
                write!(f, "invalid extension contribution: {message}")
            }
            Self::DuplicateContribution(id) => {
                write!(f, "extension contribution '{id}' is duplicated")
            }
            Self::CommandFailed { id, message } => {
                write!(f, "extension command '{id}' failed: {message}")
            }
            Self::Io { path, message } => write!(
                f,
                "extension directory '{}' failed: {message}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ExtensionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    fn manifest(capabilities: &[Capability]) -> ExtensionManifest {
        ExtensionManifest {
            schema_version: 1,
            id: "org.example.hello".into(),
            name: "Hello".into(),
            publisher: "Koren contributors".into(),
            version: "0.1.0".into(),
            description: "Example extension".into(),
            entry_point: "main.lua".into(),
            api_version: EXTENSION_API_VERSION.into(),
            capabilities: capabilities.iter().copied().collect(),
            activation: ActivationConditions::default(),
            languages: Vec::new(),
            language_servers: Vec::new(),
        }
    }

    fn temp_dir() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("koren-extension-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn manifest_validation_checks_schema_id_paths_and_api() {
        let good = "schema_version=1\nid='org.example.hello'\nname='Hello'\npublisher='Koren'\nversion='0.1.0'\ndescription='Example'\nentry_point='main.lua'\napi_version='1.0.0'\ncapabilities=['commands']\n";
        assert!(ExtensionManifest::parse(good).is_ok());
        assert!(ExtensionManifest::parse(&good.replace("org.example.hello", "Hello")).is_err());
        assert!(ExtensionManifest::parse(&good.replace("main.lua", "../main.lua")).is_err());
        assert!(ExtensionManifest::parse(&good.replace("1.0.0", "2.0.0")).is_err());
        assert!(
            ExtensionManifest::parse(&good.replace("schema_version=1", "schema_version=2"))
                .is_err()
        );
    }

    #[test]
    fn activation_is_transactional_and_disable_cleans_up_commands_and_events() {
        let mut manager = ExtensionManager::new();
        let dir = temp_dir();
        fs::write(dir.join("main.lua"), "return {}\n").unwrap();
        manager
            .register_installed(
                manifest(&[Capability::Commands, Capability::Events]),
                dir.clone(),
            )
            .unwrap();
        manager.enable("org.example.hello").unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let called = count.clone();
        manager
            .activate("org.example.hello", move |_manifest, builder| {
                builder
                    .command(
                        "hello",
                        "Hello",
                        "Return greeting",
                        Arc::new(move |_, _| {
                            called.fetch_add(1, Ordering::SeqCst);
                            Ok("hello".into())
                        }),
                    )
                    .map_err(|error| error.to_string())?;
                builder
                    .subscribe("document_changed")
                    .map_err(|error| error.to_string())?;
                Ok(())
            })
            .unwrap();
        let context = CommandContext::default();
        assert_eq!(
            manager
                .invoke_command("org.example.hello.hello", &context, &[])
                .unwrap(),
            "hello"
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        manager.disable("org.example.hello").unwrap();
        assert!(
            manager
                .invoke_command("org.example.hello.hello", &context, &[])
                .is_err()
        );

        manager.enable("org.example.hello").unwrap();
        assert!(
            manager
                .activate("org.example.hello", |_manifest, builder| {
                    builder
                        .command(
                            "lost",
                            "Lost",
                            "Not committed",
                            Arc::new(|_, _| Ok(String::new())),
                        )
                        .map_err(|error| error.to_string())?;
                    Err("activation rejected".into())
                })
                .is_err()
        );
        assert_eq!(manager.list()[0].state, ExtensionState::Failed);
        assert!(manager.contributions("org.example.hello").is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn capability_and_menu_validation_reject_unowned_features() {
        let mut manager = ExtensionManager::new();
        let dir = temp_dir();
        fs::write(dir.join("main.lua"), "return {}\n").unwrap();
        manager
            .register_installed(manifest(&[]), dir.clone())
            .unwrap();
        manager.enable("org.example.hello").unwrap();
        assert!(
            manager
                .activate("org.example.hello", |_manifest, builder| {
                    builder
                        .command(
                            "hello",
                            "Hello",
                            "No capability",
                            Arc::new(|_, _| Ok(String::new())),
                        )
                        .map_err(|error| error.to_string())?;
                    Ok(())
                })
                .is_err()
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn discovery_is_explicit_and_lists_metadata_without_activation() {
        let root = temp_dir();
        let dir = root.join("hello");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(MANIFEST_FILE), "schema_version=1\nid='org.example.hello'\nname='Hello'\npublisher='Koren'\nversion='0.1.0'\ndescription='Example'\nentry_point='main.lua'\napi_version='1.0.0'\ncapabilities=[]\n").unwrap();
        fs::write(
            dir.join("main.lua"),
            "error('must not execute during discovery')\n",
        )
        .unwrap();
        let mut manager = ExtensionManager::new();
        let listed = manager.discover_installed(&root).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].state, ExtensionState::Disabled);
        assert!(!listed[0].enabled);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn discovery_rejects_an_entry_point_symlink_outside_the_extension_directory() {
        use std::os::unix::fs::symlink;

        let root = temp_dir();
        let directory = root.join("extension");
        fs::create_dir_all(&directory).unwrap();
        let outside = root.join("outside.lua");
        fs::write(&outside, "return {}\n").unwrap();
        symlink(&outside, directory.join("main.lua")).unwrap();
        fs::write(directory.join(MANIFEST_FILE), "schema_version=1\nid='org.example.hello'\nname='Hello'\npublisher='Koren'\nversion='0.1.0'\ndescription='Example'\nentry_point='main.lua'\napi_version='1.0.0'\ncapabilities=[]\n").unwrap();

        let mut manager = ExtensionManager::new();
        assert!(matches!(
            manager.discover_installed(&root),
            Err(ExtensionError::MissingEntryPoint(_))
        ));
        assert_eq!(manager.list().len(), 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_discovery_does_not_leave_partially_registered_extensions() {
        let root = temp_dir();
        for name in ["a-valid", "b-invalid"] {
            let directory = root.join(name);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("main.lua"), "return {}\n").unwrap();
        }
        fs::write(root.join("a-valid").join(MANIFEST_FILE), "schema_version=1\nid='org.example.valid'\nname='Valid'\npublisher='Koren'\nversion='0.1.0'\ndescription='Valid'\nentry_point='main.lua'\napi_version='1.0.0'\ncapabilities=[]\n").unwrap();
        fs::write(
            root.join("b-invalid").join(MANIFEST_FILE),
            "not valid toml [\n",
        )
        .unwrap();
        let mut manager = ExtensionManager::new();
        assert!(manager.discover_installed(&root).is_err());
        assert_eq!(manager.list().len(), 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bundled_example_manifests_parse_and_describe_the_shared_lsp_api() {
        let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions");
        let hello =
            fs::read_to_string(examples.join("org.example.hello/koren-extension.toml")).unwrap();
        assert_eq!(
            ExtensionManifest::parse(&hello).unwrap().id,
            "org.example.hello"
        );
        let rust =
            fs::read_to_string(examples.join("org.koren.rust-analyzer/koren-extension.toml"))
                .unwrap();
        let manifest = ExtensionManifest::parse(&rust).unwrap();
        assert_eq!(manifest.language_servers[0].command, "rust-analyzer");
        assert_eq!(manifest.language_servers[0].languages, ["rust"]);
    }
}
