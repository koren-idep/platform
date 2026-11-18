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
    /// Contribute declarative DAP adapters and profiles; every start still requires host approval.
    DebugAdapters,
    /// Publish language-neutral diagnostics.
    Diagnostics,
    /// Register typed namespaced settings.
    Settings,
    /// Register safe design-token themes.
    Themes,
    /// Declare declarative run configurations; execution still requires host approval.
    RunConfigurations,
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

/// Launch or attach type for an extension-contributed DAP profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebugRequestContribution {
    /// Start a new target using the adapter.
    Launch,
    /// Attach to an existing target; disconnect must leave it running.
    Attach,
}

/// Declarative local DAP adapter. Registration never starts its executable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugAdapterContribution {
    /// Stable identifier within the extension.
    pub id: String,
    /// User-visible adapter name.
    pub name: String,
    /// Executable passed directly to the process API.
    pub command: String,
    /// Direct adapter arguments.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// Adapter working directory; `${workspaceFolder}` resolves when the host opens a profile.
    pub working_directory: String,
    /// Explicit environment overrides; values are shown only to the process and not in approval UI.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

/// Suggested, language-neutral launch or attach profile.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugConfigurationContribution {
    /// Stable identifier within the extension.
    pub id: String,
    /// User-visible profile name.
    pub name: String,
    /// Local adapter identifier from `debug_adapters`.
    pub adapter_id: String,
    /// Whether this profile launches or attaches.
    pub request: DebugRequestContribution,
    /// Target program path for launch or optional executable metadata for attach.
    pub program: Option<String>,
    /// Process ID required for attach profiles.
    pub process_id: Option<u32>,
    /// Program arguments.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// Target working directory; `${workspaceFolder}` resolves when activated for a workspace.
    pub working_directory: String,
    /// Explicit target environment overrides.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Adapter-specific JSON launch settings.
    #[serde(default)]
    pub adapter_settings: serde_json::Map<String, serde_json::Value>,
    /// Stop at the first instruction when supported by the adapter.
    #[serde(default)]
    pub stop_on_entry: bool,
}

/// Declarative run configuration contributed by an extension. Registration never starts a process.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunConfigurationContribution {
    /// Stable identifier within the extension.
    pub id: String,
    /// User-visible configuration name.
    pub name: String,
    /// Executable name or absolute path. Mutually exclusive with `tool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    /// Optional Tool Manager identity. Mutually exclusive with `executable`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Direct argument vector, without shell parsing.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// Working directory with optional placeholders.
    #[serde(default)]
    pub working_directory: String,
    /// Explicit environment overrides; values are never shown in approval summaries.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Applicable language identifiers. Empty means any language.
    #[serde(default)]
    pub languages: Vec<String>,
    /// Output destination: `"run"` or `"terminal"`.
    #[serde(default = "default_run_output")]
    pub output: String,
    /// Unsaved-document policy: `"ask"`, `"save"`, or `"disk"`.
    #[serde(default = "default_run_save")]
    pub save: String,
    /// Optional execution timeout in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Optional debug adapter association.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug_adapter: Option<String>,
    /// Whether users are encouraged to customize arguments when duplicating.
    #[serde(default)]
    pub configurable_arguments: bool,
}

fn default_run_output() -> String {
    "run".into()
}

fn default_run_save() -> String {
    "ask".into()
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
    /// Optional declarative DAP adapters.
    #[serde(default)]
    pub debug_adapters: Vec<DebugAdapterContribution>,
    /// Optional suggested DAP profiles.
    #[serde(default)]
    pub debug_configurations: Vec<DebugConfigurationContribution>,
    /// Optional declarative run configurations.
    #[serde(default)]
    pub run_configurations: Vec<RunConfigurationContribution>,
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
        if (!self.debug_adapters.is_empty() || !self.debug_configurations.is_empty())
            && !self.capabilities.contains(&Capability::DebugAdapters)
        {
            return Err(ExtensionError::MissingCapability(Capability::DebugAdapters));
        }
        if !self.run_configurations.is_empty()
            && !self.capabilities.contains(&Capability::RunConfigurations)
        {
            return Err(ExtensionError::MissingCapability(
                Capability::RunConfigurations,
            ));
        }
        self.validate_contributions()?;
        Ok(())
    }

    fn validate_contributions(&self) -> Result<(), ExtensionError> {
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
        self.validate_debug_contributions()?;
        Ok(())
    }

    fn validate_debug_contributions(&self) -> Result<(), ExtensionError> {
        let mut ids = BTreeSet::new();
        ids.clear();
        for adapter in &self.debug_adapters {
            validate_language_id(&adapter.id).map_err(ExtensionError::InvalidManifest)?;
            if !ids.insert(adapter.id.as_str())
                || adapter.name.trim().is_empty()
                || adapter.command.trim().is_empty()
                || adapter.working_directory.trim().is_empty()
                || adapter.command.contains('\0')
                || adapter
                    .arguments
                    .iter()
                    .any(|argument| argument.contains('\0'))
                || adapter.environment.iter().any(|(key, value)| {
                    key.is_empty()
                        || key.contains('=')
                        || key.contains('\0')
                        || value.contains('\0')
                })
            {
                return Err(ExtensionError::InvalidManifest(
                    "debug adapters need unique ids and valid process settings".into(),
                ));
            }
        }
        ids.clear();
        for configuration in &self.debug_configurations {
            validate_language_id(&configuration.id).map_err(ExtensionError::InvalidManifest)?;
            let adapter_exists = self
                .debug_adapters
                .iter()
                .any(|adapter| adapter.id == configuration.adapter_id);
            let valid_target = match configuration.request {
                DebugRequestContribution::Launch => configuration
                    .program
                    .as_ref()
                    .is_some_and(|program| !program.trim().is_empty()),
                DebugRequestContribution::Attach => {
                    configuration.process_id.is_some_and(|pid| pid > 0)
                }
            };
            if !ids.insert(configuration.id.as_str())
                || configuration.name.trim().is_empty()
                || !adapter_exists
                || !valid_target
                || configuration.working_directory.trim().is_empty()
                || configuration
                    .arguments
                    .iter()
                    .any(|argument| argument.contains('\0'))
                || configuration.environment.iter().any(|(key, value)| {
                    key.is_empty()
                        || key.contains('=')
                        || key.contains('\0')
                        || value.contains('\0')
                })
            {
                return Err(ExtensionError::InvalidManifest(
                    "debug profiles need a unique id, adapter, target, and valid process settings"
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

/// Host-evaluated command availability condition.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandEnablement {
    /// Always enabled.
    #[default]
    Always,
    /// Enabled only while a workspace is open.
    WorkspaceOpen,
    /// Enabled only while an editor document is active.
    ActiveDocument,
}

/// Optional presentation and availability metadata for an extension command.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandOptions {
    /// Optional shortcut label in conventional key notation.
    pub shortcut: Option<String>,
    /// Host-evaluated condition.
    pub enablement: CommandEnablement,
}

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
    /// Project-explorer item context menu.
    ProjectExplorerContext,
    /// Command palette (commands are also discoverable through the default palette).
    CommandPalette,
}

/// A bounded, declarative document rendered by the host with native editor components.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiDocument {
    /// Stable document identifier within its owning extension.
    pub id: String,
    /// Safe elements in display order.
    #[serde(default)]
    pub elements: Vec<UiElement>,
}

/// Supported safe UI primitives. These values contain no markup, script, or arbitrary style.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiElement {
    /// Plain text.
    Text {
        /// Stable element identifier.
        id: String,
        /// Plain display text.
        text: String,
    },
    /// A section heading.
    Heading {
        /// Stable element identifier.
        id: String,
        /// Heading text.
        text: String,
    },
    /// A button that invokes an extension-owned command.
    Button {
        /// Stable element identifier.
        id: String,
        /// Button label.
        label: String,
        /// Registered extension-owned command.
        command_id: String,
    },
    /// A simple list.
    List {
        /// Stable element identifier.
        id: String,
        /// Plain list items.
        items: Vec<String>,
    },
    /// A two-column table.
    Table {
        /// Stable element identifier.
        id: String,
        /// Table rows.
        rows: Vec<[String; 2]>,
    },
    /// A bounded text input for generic extension forms.
    Input {
        /// Stable element identifier.
        id: String,
        /// Field label.
        label: String,
        /// Current default value.
        value: String,
        /// Whether the user must provide a value.
        required: bool,
        /// Maximum byte length.
        max_length: usize,
    },
    /// A fixed-option select control.
    Select {
        /// Stable element identifier.
        id: String,
        /// Field label.
        label: String,
        /// Available options.
        options: Vec<String>,
        /// Selected default.
        value: String,
    },
    /// A boolean form control.
    Checkbox {
        /// Stable element identifier.
        id: String,
        /// Field label.
        label: String,
        /// Default state.
        checked: bool,
    },
    /// A status indicator.
    Status {
        /// Stable element identifier.
        id: String,
        /// Status label.
        label: String,
        /// Semantic display tone.
        tone: StatusTone,
    },
}

/// Restrained semantic status colors.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusTone {
    /// Neutral information.
    Neutral,
    /// Successful state.
    Success,
    /// Caution state.
    Warning,
    /// Error state.
    Error,
}

/// A user-openable extension panel. Not shown until the user opens it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolWindowContribution {
    /// Owner-qualified stable identifier.
    pub id: String,
    /// User-facing title.
    pub title: String,
    /// Initial safe view.
    pub initial_view: UiDocument,
}

/// Namespaced setting schema.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingContribution {
    /// Fully qualified setting key.
    pub id: String,
    /// User-facing label.
    pub title: String,
    /// Optional help text.
    pub description: String,
    /// Type and default value.
    pub kind: SettingKind,
}

/// Supported setting types and validated defaults.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SettingKind {
    /// Boolean toggle.
    Boolean {
        /// Default value.
        default: bool,
    },
    /// Bounded integer value.
    Integer {
        /// Default value.
        default: i64,
        /// Inclusive minimum.
        min: i64,
        /// Inclusive maximum.
        max: i64,
    },
    /// Bounded plain-text value.
    Text {
        /// Default value.
        default: String,
        /// Maximum byte length.
        max_length: usize,
    },
    /// One of a fixed set of choices.
    Choice {
        /// Default option.
        default: String,
        /// Allowed options.
        options: Vec<String>,
    },
}

/// A typed setting value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SettingValue {
    /// Boolean value.
    Boolean(bool),
    /// Integer value.
    Integer(i64),
    /// Plain-text value.
    Text(String),
}

/// Approved semantic design tokens that extensions may override within their own theme.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesignToken {
    /// Main application background.
    Background,
    /// Panel background.
    Panel,
    /// Raised surface background.
    Raised,
    /// Main text color.
    Text,
    /// Muted text color.
    Muted,
    /// Border color.
    Border,
    /// Accent color.
    Accent,
    /// Selection background.
    Selection,
}

/// Safe theme definition using existing editor tokens only.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeContribution {
    /// Owner-qualified stable identifier.
    pub id: String,
    /// User-facing title.
    pub title: String,
    /// Palette used for unspecified tokens.
    pub base: ThemeBase,
    /// Safe token overrides.
    pub tokens: BTreeMap<DesignToken, String>,
}

/// Base palette used for unspecified tokens.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeBase {
    /// Dark palette base.
    Dark,
    /// Light palette base.
    Light,
}

/// Enrichment attached to matching diagnostics without replacing their provider data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticDetailContribution {
    /// Owner-qualified stable identifier.
    pub id: String,
    /// Optional diagnostic source selector.
    pub source: Option<String>,
    /// Optional diagnostic code selector.
    pub code: Option<String>,
    /// User-facing detail heading.
    pub title: String,
    /// Additional explanation.
    pub explanation: String,
    /// Optional extension-owned suggested action.
    #[serde(default)]
    pub suggested_command: Option<String>,
    /// Plain related diagnostic information.
    #[serde(default)]
    pub related: Vec<String>,
}

/// Typed selector and presentation data used when registering diagnostic enrichment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticDetailDefinition {
    /// Optional diagnostic source selector.
    pub source: Option<String>,
    /// Optional diagnostic code selector.
    pub code: Option<String>,
    /// User-facing detail heading.
    pub title: String,
    /// Additional explanation.
    pub explanation: String,
    /// Optional extension-owned suggested action.
    pub suggested_command: Option<String>,
    /// Plain related diagnostic information.
    pub related: Vec<String>,
}

/// Typed, reusable project-creation extension point. Host performs authorized I/O on submit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectWizardContribution {
    /// Owner-qualified stable identifier.
    pub id: String,
    /// User-facing title.
    pub title: String,
    /// Declarative form content, submitted for host-authorized operations.
    pub form: UiDocument,
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
    /// DAP adapter definitions from the manifest.
    pub debug_adapters: Vec<DebugAdapterContribution>,
    /// Suggested DAP launch and attach profiles from the manifest.
    pub debug_configurations: Vec<DebugConfigurationContribution>,
    /// Diagnostic provider declarations.
    pub diagnostic_providers: Vec<DiagnosticProviderContribution>,
    /// Extension-owned panels.
    pub tool_windows: Vec<ToolWindowContribution>,
    /// Namespaced settings schemas.
    pub settings: Vec<SettingContribution>,
    /// Safe theme definitions.
    pub themes: Vec<ThemeContribution>,
    /// Diagnostic presentation enrichments.
    pub diagnostic_details: Vec<DiagnosticDetailContribution>,
    /// Generic future project wizard declarations.
    pub project_wizards: Vec<ProjectWizardContribution>,
    /// Declarative run configurations.
    pub run_configurations: Vec<RunConfigurationContribution>,
}

/// A command and its callback registered by an extension.
#[derive(Clone)]
pub struct RegisteredCommand {
    /// Localized display label.
    pub title: String,
    /// Short command description.
    pub description: String,
    /// Optional keyboard shortcut.
    pub shortcut: Option<String>,
    /// Host-evaluated availability condition.
    pub enablement: CommandEnablement,
    /// Extension callback.
    pub callback: CommandCallback,
}

impl std::fmt::Debug for RegisteredCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisteredCommand")
            .field("title", &self.title)
            .field("description", &self.description)
            .field("shortcut", &self.shortcut)
            .field("enablement", &self.enablement)
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
        let run_configurations = manifest
            .run_configurations
            .iter()
            .cloned()
            .map(|mut item| {
                item.id = qualify_id(&manifest.id, &item.id);
                item
            })
            .collect();
        Self {
            extension_id: &manifest.id,
            capabilities: &manifest.capabilities,
            values: ExtensionContributions {
                languages: manifest.languages.clone(),
                language_servers: manifest.language_servers.clone(),
                debug_adapters: manifest.debug_adapters.clone(),
                debug_configurations: manifest.debug_configurations.clone(),
                run_configurations,
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
        self.command_with_options(id, title, description, CommandOptions::default(), callback)
    }

    /// Registers a command with optional shortcut and host-evaluated availability.
    ///
    /// # Errors
    /// Returns an error when the capability is undeclared or command metadata is invalid.
    pub fn command_with_options(
        &mut self,
        id: &str,
        title: impl Into<String>,
        description: impl Into<String>,
        options: CommandOptions,
        callback: CommandCallback,
    ) -> Result<String, ExtensionError> {
        self.require(Capability::Commands)?;
        validate_local_id(id)?;
        let title = title.into();
        let description = description.into();
        if title.trim().is_empty() || description.len() > 512 || description.contains('\0') {
            return Err(ExtensionError::InvalidContribution(
                "command title must not be empty and description must be at most 512 bytes".into(),
            ));
        }
        if options.shortcut.as_ref().is_some_and(|shortcut| {
            shortcut.trim().is_empty()
                || shortcut.len() > 48
                || shortcut.chars().any(char::is_control)
                || shortcut.split('+').count() < 2
        }) {
            return Err(ExtensionError::InvalidContribution(
                "command shortcut must include a modifier and fit within 48 printable characters"
                    .into(),
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
                    shortcut: options.shortcut,
                    enablement: options.enablement,
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
                | "settings_changed"
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

    /// Registers a user-openable panel rendered from bounded safe components.
    ///
    /// # Errors
    /// Returns an error when the capability is undeclared or the panel schema is invalid.
    pub fn tool_window(
        &mut self,
        id: &str,
        title: impl Into<String>,
        mut view: UiDocument,
    ) -> Result<String, ExtensionError> {
        self.require(Capability::UiContributions)?;
        validate_local_id(id)?;
        let title = title.into();
        validate_text(&title, "tool window title")?;
        qualify_ui_actions(&mut view, self.extension_id);
        validate_ui_document(&view, self.extension_id)?;
        let full_id = format!("{}.{}", self.extension_id, id);
        ensure_new_id(
            &self
                .values
                .tool_windows
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            &full_id,
        )?;
        self.values.tool_windows.push(ToolWindowContribution {
            id: full_id.clone(),
            title,
            initial_view: view,
        });
        Ok(full_id)
    }

    /// Registers a typed, namespaced setting with a validated default.
    ///
    /// # Errors
    /// Returns an error when the capability is undeclared or the setting schema is invalid.
    pub fn setting(
        &mut self,
        id: &str,
        title: impl Into<String>,
        description: impl Into<String>,
        kind: SettingKind,
    ) -> Result<String, ExtensionError> {
        self.require(Capability::Settings)?;
        validate_local_id(id)?;
        let title = title.into();
        let description = description.into();
        validate_text(&title, "setting title")?;
        if !description.is_empty() && description.len() > 512 {
            return Err(invalid_contribution(
                "setting description exceeds 512 bytes",
            ));
        }
        validate_setting_kind(&kind)?;
        let full_id = format!("{}.{}", self.extension_id, id);
        ensure_new_id(
            &self
                .values
                .settings
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            &full_id,
        )?;
        self.values.settings.push(SettingContribution {
            id: full_id.clone(),
            title,
            description,
            kind,
        });
        Ok(full_id)
    }

    /// Registers a theme limited to approved design tokens and hexadecimal colors.
    ///
    /// # Errors
    /// Returns an error when the capability is undeclared or a theme token is invalid.
    pub fn theme(
        &mut self,
        id: &str,
        title: impl Into<String>,
        base: ThemeBase,
        tokens: BTreeMap<DesignToken, String>,
    ) -> Result<String, ExtensionError> {
        self.require(Capability::Themes)?;
        validate_local_id(id)?;
        let title = title.into();
        validate_text(&title, "theme title")?;
        if tokens.len() > 8 || tokens.values().any(|color| !valid_hex_color(color)) {
            return Err(invalid_contribution(
                "theme colors must be #RRGGBB and use no more than eight design tokens",
            ));
        }
        let full_id = format!("{}.{}", self.extension_id, id);
        ensure_new_id(
            &self
                .values
                .themes
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            &full_id,
        )?;
        self.values.themes.push(ThemeContribution {
            id: full_id.clone(),
            title,
            base,
            tokens,
        });
        Ok(full_id)
    }

    /// Registers a non-destructive diagnostic explanation or suggested command.
    ///
    /// # Errors
    /// Returns an error when the capability is undeclared or the detail/action is invalid.
    pub fn diagnostic_detail(
        &mut self,
        id: &str,
        definition: DiagnosticDetailDefinition,
    ) -> Result<String, ExtensionError> {
        self.require(Capability::Diagnostics)?;
        validate_local_id(id)?;
        let DiagnosticDetailDefinition {
            source,
            code,
            title,
            explanation,
            suggested_command,
            related,
        } = definition;
        validate_text(&title, "diagnostic detail title")?;
        validate_text(&explanation, "diagnostic explanation")?;
        if source.as_ref().is_some_and(|value| value.len() > 128)
            || code.as_ref().is_some_and(|value| value.len() > 128)
            || related.len() > 16
            || related.iter().any(|item| item.len() > 512)
        {
            return Err(invalid_contribution(
                "diagnostic detail exceeds size limits",
            ));
        }
        let full_id = format!("{}.{}", self.extension_id, id);
        if let Some(command) = &suggested_command {
            let qualified = qualify_id(self.extension_id, command);
            if !self.values.commands.contains_key(&qualified) {
                return Err(invalid_contribution(
                    "diagnostic detail references an unregistered command",
                ));
            }
        }
        ensure_new_id(
            &self
                .values
                .diagnostic_details
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            &full_id,
        )?;
        self.values
            .diagnostic_details
            .push(DiagnosticDetailContribution {
                id: full_id.clone(),
                source,
                code,
                title,
                explanation,
                suggested_command: suggested_command
                    .map(|value| qualify_id(self.extension_id, &value)),
                related,
            });
        Ok(full_id)
    }

    /// Declares a generic project-creation form. Submission must be handled by authorized host APIs.
    ///
    /// # Errors
    /// Returns an error when the UI capability is undeclared or the form is invalid.
    pub fn project_wizard(
        &mut self,
        id: &str,
        title: impl Into<String>,
        mut form: UiDocument,
    ) -> Result<String, ExtensionError> {
        self.require(Capability::UiContributions)?;
        validate_local_id(id)?;
        let title = title.into();
        validate_text(&title, "project wizard title")?;
        qualify_ui_actions(&mut form, self.extension_id);
        validate_ui_document(&form, self.extension_id)?;
        let full_id = format!("{}.{}", self.extension_id, id);
        ensure_new_id(
            &self
                .values
                .project_wizards
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            &full_id,
        )?;
        self.values.project_wizards.push(ProjectWizardContribution {
            id: full_id.clone(),
            title,
            form,
        });
        Ok(full_id)
    }

    /// Registers a declarative run configuration owned by this extension.
    ///
    /// # Errors
    /// Returns an error when `RunConfigurations` is undeclared or fields are invalid.
    pub fn run_configuration(
        &mut self,
        mut contribution: RunConfigurationContribution,
    ) -> Result<String, ExtensionError> {
        self.require(Capability::RunConfigurations)?;
        validate_local_id(&contribution.id)?;
        let full_id = qualify_id(self.extension_id, &contribution.id);
        ensure_new_id(
            &self
                .values
                .run_configurations
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            &full_id,
        )?;
        contribution.id.clone_from(&full_id);
        validate_run_configuration_contribution(&contribution)?;
        self.values.run_configurations.push(contribution);
        Ok(full_id)
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

fn invalid_contribution(message: &str) -> ExtensionError {
    ExtensionError::InvalidContribution(message.into())
}
fn validate_run_configuration_contribution(
    item: &RunConfigurationContribution,
) -> Result<(), ExtensionError> {
    validate_text(&item.name, "run configuration name")?;
    let has_exec = item
        .executable
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty() && !value.contains('\0'));
    let has_tool = item
        .tool
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty() && !value.contains('\0'));
    if has_exec == has_tool {
        return Err(invalid_contribution(
            "run configuration requires exactly one of executable or tool",
        ));
    }
    if item
        .arguments
        .iter()
        .any(|arg| arg.len() > 4096 || arg.contains('\0'))
        || item.working_directory.len() > 4096
        || item.working_directory.contains('\0')
        || item
            .environment
            .iter()
            .any(|(k, v)| k.trim().is_empty() || k.contains('\0') || v.contains('\0'))
    {
        return Err(invalid_contribution(
            "run configuration contains invalid arguments, working directory, or environment",
        ));
    }
    if !matches!(item.output.as_str(), "run" | "terminal") {
        return Err(invalid_contribution(
            "run configuration output must be 'run' or 'terminal'",
        ));
    }
    if !matches!(item.save.as_str(), "ask" | "save" | "disk") {
        return Err(invalid_contribution(
            "run configuration save policy must be 'ask', 'save', or 'disk'",
        ));
    }
    Ok(())
}
fn validate_text(value: &str, label: &str) -> Result<(), ExtensionError> {
    if value.trim().is_empty() || value.len() > 512 || value.contains('\0') {
        Err(invalid_contribution(&format!(
            "{label} is empty or exceeds 512 bytes"
        )))
    } else {
        Ok(())
    }
}
fn qualify_id(owner: &str, id: &str) -> String {
    if id.starts_with(&format!("{owner}.")) {
        id.to_owned()
    } else {
        format!("{owner}.{id}")
    }
}
fn qualify_ui_actions(view: &mut UiDocument, owner: &str) {
    for element in &mut view.elements {
        if let UiElement::Button { command_id, .. } = element {
            *command_id = qualify_id(owner, command_id);
        }
    }
}
fn ensure_new_id(existing: &[&str], id: &str) -> Result<(), ExtensionError> {
    if existing.contains(&id) {
        Err(ExtensionError::DuplicateContribution(id.into()))
    } else {
        Ok(())
    }
}
fn valid_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn validate_setting_kind(kind: &SettingKind) -> Result<(), ExtensionError> {
    match kind {
        SettingKind::Integer { default, min, max }
            if min > max || default < min || default > max =>
        {
            return Err(invalid_contribution(
                "integer setting default must be within a valid range",
            ));
        }
        SettingKind::Text {
            default,
            max_length,
        } if *max_length > 4096 || default.len() > *max_length => {
            return Err(invalid_contribution(
                "text setting default exceeds its length limit",
            ));
        }
        SettingKind::Choice { default, options }
            if options.is_empty()
                || options.len() > 64
                || !options.contains(default)
                || options
                    .iter()
                    .any(|item| item.trim().is_empty() || item.len() > 128) =>
        {
            return Err(invalid_contribution(
                "choice setting needs a valid default and 1 to 64 options",
            ));
        }
        _ => {}
    }
    Ok(())
}
fn validate_ui_document(view: &UiDocument, owner: &str) -> Result<(), ExtensionError> {
    validate_local_id(&view.id)?;
    if view.elements.len() > 128 {
        return Err(invalid_contribution(
            "UI document contains more than 128 elements",
        ));
    }
    let mut ids = BTreeSet::new();
    for element in &view.elements {
        let (id, text) = match element {
            UiElement::Text { id, text } | UiElement::Heading { id, text } => (id, text.as_str()),
            UiElement::Button {
                id,
                label,
                command_id,
            } => {
                if !command_id.starts_with(&format!("{owner}.")) {
                    return Err(invalid_contribution(
                        "UI actions must reference commands owned by their extension",
                    ));
                }
                (id, label.as_str())
            }
            UiElement::List { id, items } => {
                if items.len() > 128 || items.iter().any(|item| item.len() > 1024) {
                    return Err(invalid_contribution("UI list exceeds size limits"));
                }
                (id, "")
            }
            UiElement::Table { id, rows } => {
                if rows.len() > 128 || rows.iter().flatten().any(|item| item.len() > 512) {
                    return Err(invalid_contribution("UI table exceeds size limits"));
                }
                (id, "")
            }
            UiElement::Input {
                id,
                label,
                value,
                required: _,
                max_length,
            } => {
                if *max_length > 4096 || value.len() > *max_length {
                    return Err(invalid_contribution("UI input exceeds its declared length"));
                }
                (id, label.as_str())
            }
            UiElement::Select {
                id,
                label,
                options,
                value,
            } => {
                if options.is_empty()
                    || options.len() > 64
                    || !options.contains(value)
                    || options.iter().any(|item| item.len() > 128)
                {
                    return Err(invalid_contribution(
                        "UI select needs a valid default and up to 64 options",
                    ));
                }
                (id, label.as_str())
            }
            UiElement::Checkbox { id, label, .. } | UiElement::Status { id, label, .. } => {
                (id, label.as_str())
            }
        };
        validate_local_id(id)?;
        if !ids.insert(id) {
            return Err(ExtensionError::DuplicateContribution(id.clone()));
        }
        if text.len() > 1024 || text.contains('\0') {
            return Err(invalid_contribution("UI text exceeds 1024 bytes"));
        }
    }
    Ok(())
}

fn validate_local_id(id: &str) -> Result<(), ExtensionError> {
    if id.is_empty()
        || id.split('.').any(|part| {
            part.is_empty()
                || !part.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'-'
                        || byte == b'_'
                })
        })
    {
        return Err(ExtensionError::InvalidContribution(format!(
            "'{id}' is not a valid lower-case local identifier"
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
}

/// Manager-local contribution store. Every record is indexed by its owner and removed as one
/// lifecycle operation; it deliberately has no process-global mutable state.
#[derive(Default)]
pub struct ContributionRegistry {
    owners: BTreeMap<String, ExtensionContributions>,
}

impl ContributionRegistry {
    /// Creates an empty owner-indexed contribution registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Atomically registers all contributions for one extension after checking ownership and IDs.
    ///
    /// # Errors
    /// Returns an error when ownership, identifiers, references, or schemas are invalid.
    pub fn register(
        &mut self,
        owner: &str,
        contributions: ExtensionContributions,
    ) -> Result<(), ExtensionError> {
        validate_identifier(owner).map_err(ExtensionError::InvalidContribution)?;
        if self.owners.contains_key(owner) {
            return Err(ExtensionError::DuplicateContribution(format!(
                "{owner} already has registered contributions"
            )));
        }
        if contribution_count(&contributions) > MAX_OWNER_CONTRIBUTIONS {
            return Err(invalid_contribution(
                "an extension may register at most 512 contributions",
            ));
        }
        if contribution_payload_bytes(&contributions) > MAX_OWNER_PAYLOAD_BYTES {
            return Err(invalid_contribution(
                "extension contribution payload exceeds 512 KiB",
            ));
        }
        let belongs_to_owner = |id: &str| id.starts_with(&format!("{owner}."));
        let mut ids = BTreeSet::new();
        let mut add = |id: &str| -> Result<(), ExtensionError> {
            if !belongs_to_owner(id) {
                return Err(invalid_contribution(&format!(
                    "contribution '{id}' is not owned by '{owner}'"
                )));
            }
            if !ids.insert(id.to_owned()) {
                return Err(ExtensionError::DuplicateContribution(id.into()));
            }
            Ok(())
        };
        for id in contributions.commands.keys() {
            add(id)?;
        }
        for item in &contributions.tool_windows {
            add(&item.id)?;
            validate_ui_document(&item.initial_view, owner)?;
            validate_action_commands(&item.initial_view, &contributions.commands)?;
        }
        for item in &contributions.settings {
            add(&item.id)?;
            validate_setting_kind(&item.kind)?;
        }
        for item in &contributions.themes {
            add(&item.id)?;
            if item.tokens.len() > 8 || item.tokens.values().any(|color| !valid_hex_color(color)) {
                return Err(invalid_contribution("invalid theme token color"));
            }
        }
        for item in &contributions.diagnostic_details {
            add(&item.id)?;
        }
        for item in &contributions.project_wizards {
            add(&item.id)?;
            validate_ui_document(&item.form, owner)?;
            validate_action_commands(&item.form, &contributions.commands)?;
        }
        for item in &contributions.diagnostic_providers {
            add(&item.id)?;
        }
        for item in &contributions.run_configurations {
            add(&item.id)?;
            validate_run_configuration_contribution(item)?;
        }
        for menu in &contributions.menus {
            if !belongs_to_owner(&menu.command_id)
                || !contributions.commands.contains_key(&menu.command_id)
            {
                return Err(invalid_contribution(
                    "menu contribution references a command not owned by its extension",
                ));
            }
        }
        for detail in &contributions.diagnostic_details {
            if detail.suggested_command.as_ref().is_some_and(|command| {
                !belongs_to_owner(command) || !contributions.commands.contains_key(command)
            }) {
                return Err(invalid_contribution(
                    "diagnostic detail references an unregistered command",
                ));
            }
        }
        self.owners.insert(owner.into(), contributions);
        Ok(())
    }

    /// Removes and returns every contribution owned by an extension.
    pub fn unregister(&mut self, owner: &str) -> Option<ExtensionContributions> {
        self.owners.remove(owner)
    }

    /// Returns the contribution set for one active owner.
    #[must_use]
    pub fn get(&self, owner: &str) -> Option<&ExtensionContributions> {
        self.owners.get(owner)
    }

    /// Returns all currently registered extension owners.
    pub fn owners(&self) -> impl Iterator<Item = &str> {
        self.owners.keys().map(String::as_str)
    }

    /// Returns all active extension-owned run configurations in owner order.
    pub fn run_configurations(
        &self,
    ) -> impl Iterator<Item = (&str, &RunConfigurationContribution)> {
        self.owners.iter().flat_map(|(owner, items)| {
            items
                .run_configurations
                .iter()
                .map(move |item| (owner.as_str(), item))
        })
    }
}

fn validate_action_commands(
    view: &UiDocument,
    commands: &BTreeMap<String, RegisteredCommand>,
) -> Result<(), ExtensionError> {
    for element in &view.elements {
        if let UiElement::Button { command_id, .. } = element {
            if !commands.contains_key(command_id) {
                return Err(invalid_contribution(&format!(
                    "UI action references unregistered command '{command_id}'"
                )));
            }
        }
    }
    Ok(())
}

const MAX_OWNER_CONTRIBUTIONS: usize = 512;
const MAX_OWNER_PAYLOAD_BYTES: usize = 512 * 1024;

fn ui_document_bytes(view: &UiDocument) -> usize {
    let mut bytes = view.id.len();
    for element in &view.elements {
        bytes = bytes.saturating_add(match element {
            UiElement::Text { id, text } | UiElement::Heading { id, text } => id.len() + text.len(),
            UiElement::Button {
                id,
                label,
                command_id,
            } => id.len() + label.len() + command_id.len(),
            UiElement::List { id, items } => {
                id.len() + items.iter().map(String::len).sum::<usize>()
            }
            UiElement::Table { id, rows } => {
                id.len() + rows.iter().flatten().map(String::len).sum::<usize>()
            }
            UiElement::Input {
                id, label, value, ..
            } => id.len() + label.len() + value.len(),
            UiElement::Select {
                id,
                label,
                options,
                value,
            } => {
                id.len()
                    + label.len()
                    + value.len()
                    + options.iter().map(String::len).sum::<usize>()
            }
            UiElement::Checkbox { id, label, .. } | UiElement::Status { id, label, .. } => {
                id.len() + label.len()
            }
        });
    }
    bytes
}

fn contribution_count(items: &ExtensionContributions) -> usize {
    items.commands.len()
        + items.menus.len()
        + items.events.len()
        + items.languages.len()
        + items.language_servers.len()
        + items.debug_adapters.len()
        + items.debug_configurations.len()
        + items.diagnostic_providers.len()
        + items.tool_windows.len()
        + items.settings.len()
        + items.themes.len()
        + items.diagnostic_details.len()
        + items.project_wizards.len()
        + items.run_configurations.len()
}

fn contribution_payload_bytes(items: &ExtensionContributions) -> usize {
    let mut bytes = 0usize;
    for (id, command) in &items.commands {
        bytes = bytes.saturating_add(
            id.len()
                + command.title.len()
                + command.description.len()
                + command.shortcut.as_ref().map_or(0, String::len),
        );
    }
    for menu in &items.menus {
        bytes = bytes.saturating_add(menu.command_id.len() + menu.label.len());
    }
    for event in &items.events {
        bytes = bytes.saturating_add(event.len());
    }
    for language in &items.languages {
        bytes = bytes.saturating_add(
            language.id.len()
                + language.name.len()
                + language.extensions.iter().map(String::len).sum::<usize>(),
        );
    }
    for server in &items.language_servers {
        bytes = bytes.saturating_add(
            server.id.len()
                + server.command.len()
                + server.arguments.iter().map(String::len).sum::<usize>()
                + server.languages.iter().map(String::len).sum::<usize>()
                + server
                    .file_extensions
                    .iter()
                    .map(String::len)
                    .sum::<usize>()
                + server
                    .environment
                    .iter()
                    .map(|(key, value)| key.len() + value.len())
                    .sum::<usize>(),
        );
    }
    bytes = bytes.saturating_add(debug_contribution_payload_bytes(items));
    for provider in &items.diagnostic_providers {
        bytes = bytes.saturating_add(provider.id.len() + provider.name.len());
    }
    for panel in &items.tool_windows {
        bytes = bytes.saturating_add(
            panel.id.len() + panel.title.len() + ui_document_bytes(&panel.initial_view),
        );
    }
    for setting in &items.settings {
        bytes = bytes
            .saturating_add(setting.id.len() + setting.title.len() + setting.description.len());
        bytes = bytes.saturating_add(match &setting.kind {
            SettingKind::Boolean { .. } => 1,
            SettingKind::Integer { .. } => 24,
            SettingKind::Text { default, .. } => default.len(),
            SettingKind::Choice { default, options } => {
                default.len() + options.iter().map(String::len).sum::<usize>()
            }
        });
    }
    for theme in &items.themes {
        bytes = bytes.saturating_add(
            theme.id.len()
                + theme.title.len()
                + theme.tokens.values().map(String::len).sum::<usize>(),
        );
    }
    for detail in &items.diagnostic_details {
        bytes = bytes.saturating_add(
            detail.id.len()
                + detail.source.as_ref().map_or(0, String::len)
                + detail.code.as_ref().map_or(0, String::len)
                + detail.title.len()
                + detail.explanation.len()
                + detail.suggested_command.as_ref().map_or(0, String::len)
                + detail.related.iter().map(String::len).sum::<usize>(),
        );
    }
    for wizard in &items.project_wizards {
        bytes = bytes
            .saturating_add(wizard.id.len() + wizard.title.len() + ui_document_bytes(&wizard.form));
    }
    bytes
}

fn debug_contribution_payload_bytes(items: &ExtensionContributions) -> usize {
    let adapter_bytes = items
        .debug_adapters
        .iter()
        .map(|adapter| {
            adapter.id.len()
                + adapter.name.len()
                + adapter.command.len()
                + adapter.working_directory.len()
                + adapter.arguments.iter().map(String::len).sum::<usize>()
                + adapter
                    .environment
                    .iter()
                    .map(|(key, value)| key.len() + value.len())
                    .sum::<usize>()
        })
        .sum::<usize>();
    let configuration_bytes = items
        .debug_configurations
        .iter()
        .map(|configuration| {
            configuration.id.len()
                + configuration.name.len()
                + configuration.adapter_id.len()
                + configuration.program.as_ref().map_or(0, String::len)
                + configuration.working_directory.len()
                + configuration
                    .arguments
                    .iter()
                    .map(String::len)
                    .sum::<usize>()
                + configuration
                    .environment
                    .iter()
                    .map(|(key, value)| key.len() + value.len())
                    .sum::<usize>()
                + serde_json::to_vec(&configuration.adapter_settings).map_or(0, |value| value.len())
        })
        .sum::<usize>();
    adapter_bytes.saturating_add(configuration_bytes)
}

/// Manager-local extension registry. It does not load code or communicate with the desktop.
#[derive(Default)]
pub struct ExtensionManager {
    extensions: BTreeMap<String, ExtensionRecord>,
    contributions: ContributionRegistry,
}

impl std::fmt::Debug for ExtensionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionManager")
            .field("extensions", &self.list())
            .field(
                "contribution_owners",
                &self.contributions.owners().collect::<Vec<_>>(),
            )
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
        match outcome {
            Ok(Ok(())) => {
                if let Err(error) = self
                    .contributions
                    .register(id, staging.into_contributions())
                {
                    let record = self.record_mut(id)?;
                    record.state = ExtensionState::Failed;
                    record.error = Some(error.to_string());
                    return Err(error);
                }
                let record = self.record_mut(id)?;
                record.state = ExtensionState::Active;
                record.error = None;
                Ok(())
            }
            Ok(Err(error)) => {
                self.contributions.unregister(id);
                let record = self.record_mut(id)?;
                record.state = ExtensionState::Failed;
                record.error = Some(error.clone());
                Err(ExtensionError::ActivationFailed {
                    id: id.into(),
                    message: error,
                })
            }
            Err(_) => {
                let error = "activation panicked".to_owned();
                self.contributions.unregister(id);
                let record = self.record_mut(id)?;
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
        self.contributions.unregister(id);
        let record = self.record_mut(id)?;
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
        self.contributions.unregister(id);
        let record = self.record_mut(id)?;
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
        self.contributions
            .get(id)
            .ok_or_else(|| ExtensionError::NotActive(id.into()))
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
        for (owner, record) in &self.extensions {
            if record.state == ExtensionState::Active {
                if let Some(command) = self
                    .contributions
                    .get(owner)
                    .and_then(|items| items.commands.get(id))
                {
                    let enabled = match command.enablement {
                        CommandEnablement::Always => true,
                        CommandEnablement::WorkspaceOpen => context.workspace_root.is_some(),
                        CommandEnablement::ActiveDocument => context.active_document_id.is_some(),
                    };
                    if !enabled {
                        return Err(ExtensionError::CommandDisabled(id.into()));
                    }
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
    /// An extension command's declared availability condition is not met.
    CommandDisabled(String),
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
            Self::CommandDisabled(id) => {
                write!(f, "extension command '{id}' is currently disabled")
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
            debug_adapters: Vec::new(),
            debug_configurations: Vec::new(),
            run_configurations: Vec::new(),
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
    fn debug_manifest_contributions_require_capability_and_are_removed_on_disable() {
        let adapter = DebugAdapterContribution {
            id: "python-debugger".into(),
            name: "Python DAP".into(),
            command: "debugpy-adapter".into(),
            arguments: Vec::new(),
            working_directory: "${workspaceFolder}".into(),
            environment: BTreeMap::new(),
        };
        let configuration = DebugConfigurationContribution {
            id: "launch-main".into(),
            name: "Launch main".into(),
            adapter_id: adapter.id.clone(),
            request: DebugRequestContribution::Launch,
            program: Some("${workspaceFolder}/main.py".into()),
            process_id: None,
            arguments: Vec::new(),
            working_directory: "${workspaceFolder}".into(),
            environment: BTreeMap::new(),
            adapter_settings: serde_json::Map::new(),
            stop_on_entry: false,
        };
        let mut invalid = manifest(&[]);
        invalid.debug_adapters.push(adapter.clone());
        invalid.debug_configurations.push(configuration.clone());
        assert!(matches!(
            invalid.validate(&Version::parse(EXTENSION_API_VERSION).unwrap()),
            Err(ExtensionError::MissingCapability(Capability::DebugAdapters))
        ));

        let mut valid = manifest(&[Capability::DebugAdapters]);
        valid.debug_adapters.push(adapter);
        valid.debug_configurations.push(configuration);
        valid
            .validate(&Version::parse(EXTENSION_API_VERSION).unwrap())
            .unwrap();

        let dir = temp_dir();
        fs::write(dir.join("main.lua"), "return {}\n").unwrap();
        let mut manager = ExtensionManager::new();
        manager.register_installed(valid, dir.clone()).unwrap();
        manager.enable("org.example.hello").unwrap();
        manager
            .activate("org.example.hello", |_manifest, _builder| Ok(()))
            .unwrap();
        let contributions = manager.contributions("org.example.hello").unwrap();
        assert_eq!(contributions.debug_adapters.len(), 1);
        assert_eq!(contributions.debug_configurations.len(), 1);
        manager.disable("org.example.hello").unwrap();
        assert!(manager.contributions("org.example.hello").is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn debug_manifest_rejects_profiles_for_missing_adapters_and_invalid_process_values() {
        let mut value = manifest(&[Capability::DebugAdapters]);
        value
            .debug_configurations
            .push(DebugConfigurationContribution {
                id: "launch-main".into(),
                name: "Launch main".into(),
                adapter_id: "missing".into(),
                request: DebugRequestContribution::Launch,
                program: Some("main.py".into()),
                process_id: None,
                arguments: Vec::new(),
                working_directory: ".".into(),
                environment: BTreeMap::new(),
                adapter_settings: serde_json::Map::new(),
                stop_on_entry: false,
            });
        assert!(
            value
                .validate(&Version::parse(EXTENSION_API_VERSION).unwrap())
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
    fn registry_validates_owned_ui_and_removes_all_categories_on_deactivation() {
        let mut manager = ExtensionManager::new();
        let dir = temp_dir();
        fs::write(dir.join("main.lua"), "return {}\n").unwrap();
        let mut extension = manifest(&[
            Capability::Commands,
            Capability::UiContributions,
            Capability::Settings,
            Capability::Themes,
            Capability::Diagnostics,
        ]);
        extension.capabilities.extend([
            Capability::Settings,
            Capability::Themes,
            Capability::Diagnostics,
        ]);
        manager.register_installed(extension, dir.clone()).unwrap();
        manager.enable("org.example.hello").unwrap();
        manager
            .activate("org.example.hello", |_manifest, builder| {
                builder
                    .command(
                        "refresh",
                        "Refresh",
                        "Update developer info",
                        Arc::new(|_, _| Ok("updated".into())),
                    )
                    .map_err(|error| error.to_string())?;
                builder
                    .tool_window(
                        "workspace",
                        "Workspace Info",
                        UiDocument {
                            id: "main".into(),
                            elements: vec![UiElement::Button {
                                id: "refresh-action".into(),
                                label: "Refresh".into(),
                                command_id: "refresh".into(),
                            }],
                        },
                    )
                    .map_err(|error| error.to_string())?;
                builder
                    .setting(
                        "enabled",
                        "Enabled",
                        "",
                        SettingKind::Boolean { default: true },
                    )
                    .map_err(|error| error.to_string())?;
                builder
                    .theme(
                        "midnight",
                        "Midnight",
                        ThemeBase::Dark,
                        BTreeMap::from([(DesignToken::Accent, "#62a8ff".into())]),
                    )
                    .map_err(|error| error.to_string())?;
                builder
                    .diagnostic_detail(
                        "extra-help",
                        DiagnosticDetailDefinition {
                            source: None,
                            code: None,
                            title: "Extra help".into(),
                            explanation: "Explanation".into(),
                            suggested_command: Some("refresh".into()),
                            related: vec![],
                        },
                    )
                    .map_err(|error| error.to_string())?;
                Ok(())
            })
            .unwrap();
        let values = manager.contributions("org.example.hello").unwrap();
        assert_eq!(values.tool_windows.len(), 1);
        assert_eq!(values.settings.len(), 1);
        assert_eq!(values.themes.len(), 1);
        assert_eq!(values.diagnostic_details.len(), 1);
        manager.deactivate("org.example.hello").unwrap();
        assert!(manager.contributions("org.example.hello").is_err());
        assert!(manager.contributions.owners().next().is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn registry_rejects_cross_owner_ids_and_invalid_safe_views() {
        let mut registry = ContributionRegistry::new();
        let mut values = ExtensionContributions::default();
        values.tool_windows.push(ToolWindowContribution {
            id: "org.other.panel".into(),
            title: "Panel".into(),
            initial_view: UiDocument {
                id: "main".into(),
                elements: vec![],
            },
        });
        assert!(matches!(
            registry.register("org.example.owner", values),
            Err(ExtensionError::InvalidContribution(_))
        ));
        let repeated = ToolWindowContribution {
            id: "org.example.owner.panel".into(),
            title: "Panel".into(),
            initial_view: UiDocument {
                id: "main".into(),
                elements: vec![],
            },
        };
        let duplicate_values = ExtensionContributions {
            tool_windows: vec![repeated.clone(), repeated],
            ..ExtensionContributions::default()
        };
        assert!(matches!(
            registry.register("org.example.owner", duplicate_values),
            Err(ExtensionError::DuplicateContribution(_))
        ));
        let builder_manifest = manifest(&[Capability::UiContributions, Capability::Commands]);
        let mut builder = ContributionBuilder::new(&builder_manifest);
        builder
            .command("ok", "OK", "Action", Arc::new(|_, _| Ok(String::new())))
            .unwrap();
        let invalid = UiDocument {
            id: "main".into(),
            elements: vec![UiElement::Button {
                id: "do-it".into(),
                label: "Do".into(),
                command_id: "org.example.hello.missing".into(),
            }],
        };
        let mut values = builder.into_contributions();
        values.tool_windows.push(ToolWindowContribution {
            id: "org.example.hello.panel".into(),
            title: "Panel".into(),
            initial_view: invalid,
        });
        assert!(matches!(
            registry.register(&builder_manifest.id, values),
            Err(ExtensionError::InvalidContribution(_))
        ));
    }

    #[test]
    fn registry_bounds_aggregate_contribution_payloads_and_counts() {
        let mut registry = ContributionRegistry::new();
        let mut oversized = ExtensionContributions::default();
        let text = "x".repeat(1024);
        let elements = (0..128)
            .map(|index| UiElement::Text {
                id: format!("item-{index}"),
                text: text.clone(),
            })
            .collect::<Vec<_>>();
        for index in 0..5 {
            oversized.tool_windows.push(ToolWindowContribution {
                id: format!("org.example.owner.panel-{index}"),
                title: "Panel".into(),
                initial_view: UiDocument {
                    id: "main".into(),
                    elements: elements.clone(),
                },
            });
        }
        assert!(matches!(
            registry.register("org.example.owner", oversized),
            Err(ExtensionError::InvalidContribution(_))
        ));

        let settings = (0..=MAX_OWNER_CONTRIBUTIONS)
            .map(|index| SettingContribution {
                id: format!("org.example.owner.setting-{index}"),
                title: "Setting".into(),
                description: String::new(),
                kind: SettingKind::Boolean { default: false },
            })
            .collect();
        let too_many = ExtensionContributions {
            settings,
            ..ExtensionContributions::default()
        };
        assert!(matches!(
            registry.register("org.example.owner", too_many),
            Err(ExtensionError::InvalidContribution(_))
        ));
    }

    #[test]
    fn command_enablement_is_enforced_by_the_shared_host_registry() {
        let mut manager = ExtensionManager::new();
        let dir = temp_dir();
        fs::write(dir.join("main.lua"), "return {}\n").unwrap();
        manager
            .register_installed(manifest(&[Capability::Commands]), dir.clone())
            .unwrap();
        manager.enable("org.example.hello").unwrap();
        manager
            .activate("org.example.hello", |_manifest, builder| {
                builder
                    .command_with_options(
                        "document-action",
                        "Document action",
                        "Needs an active document",
                        CommandOptions {
                            shortcut: Some("Ctrl+Alt+D".into()),
                            enablement: CommandEnablement::ActiveDocument,
                        },
                        Arc::new(|_, _| Ok("ran".into())),
                    )
                    .map_err(|error| error.to_string())?;
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            manager.invoke_command(
                "org.example.hello.document-action",
                &CommandContext::default(),
                &[]
            ),
            Err(ExtensionError::CommandDisabled(_))
        ));
        assert_eq!(
            manager
                .invoke_command(
                    "org.example.hello.document-action",
                    &CommandContext {
                        active_document_id: Some(7),
                        ..CommandContext::default()
                    },
                    &[]
                )
                .unwrap(),
            "ran"
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

    #[test]
    fn run_configuration_contributions_require_capability_and_clean_up_on_unregister() {
        let mut missing_cap = manifest(&[]);
        missing_cap
            .run_configurations
            .push(RunConfigurationContribution {
                id: "run-lua".into(),
                name: "Run Lua".into(),
                executable: Some("lua5.4".into()),
                tool: None,
                arguments: vec!["${file}".into()],
                working_directory: "${workspace}".into(),
                environment: BTreeMap::new(),
                languages: vec!["lua".into()],
                output: "run".into(),
                save: "ask".into(),
                timeout_ms: None,
                debug_adapter: None,
                configurable_arguments: false,
            });
        assert!(matches!(
            missing_cap.validate(&Version::new(1, 0, 0)),
            Err(ExtensionError::MissingCapability(
                Capability::RunConfigurations
            ))
        ));

        let allowed = manifest(&[Capability::RunConfigurations]);
        let mut builder = ContributionBuilder::new(&allowed);
        let registered_id = builder
            .run_configuration(RunConfigurationContribution {
                id: "run-lua".into(),
                name: "Run Lua".into(),
                executable: Some("lua5.4".into()),
                tool: None,
                arguments: vec!["${file}".into()],
                working_directory: "${workspace}".into(),
                environment: BTreeMap::new(),
                languages: vec!["lua".into()],
                output: "run".into(),
                save: "ask".into(),
                timeout_ms: None,
                debug_adapter: None,
                configurable_arguments: true,
            })
            .unwrap();
        assert_eq!(registered_id, "org.example.hello.run-lua");
        assert!(
            builder
                .run_configuration(RunConfigurationContribution {
                    id: "run-lua".into(),
                    name: "Duplicate".into(),
                    executable: Some("lua5.4".into()),
                    tool: None,
                    arguments: vec![],
                    working_directory: String::new(),
                    environment: BTreeMap::new(),
                    languages: vec![],
                    output: "run".into(),
                    save: "ask".into(),
                    timeout_ms: None,
                    debug_adapter: None,
                    configurable_arguments: false,
                })
                .is_err()
        );
        let mut registry = ContributionRegistry::new();
        registry
            .register("org.example.hello", builder.into_contributions())
            .unwrap();
        assert_eq!(registry.run_configurations().count(), 1);
        registry.unregister("org.example.hello");
        assert_eq!(registry.run_configurations().count(), 0);
    }
}
