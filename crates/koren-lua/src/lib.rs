//! Lua 5.4 runtime adapter for the shared Koren extension contributions.
//!
//! Lua code receives only the typed Koren API table. The safe Lua standard-library subset is
//! loaded; `io`, `os`, `debug`, `package`, and native C-module loading are unavailable. The
//! runtime is capability checked, memory limited, and instruction limited, but is **not** an
//! operating-system security sandbox. No task/process or network API is exposed here.

use koren_extensions::{
    Capability, CommandCallback, CommandContext, CommandEnablement, CommandOptions,
    ContributionBuilder, DesignToken, DiagnosticDetailDefinition, ExtensionError,
    ExtensionManifest, MenuLocation, SettingKind, SettingValue, ThemeBase, UiDocument, UiElement,
};
use koren_protocol::{TextPosition, TextRange};
use mlua::{Function, HookTriggers, Lua, LuaOptions, RegistryKey, StdLib, Table, Value, VmState};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

/// Default per-VM Lua memory ceiling.
pub const DEFAULT_LUA_MEMORY_LIMIT: usize = 32 * 1024 * 1024;
/// Default maximum VM instruction count per activation, command, or event callback.
pub const DEFAULT_LUA_INSTRUCTION_LIMIT: u64 = 5_000_000;

/// Runtime resource bounds. These constrain the VM, not work performed by the host APIs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LuaLimits {
    /// Maximum Lua allocator usage in bytes.
    pub memory_bytes: usize,
    /// Maximum VM instructions between hook checks.
    pub instructions: u64,
}

impl Default for LuaLimits {
    fn default() -> Self {
        Self {
            memory_bytes: DEFAULT_LUA_MEMORY_LIMIT,
            instructions: DEFAULT_LUA_INSTRUCTION_LIMIT,
        }
    }
}

/// Safe host context visible to an extension callback.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LuaHostContext {
    /// Current local workspace root.
    pub workspace_root: Option<PathBuf>,
    /// Active document snapshot, supplied by the host for the duration it chooses.
    pub active_document: Option<LuaDocumentContext>,
    /// Effective host-validated namespaced extension settings.
    pub settings: BTreeMap<String, SettingValue>,
}

/// Active document data made available to extensions that declare `document_read`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LuaDocumentContext {
    /// Manager-local document identifier.
    pub id: u64,
    /// Workspace-relative document path, when one exists.
    pub path: Option<PathBuf>,
    /// Authoritative revision at snapshot time.
    pub revision: u64,
    /// Snapshot text from the host document manager.
    pub contents: String,
}

/// Revision-checked edit request returned to the host for application through `DocumentManager`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentEditRequest {
    /// Document to edit.
    pub document_id: u64,
    /// Revision against which the extension calculated this edit.
    pub expected_revision: u64,
    /// UTF-16 replacement range.
    pub range: TextRange,
    /// Replacement text.
    pub new_text: String,
}

#[derive(Clone)]
struct StagedCommand {
    id: String,
    title: String,
    description: String,
    options: CommandOptions,
}
#[derive(Clone)]
struct StagedMenu {
    command_id: String,
    label: String,
    location: MenuLocation,
}
#[derive(Clone)]
struct StagedToolWindow {
    id: String,
    title: String,
    text: String,
}
#[derive(Clone)]
struct StagedSetting {
    id: String,
    title: String,
    kind: SettingKind,
}
#[derive(Clone)]
struct StagedTheme {
    id: String,
    title: String,
    base: ThemeBase,
    tokens: BTreeMap<DesignToken, String>,
}
#[derive(Clone)]
struct StagedDiagnosticDetail {
    id: String,
    source: Option<String>,
    code: Option<String>,
    title: String,
    explanation: String,
}
struct VmStateData {
    lua: Lua,
    command_keys: Arc<Mutex<BTreeMap<String, RegistryKey>>>,
    event_keys: Arc<Mutex<BTreeMap<String, Vec<RegistryKey>>>>,
    deactivate_key: Option<RegistryKey>,
    module_key: Option<RegistryKey>,
    context: Arc<Mutex<LuaHostContext>>,
    edit_requests: Arc<Mutex<VecDeque<DocumentEditRequest>>>,
    ui_updates: Arc<Mutex<VecDeque<(String, String)>>>,
    instructions: Arc<AtomicU64>,
    limits: LuaLimits,
}

/// One activated extension VM. Clones share the same Lua state and contribution lifetime.
#[derive(Clone)]
pub struct LuaExtensionRuntime {
    inner: Arc<Mutex<VmStateData>>,
}

impl std::fmt::Debug for LuaExtensionRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock().map_err(|_| std::fmt::Error)?;
        f.debug_struct("LuaExtensionRuntime")
            .field("memory_bytes", &inner.lua.used_memory())
            .field("memory_limit", &inner.limits.memory_bytes)
            .field("instruction_limit", &inner.limits.instructions)
            .finish()
    }
}

impl LuaExtensionRuntime {
    /// Loads and activates an extension entry point, then stages its typed contributions.
    ///
    /// # Errors
    /// Returns an error when the entry point is invalid, Lua execution fails, or a contribution
    /// cannot be registered.
    pub fn activate_file(
        manifest: &ExtensionManifest,
        extension_root: &Path,
        context: LuaHostContext,
        limits: LuaLimits,
        contributions: &mut ContributionBuilder<'_>,
    ) -> Result<Self, LuaRuntimeError> {
        let root = fs::canonicalize(extension_root)
            .map_err(|error| LuaRuntimeError::Host(error.to_string()))?;
        let entry = root.join(&manifest.entry_point);
        let resolved = fs::canonicalize(&entry).map_err(|error| {
            LuaRuntimeError::Host(format!("extension entry point unavailable: {error}"))
        })?;
        if !resolved.starts_with(&root) || !resolved.is_file() {
            return Err(LuaRuntimeError::Host(
                "extension entry point escapes its installation directory".into(),
            ));
        }
        let source = fs::read_to_string(&resolved).map_err(|error| {
            LuaRuntimeError::Host(format!("failed to read extension entry point: {error}"))
        })?;
        Self::activate_source(manifest, &source, context, limits, contributions)
    }

    /// Loads source code for deterministic tests and embedded trusted examples.
    ///
    /// # Errors
    /// Returns an error when limits are invalid, Lua execution fails, or a contribution cannot be
    /// registered.
    // The method deliberately performs setup in one transaction so callback references and
    // contributions are either fully staged together or dropped together on failure.
    #[allow(clippy::too_many_lines)]
    pub fn activate_source(
        manifest: &ExtensionManifest,
        source: &str,
        context: LuaHostContext,
        limits: LuaLimits,
        contributions: &mut ContributionBuilder<'_>,
    ) -> Result<Self, LuaRuntimeError> {
        if limits.memory_bytes < 1024 * 1024 || limits.instructions < 10_000 {
            return Err(LuaRuntimeError::Host(
                "Lua limits are below safe minimum values".into(),
            ));
        }
        let lua = Lua::new_with(
            StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH | StdLib::COROUTINE,
            LuaOptions::default(),
        )
        .map_err(|error| lua_error(&error))?;
        let globals = lua.globals();
        globals
            .set("dofile", Value::Nil)
            .map_err(|error| lua_error(&error))?;
        globals
            .set("loadfile", Value::Nil)
            .map_err(|error| lua_error(&error))?;
        lua.set_memory_limit(limits.memory_bytes)
            .map_err(|error| lua_error(&error))?;
        let instructions = Arc::new(AtomicU64::new(0));
        let quota = instructions.clone();
        let max_instructions = limits.instructions;
        lua.set_global_hook(
            HookTriggers::new().every_nth_instruction(1_000),
            move |_, _| {
                let current = quota
                    .fetch_add(1_000, Ordering::Relaxed)
                    .saturating_add(1_000);
                if current > max_instructions {
                    Err(mlua::Error::RuntimeError(
                        "extension instruction budget exceeded".into(),
                    ))
                } else {
                    Ok(VmState::Continue)
                }
            },
        )
        .map_err(|error| lua_error(&error))?;

        let context = Arc::new(Mutex::new(context));
        let edit_requests = Arc::new(Mutex::new(VecDeque::new()));
        let ui_updates = Arc::new(Mutex::new(VecDeque::new()));
        let command_keys = Arc::new(Mutex::new(BTreeMap::<String, RegistryKey>::new()));
        let event_keys = Arc::new(Mutex::new(BTreeMap::<String, Vec<RegistryKey>>::new()));
        let staged_commands = Arc::new(Mutex::new(Vec::<StagedCommand>::new()));
        let staged_menus = Arc::new(Mutex::new(Vec::<StagedMenu>::new()));
        let staged_tool_windows = Arc::new(Mutex::new(Vec::<StagedToolWindow>::new()));
        let staged_settings = Arc::new(Mutex::new(Vec::<StagedSetting>::new()));
        let staged_themes = Arc::new(Mutex::new(Vec::<StagedTheme>::new()));
        let staged_diagnostic_details = Arc::new(Mutex::new(Vec::<StagedDiagnosticDetail>::new()));
        let api = create_api(
            &lua,
            manifest,
            &context,
            edit_requests.clone(),
            &command_keys,
            event_keys.clone(),
            &staged_commands,
            staged_menus.clone(),
            staged_tool_windows.clone(),
            staged_settings.clone(),
            staged_themes.clone(),
            staged_diagnostic_details.clone(),
            ui_updates.clone(),
        )
        .map_err(|error| lua_error(&error))?;
        lua.globals()
            .set("koren", api)
            .map_err(|error| lua_error(&error))?;
        instructions.store(0, Ordering::Relaxed);
        let module: Table = lua
            .load(source)
            .set_name(&manifest.id)
            .eval()
            .map_err(|error| lua_error(&error))?;
        let module_key = lua
            .create_registry_value(module.clone())
            .map_err(|error| lua_error(&error))?;
        let activate: Option<Function> =
            module.get("activate").map_err(|error| lua_error(&error))?;
        if let Some(activate) = activate {
            instructions.store(0, Ordering::Relaxed);
            activate.call::<()>(()).map_err(|error| lua_error(&error))?;
        }
        let deactivate: Option<Function> = module
            .get("deactivate")
            .map_err(|error| lua_error(&error))?;
        let deactivate_key = deactivate
            .map(|function| lua.create_registry_value(function))
            .transpose()
            .map_err(|error| lua_error(&error))?;

        let runtime = Self {
            inner: Arc::new(Mutex::new(VmStateData {
                lua,
                command_keys,
                event_keys,
                deactivate_key,
                module_key: Some(module_key),
                context,
                edit_requests,
                ui_updates,
                instructions,
                limits,
            })),
        };

        let staged = staged_commands
            .lock()
            .map_err(|_| LuaRuntimeError::Host("command registrations are unavailable".into()))?
            .clone();
        for command in staged {
            let id = command.id.clone();
            let runtime_clone = runtime.clone();
            let callback: CommandCallback = Arc::new(move |command_context, arguments| {
                runtime_clone
                    .call_command(&id, command_context, arguments)
                    .map_err(|error| error.to_string())
            });
            contributions
                .command_with_options(
                    &command.id,
                    command.title,
                    command.description,
                    command.options,
                    callback,
                )
                .map_err(|error| extension_error(&error))?;
        }
        let menus = staged_menus
            .lock()
            .map_err(|_| LuaRuntimeError::Host("menu registrations are unavailable".into()))?
            .clone();
        for menu in menus {
            contributions
                .menu(&menu.command_id, menu.label, menu.location)
                .map_err(|error| extension_error(&error))?;
        }
        for panel in staged_tool_windows
            .lock()
            .map_err(|_| LuaRuntimeError::Host("tool-window registration lock poisoned".into()))?
            .clone()
        {
            let full_id = contributions
                .tool_window(
                    &panel.id,
                    panel.title,
                    UiDocument {
                        id: "main".into(),
                        elements: vec![UiElement::Text {
                            id: "body".into(),
                            text: panel.text,
                        }],
                    },
                )
                .map_err(|error| extension_error(&error))?;
            let _ = full_id;
        }
        for setting in staged_settings
            .lock()
            .map_err(|_| LuaRuntimeError::Host("setting registration lock poisoned".into()))?
            .clone()
        {
            contributions
                .setting(&setting.id, setting.title, "", setting.kind)
                .map_err(|error| extension_error(&error))?;
        }
        for theme in staged_themes
            .lock()
            .map_err(|_| LuaRuntimeError::Host("theme registration lock poisoned".into()))?
            .clone()
        {
            contributions
                .theme(&theme.id, theme.title, theme.base, theme.tokens)
                .map_err(|error| extension_error(&error))?;
        }
        for detail in staged_diagnostic_details
            .lock()
            .map_err(|_| {
                LuaRuntimeError::Host("diagnostic detail registration lock poisoned".into())
            })?
            .clone()
        {
            contributions
                .diagnostic_detail(
                    &detail.id,
                    DiagnosticDetailDefinition {
                        source: detail.source,
                        code: detail.code,
                        title: detail.title,
                        explanation: detail.explanation,
                        suggested_command: None,
                        related: Vec::new(),
                    },
                )
                .map_err(|error| extension_error(&error))?;
        }
        let subscriptions = runtime
            .inner
            .lock()
            .map_err(|_| LuaRuntimeError::Host("Lua VM lock is poisoned".into()))?
            .event_keys
            .lock()
            .map_err(|_| LuaRuntimeError::Host("Lua event map is poisoned".into()))?
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for event in subscriptions {
            contributions
                .subscribe(&event)
                .map_err(|error| extension_error(&error))?;
        }
        Ok(runtime)
    }

    /// Replaces workspace and active-document context before a host invokes a callback.
    ///
    /// # Errors
    /// Returns an error if the runtime's synchronization lock is poisoned.
    pub fn update_context(&self, context: LuaHostContext) -> Result<(), LuaRuntimeError> {
        *self
            .inner
            .lock()
            .map_err(|_| LuaRuntimeError::Host("Lua VM lock is poisoned".into()))?
            .context
            .lock()
            .map_err(|_| LuaRuntimeError::Host("Lua context lock is poisoned".into()))? = context;
        Ok(())
    }

    /// Returns a snapshot of the active extension VM's current memory usage.
    ///
    /// # Errors
    /// Returns an error if the runtime's synchronization lock is poisoned.
    pub fn used_memory(&self) -> Result<usize, LuaRuntimeError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| LuaRuntimeError::Host("Lua VM lock is poisoned".into()))?
            .lua
            .used_memory())
    }

    /// Invokes an extension-owned command callback on the caller's thread.
    ///
    /// # Errors
    /// Returns an error when a lock is poisoned, the command is missing, or Lua execution fails.
    pub fn call_command(
        &self,
        id: &str,
        command_context: &CommandContext,
        arguments: &[String],
    ) -> Result<String, LuaRuntimeError> {
        let state = self
            .inner
            .lock()
            .map_err(|_| LuaRuntimeError::Host("Lua VM lock is poisoned".into()))?;
        {
            let mut context = state
                .context
                .lock()
                .map_err(|_| LuaRuntimeError::Host("Lua context lock is poisoned".into()))?;
            context
                .workspace_root
                .clone_from(&command_context.workspace_root);
            context.active_document =
                command_context
                    .active_document_id
                    .map(|id| LuaDocumentContext {
                        id,
                        path: command_context.active_document_path.clone(),
                        revision: command_context.active_document_revision.unwrap_or_default(),
                        contents: command_context
                            .active_document_contents
                            .clone()
                            .unwrap_or_default(),
                    });
        }
        state.instructions.store(0, Ordering::Relaxed);
        let function: Function = {
            let command_keys = state
                .command_keys
                .lock()
                .map_err(|_| LuaRuntimeError::Host("Lua command map is poisoned".into()))?;
            let key = command_keys
                .get(id)
                .ok_or_else(|| LuaRuntimeError::UnknownCommand(id.into()))?;
            state
                .lua
                .registry_value(key)
                .map_err(|error| lua_error(&error))?
        };
        let args = state
            .lua
            .create_table()
            .map_err(|error| lua_error(&error))?;
        for (index, argument) in arguments.iter().enumerate() {
            args.set(index + 1, argument.as_str())
                .map_err(|error| lua_error(&error))?;
        }
        let context = state
            .context
            .lock()
            .map_err(|_| LuaRuntimeError::Host("Lua context lock is poisoned".into()))?
            .clone();
        let context_table =
            context_table(&state.lua, &context).map_err(|error| lua_error(&error))?;
        function
            .call::<Value>((args, context_table))
            .map_err(|error| lua_error(&error))
            .and_then(|value| match value {
                Value::Nil => Ok(String::new()),
                Value::String(value) => value
                    .to_str()
                    .map(|value| value.to_string())
                    .map_err(|error| lua_error(&error)),
                other => other.to_string().map_err(|error| lua_error(&error)),
            })
    }

    /// Delivers a JSON event to callbacks registered for that event name.
    ///
    /// # Errors
    /// Returns an error when a lock is poisoned, the event is unknown, or Lua execution fails.
    pub fn dispatch_event(&self, event: &str, payload_json: &str) -> Result<(), LuaRuntimeError> {
        let state = self
            .inner
            .lock()
            .map_err(|_| LuaRuntimeError::Host("Lua VM lock is poisoned".into()))?;
        state.instructions.store(0, Ordering::Relaxed);
        let callbacks = {
            let event_keys = state
                .event_keys
                .lock()
                .map_err(|_| LuaRuntimeError::Host("Lua event map is poisoned".into()))?;
            event_keys
                .get(event)
                .ok_or_else(|| LuaRuntimeError::UnknownEvent(event.into()))?
                .iter()
                .map(|key| {
                    state
                        .lua
                        .registry_value::<Function>(key)
                        .map_err(|error| lua_error(&error))
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        for callback in callbacks {
            callback
                .call::<()>((payload_json,))
                .map_err(|error| lua_error(&error))?;
        }
        Ok(())
    }

    /// Drains revision-checked edit requests for the host to validate and apply.
    ///
    /// # Errors
    /// Returns an error if the runtime's synchronization lock is poisoned.
    pub fn take_edit_requests(&self) -> Result<Vec<DocumentEditRequest>, LuaRuntimeError> {
        let state = self
            .inner
            .lock()
            .map_err(|_| LuaRuntimeError::Host("Lua VM lock is poisoned".into()))?;
        Ok(state
            .edit_requests
            .lock()
            .map_err(|_| LuaRuntimeError::Host("edit queue lock is poisoned".into()))?
            .drain(..)
            .collect())
    }

    /// Drains bounded plain-text panel update requests for host-side validation and display.
    ///
    /// # Errors
    /// Returns an error if the runtime or update queue lock is poisoned.
    pub fn take_ui_updates(&self) -> Result<Vec<(String, String)>, LuaRuntimeError> {
        let state = self
            .inner
            .lock()
            .map_err(|_| LuaRuntimeError::Host("Lua VM lock is poisoned".into()))?;
        Ok(state
            .ui_updates
            .lock()
            .map_err(|_| LuaRuntimeError::Host("UI update queue lock is poisoned".into()))?
            .drain(..)
            .collect())
    }

    /// Calls the extension's optional `deactivate()` function and releases callback references.
    ///
    /// # Errors
    /// Returns an error when a lock is poisoned or the deactivate callback fails.
    pub fn deactivate(&self) -> Result<(), LuaRuntimeError> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| LuaRuntimeError::Host("Lua VM lock is poisoned".into()))?;
        if let Some(key) = &state.deactivate_key {
            state.instructions.store(0, Ordering::Relaxed);
            let callback: Function = state
                .lua
                .registry_value(key)
                .map_err(|error| lua_error(&error))?;
            callback.call::<()>(()).map_err(|error| lua_error(&error))?;
        }
        state
            .command_keys
            .lock()
            .map_err(|_| LuaRuntimeError::Host("Lua command map is poisoned".into()))?
            .clear();
        state
            .event_keys
            .lock()
            .map_err(|_| LuaRuntimeError::Host("Lua event map is poisoned".into()))?
            .clear();
        state
            .ui_updates
            .lock()
            .map_err(|_| LuaRuntimeError::Host("UI update queue lock is poisoned".into()))?
            .clear();
        state.deactivate_key = None;
        state.module_key = None;
        Ok(())
    }
}

// API assembly remains in one function so each exposed table is gated beside its capability
// check; the function's length reflects this security-sensitive registration boundary.
#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
fn create_api(
    lua: &Lua,
    manifest: &ExtensionManifest,
    context: &Arc<Mutex<LuaHostContext>>,
    edit_requests: Arc<Mutex<VecDeque<DocumentEditRequest>>>,
    command_keys: &Arc<Mutex<BTreeMap<String, RegistryKey>>>,
    event_keys: Arc<Mutex<BTreeMap<String, Vec<RegistryKey>>>>,
    staged_commands: &Arc<Mutex<Vec<StagedCommand>>>,
    staged_menus: Arc<Mutex<Vec<StagedMenu>>>,
    staged_tool_windows: Arc<Mutex<Vec<StagedToolWindow>>>,
    staged_settings: Arc<Mutex<Vec<StagedSetting>>>,
    staged_themes: Arc<Mutex<Vec<StagedTheme>>>,
    staged_diagnostic_details: Arc<Mutex<Vec<StagedDiagnosticDetail>>>,
    ui_updates: Arc<Mutex<VecDeque<(String, String)>>>,
) -> mlua::Result<Table> {
    let api = lua.create_table()?;

    if manifest.capabilities.contains(&Capability::Commands) {
        let commands = lua.create_table()?;
        let pending = Arc::clone(staged_commands);
        let command_registry = Arc::clone(command_keys);
        commands.set("register", lua.create_function(move |lua, (id, title, description, callback): (String, String, String, Function)| {
            if id.trim().is_empty() || id.contains('.') { return Err(mlua::Error::RuntimeError("command id must be a non-empty local name".into())); }
            let mut registrations = pending.lock().map_err(|_| mlua::Error::RuntimeError("command registration lock poisoned".into()))?;
            if registrations.iter().any(|command| command.id == id) { return Err(mlua::Error::RuntimeError(format!("duplicate command '{id}'"))); }
            let key = lua.create_registry_value(callback)?;
            command_registry.lock().map_err(|_| mlua::Error::RuntimeError("command registry lock poisoned".into()))?.insert(id.clone(), key);
            registrations.push(StagedCommand { id, title, description, options: CommandOptions::default() });
            Ok(())
        })?)?;
        let pending = Arc::clone(staged_commands);
        let command_registry = Arc::clone(command_keys);
        commands.set(
            "register_with_options",
            lua.create_function(
                move |lua,
                      (id, title, description, options, callback): (
                    String,
                    String,
                    String,
                    Table,
                    Function,
                )| {
                    if id.trim().is_empty() || id.contains('.') {
                        return Err(mlua::Error::RuntimeError(
                            "command id must be a non-empty local name".into(),
                        ));
                    }
                    let shortcut: Option<String> = options.get("shortcut")?;
                    let enablement = match options
                        .get::<Option<String>>("enablement")?
                        .as_deref()
                        .unwrap_or("always")
                    {
                        "always" => CommandEnablement::Always,
                        "workspace_open" => CommandEnablement::WorkspaceOpen,
                        "active_document" => CommandEnablement::ActiveDocument,
                        _ => {
                            return Err(mlua::Error::RuntimeError(
                                "unsupported command enablement condition".into(),
                            ));
                        }
                    };
                    let mut registrations = pending.lock().map_err(|_| {
                        mlua::Error::RuntimeError("command registration lock poisoned".into())
                    })?;
                    if registrations.iter().any(|command| command.id == id) {
                        return Err(mlua::Error::RuntimeError(format!(
                            "duplicate command '{id}'"
                        )));
                    }
                    let key = lua.create_registry_value(callback)?;
                    command_registry
                        .lock()
                        .map_err(|_| {
                            mlua::Error::RuntimeError("command registry lock poisoned".into())
                        })?
                        .insert(id.clone(), key);
                    registrations.push(StagedCommand {
                        id,
                        title,
                        description,
                        options: CommandOptions {
                            shortcut,
                            enablement,
                        },
                    });
                    Ok(())
                },
            )?,
        )?;
        api.set("commands", commands)?;
    }

    if manifest.capabilities.contains(&Capability::WorkspaceRead) {
        let workspace = lua.create_table()?;
        let current = context.clone();
        workspace.set(
            "root",
            lua.create_function(move |_, ()| {
                let context = current.lock().map_err(|_| {
                    mlua::Error::RuntimeError("workspace context unavailable".into())
                })?;
                Ok(context
                    .workspace_root
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()))
            })?,
        )?;
        api.set("workspace", workspace)?;
    }

    if manifest.capabilities.contains(&Capability::DocumentRead)
        || manifest.capabilities.contains(&Capability::DocumentEdit)
    {
        let document = lua.create_table()?;
        if manifest.capabilities.contains(&Capability::DocumentRead) {
            let current = context.clone();
            document.set(
                "active",
                lua.create_function(move |lua, ()| {
                    let context = current.lock().map_err(|_| {
                        mlua::Error::RuntimeError("document context unavailable".into())
                    })?;
                    match &context.active_document {
                        Some(doc) => {
                            let table = lua.create_table()?;
                            table.set("id", doc.id)?;
                            table.set(
                                "path",
                                doc.path
                                    .as_ref()
                                    .map(|path| path.to_string_lossy().into_owned()),
                            )?;
                            table.set("revision", doc.revision)?;
                            Ok(Value::Table(table))
                        }
                        None => Ok(Value::Nil),
                    }
                })?,
            )?;
            let current = context.clone();
            document.set(
                "read",
                lua.create_function(move |_, id: u64| {
                    let context = current.lock().map_err(|_| {
                        mlua::Error::RuntimeError("document context unavailable".into())
                    })?;
                    Ok(context
                        .active_document
                        .as_ref()
                        .filter(|doc| doc.id == id)
                        .map(|doc| doc.contents.clone()))
                })?,
            )?;
        }
        if manifest.capabilities.contains(&Capability::DocumentEdit) {
            let current = context.clone();
            let requests = edit_requests;
            document.set(
                "request_edit",
                lua.create_function(
                    move |_,
                          (id, revision, sl, sc, el, ec, text): (
                        u64,
                        u64,
                        usize,
                        usize,
                        usize,
                        usize,
                        String,
                    )| {
                        let context = current.lock().map_err(|_| {
                            mlua::Error::RuntimeError("document context unavailable".into())
                        })?;
                        if !context
                            .active_document
                            .as_ref()
                            .is_some_and(|doc| doc.id == id && doc.revision == revision)
                        {
                            return Err(mlua::Error::RuntimeError(
                                "edit request does not match the active document snapshot".into(),
                            ));
                        }
                        let request = DocumentEditRequest {
                            document_id: id,
                            expected_revision: revision,
                            range: TextRange {
                                start: TextPosition {
                                    line: sl,
                                    character: sc,
                                },
                                end: TextPosition {
                                    line: el,
                                    character: ec,
                                },
                            },
                            new_text: text,
                        };
                        drop(context);
                        requests
                            .lock()
                            .map_err(|_| {
                                mlua::Error::RuntimeError("edit queue unavailable".into())
                            })?
                            .push_back(request);
                        Ok(true)
                    },
                )?,
            )?;
        }
        api.set("document", document)?;
    }

    if manifest.capabilities.contains(&Capability::Events) {
        let events = lua.create_table()?;
        let pending = event_keys;
        events.set(
            "on",
            lua.create_function(move |lua, (event, callback): (String, Function)| {
                if !matches!(
                    event.as_str(),
                    "workspace_opened"
                        | "document_opened"
                        | "document_changed"
                        | "document_saved"
                        | "document_closed"
                        | "settings_changed"
                ) {
                    return Err(mlua::Error::RuntimeError(format!(
                        "unsupported event '{event}'"
                    )));
                }
                let key = lua.create_registry_value(callback)?;
                pending
                    .lock()
                    .map_err(|_| mlua::Error::RuntimeError("event registry lock poisoned".into()))?
                    .entry(event)
                    .or_default()
                    .push(key);
                Ok(())
            })?,
        )?;
        api.set("events", events)?;
    }

    if manifest.capabilities.contains(&Capability::UiContributions) {
        let ui = lua.create_table()?;
        let pending_panels = staged_tool_windows;
        ui.set(
            "register_tool_window",
            lua.create_function(move |_, (id, title, text): (String, String, String)| {
                if id.trim().is_empty()
                    || id.contains('.')
                    || title.trim().is_empty()
                    || text.len() > 16 * 1024
                {
                    return Err(mlua::Error::RuntimeError(
                        "tool window requires a local id, title, and text no longer than 16 KiB"
                            .into(),
                    ));
                }
                let mut panels = pending_panels.lock().map_err(|_| {
                    mlua::Error::RuntimeError("tool-window registration lock poisoned".into())
                })?;
                if panels.iter().any(|panel| panel.id == id) {
                    return Err(mlua::Error::RuntimeError(format!(
                        "duplicate tool window '{id}'"
                    )));
                }
                panels.push(StagedToolWindow { id, title, text });
                Ok(())
            })?,
        )?;
        let updates = ui_updates;
        ui.set(
            "update_tool_window",
            lua.create_function(move |_, (id, text): (String, String)| {
                if id.trim().is_empty() || id.contains('.') || text.len() > 16 * 1024 {
                    return Err(mlua::Error::RuntimeError(
                        "panel update needs a local id and text no longer than 16 KiB".into(),
                    ));
                }
                let mut pending = updates.lock().map_err(|_| {
                    mlua::Error::RuntimeError("UI update queue lock poisoned".into())
                })?;
                if pending.len() >= 32 {
                    return Err(mlua::Error::RuntimeError(
                        "too many pending panel updates".into(),
                    ));
                }
                pending.push_back((id, text));
                Ok(())
            })?,
        )?;
        ui.set(
            "add_menu_action",
            lua.create_function(
                move |_, (command_id, label, location): (String, String, String)| {
                    let location = match location.as_str() {
                        "tools" => MenuLocation::Tools,
                        "navigate" => MenuLocation::Navigate,
                        "editor_context" => MenuLocation::EditorContext,
                        "project_explorer_context" => MenuLocation::ProjectExplorerContext,
                        "command_palette" => MenuLocation::CommandPalette,
                        _ => {
                            return Err(mlua::Error::RuntimeError(
                                "menu location must be tools, navigate, or editor_context".into(),
                            ));
                        }
                    };
                    staged_menus
                        .lock()
                        .map_err(|_| {
                            mlua::Error::RuntimeError("menu registration lock poisoned".into())
                        })?
                        .push(StagedMenu {
                            command_id,
                            label,
                            location,
                        });
                    Ok(())
                },
            )?,
        )?;
        api.set("ui", ui)?;
    }

    if manifest.capabilities.contains(&Capability::Settings) {
        let settings = lua.create_table()?;
        let settings_context = context.clone();
        let owner = manifest.id.clone();
        settings.set(
            "get",
            lua.create_function(move |lua, id: String| {
                let key = format!("{owner}.{id}");
                let context = settings_context.lock().map_err(|_| {
                    mlua::Error::RuntimeError("settings context unavailable".into())
                })?;
                Ok(match context.settings.get(&key) {
                    Some(SettingValue::Boolean(value)) => Value::Boolean(*value),
                    Some(SettingValue::Integer(value)) => Value::Integer(*value),
                    Some(SettingValue::Text(value)) => Value::String(lua.create_string(value)?),
                    None => Value::Nil,
                })
            })?,
        )?;
        let pending = staged_settings.clone();
        settings.set(
            "register_boolean",
            lua.create_function(move |_, (id, title, default): (String, String, bool)| {
                stage_setting(
                    &pending,
                    StagedSetting {
                        id,
                        title,
                        kind: SettingKind::Boolean { default },
                    },
                )
            })?,
        )?;
        let pending = staged_settings.clone();
        settings.set(
            "register_integer",
            lua.create_function(
                move |_, (id, title, default, min, max): (String, String, i64, i64, i64)| {
                    stage_setting(
                        &pending,
                        StagedSetting {
                            id,
                            title,
                            kind: SettingKind::Integer { default, min, max },
                        },
                    )
                },
            )?,
        )?;
        let pending = staged_settings.clone();
        settings.set(
            "register_text",
            lua.create_function(
                move |_, (id, title, default, max_length): (String, String, String, usize)| {
                    stage_setting(
                        &pending,
                        StagedSetting {
                            id,
                            title,
                            kind: SettingKind::Text {
                                default,
                                max_length,
                            },
                        },
                    )
                },
            )?,
        )?;
        settings.set(
            "register_choice",
            lua.create_function(
                move |_, (id, title, default, options): (String, String, String, Vec<String>)| {
                    stage_setting(
                        &staged_settings,
                        StagedSetting {
                            id,
                            title,
                            kind: SettingKind::Choice { default, options },
                        },
                    )
                },
            )?,
        )?;
        api.set("settings", settings)?;
    }

    if manifest.capabilities.contains(&Capability::Themes) {
        let themes = lua.create_table()?;
        themes.set(
            "register",
            lua.create_function(
                move |_, (id, title, base, values): (String, String, String, Table)| {
                    let base = match base.as_str() {
                        "dark" => ThemeBase::Dark,
                        "light" => ThemeBase::Light,
                        _ => {
                            return Err(mlua::Error::RuntimeError(
                                "theme base must be dark or light".into(),
                            ));
                        }
                    };
                    let mut tokens = BTreeMap::new();
                    for pair in values.pairs::<String, String>() {
                        let (key, value) = pair?;
                        let token = match key.as_str() {
                            "background" => DesignToken::Background,
                            "panel" => DesignToken::Panel,
                            "raised" => DesignToken::Raised,
                            "text" => DesignToken::Text,
                            "muted" => DesignToken::Muted,
                            "border" => DesignToken::Border,
                            "accent" => DesignToken::Accent,
                            "selection" => DesignToken::Selection,
                            _ => {
                                return Err(mlua::Error::RuntimeError(format!(
                                    "unsupported design token '{key}'"
                                )));
                            }
                        };
                        tokens.insert(token, value);
                    }
                    let mut pending = staged_themes.lock().map_err(|_| {
                        mlua::Error::RuntimeError("theme registration lock poisoned".into())
                    })?;
                    if pending.iter().any(|theme| theme.id == id) {
                        return Err(mlua::Error::RuntimeError(format!("duplicate theme '{id}'")));
                    }
                    pending.push(StagedTheme {
                        id,
                        title,
                        base,
                        tokens,
                    });
                    Ok(())
                },
            )?,
        )?;
        api.set("themes", themes)?;
    }

    if manifest.capabilities.contains(&Capability::Diagnostics) {
        let diagnostics = lua.create_table()?;
        diagnostics.set(
            "provide_detail",
            lua.create_function(
                move |_,
                      (id, source, code, title, explanation): (
                    String,
                    Option<String>,
                    Option<String>,
                    String,
                    String,
                )| {
                    let mut pending = staged_diagnostic_details.lock().map_err(|_| {
                        mlua::Error::RuntimeError("diagnostic registration lock poisoned".into())
                    })?;
                    if pending.iter().any(|detail| detail.id == id) {
                        return Err(mlua::Error::RuntimeError(format!(
                            "duplicate diagnostic detail '{id}'"
                        )));
                    }
                    pending.push(StagedDiagnosticDetail {
                        id,
                        source,
                        code,
                        title,
                        explanation,
                    });
                    Ok(())
                },
            )?,
        )?;
        api.set("diagnostics", diagnostics)?;
    }

    Ok(api)
}

fn stage_setting(
    pending: &Arc<Mutex<Vec<StagedSetting>>>,
    setting: StagedSetting,
) -> mlua::Result<()> {
    if setting.id.trim().is_empty() || setting.id.contains('.') || setting.title.trim().is_empty() {
        return Err(mlua::Error::RuntimeError(
            "setting requires a local id and title".into(),
        ));
    }
    let mut settings = pending
        .lock()
        .map_err(|_| mlua::Error::RuntimeError("setting registration lock poisoned".into()))?;
    if settings.iter().any(|item| item.id == setting.id) {
        return Err(mlua::Error::RuntimeError(format!(
            "duplicate setting '{}'",
            setting.id
        )));
    }
    settings.push(setting);
    Ok(())
}

fn context_table(lua: &Lua, context: &LuaHostContext) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set(
        "workspace_root",
        context
            .workspace_root
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
    )?;
    if let Some(document) = &context.active_document {
        let active = lua.create_table()?;
        active.set("id", document.id)?;
        active.set(
            "path",
            document
                .path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        )?;
        active.set("revision", document.revision)?;
        table.set("active_document", active)?;
    }
    Ok(table)
}

fn extension_error(error: &ExtensionError) -> LuaRuntimeError {
    LuaRuntimeError::Registration(error.to_string())
}
fn lua_error(error: &mlua::Error) -> LuaRuntimeError {
    LuaRuntimeError::Execution(error.to_string())
}

/// Lua initialization, callback, capability, or host-adapter failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LuaRuntimeError {
    /// Lua compilation or callback execution failed.
    Execution(String),
    /// An extension-owned command identifier is not registered.
    UnknownCommand(String),
    /// An event subscription is not registered.
    UnknownEvent(String),
    /// A shared contribution could not be registered.
    Registration(String),
    /// The host could not read or validate extension data.
    Host(String),
}

impl std::fmt::Display for LuaRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Execution(message) => write!(f, "Lua extension error: {message}"),
            Self::UnknownCommand(id) => write!(f, "Lua extension command '{id}' is not registered"),
            Self::UnknownEvent(event) => {
                write!(f, "Lua extension has no callback for event '{event}'")
            }
            Self::Registration(message) => {
                write!(f, "Lua contribution registration failed: {message}")
            }
            Self::Host(message) => write!(f, "Lua extension host error: {message}"),
        }
    }
}

impl std::error::Error for LuaRuntimeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use koren_extensions::{ActivationConditions, EXTENSION_API_VERSION, ExtensionManifest};
    use std::collections::BTreeSet;

    fn manifest(capabilities: &[Capability]) -> ExtensionManifest {
        ExtensionManifest {
            schema_version: 1,
            id: "org.example.sample".into(),
            name: "Sample".into(),
            publisher: "Koren".into(),
            version: "0.1.0".into(),
            description: "Fixture".into(),
            entry_point: "main.lua".into(),
            api_version: EXTENSION_API_VERSION.into(),
            capabilities: capabilities.iter().copied().collect::<BTreeSet<_>>(),
            activation: ActivationConditions::default(),
            languages: Vec::new(),
            language_servers: Vec::new(),
        }
    }

    fn builder(manifest: &ExtensionManifest) -> ContributionBuilder<'_> {
        ContributionBuilder::new(manifest)
    }

    #[test]
    fn sample_registers_command_and_menu_and_runs_without_os_libraries() {
        let manifest = manifest(&[
            Capability::Commands,
            Capability::WorkspaceRead,
            Capability::UiContributions,
        ]);
        let source = r#"
            local m = {}
            function m.activate()
              assert(io == nil and os == nil and debug == nil and package == nil)
              koren.commands.register("hello", "Hello", "Say hello", function(args, context)
                return "Hello " .. (args[1] or context.workspace_root or "world")
              end)
              koren.ui.add_menu_action("hello", "Say Hello", "tools")
            end
            function m.deactivate() end
            return m
        "#;
        let mut contributions = builder(&manifest);
        let runtime = LuaExtensionRuntime::activate_source(
            &manifest,
            source,
            LuaHostContext {
                workspace_root: Some(PathBuf::from("/workspace/demo")),
                active_document: None,
                ..LuaHostContext::default()
            },
            LuaLimits::default(),
            &mut contributions,
        )
        .unwrap();
        let staged = contributions.into_contributions();
        let command = staged.commands.get("org.example.sample.hello").unwrap();
        assert_eq!(command.title, "Hello");
        assert_eq!(staged.menus[0].label, "Say Hello");
        let result = (command.callback)(
            &CommandContext {
                workspace_root: Some(PathBuf::from("/workspace/demo")),
                ..CommandContext::default()
            },
            &[],
        )
        .unwrap();
        assert_eq!(result, "Hello /workspace/demo");
        runtime.deactivate().unwrap();
    }

    #[test]
    fn lua_sdk_registers_panels_settings_themes_and_diagnostic_details() {
        let manifest = manifest(&[
            Capability::Commands,
            Capability::UiContributions,
            Capability::Settings,
            Capability::Themes,
            Capability::Diagnostics,
        ]);
        let source = r##"
            local m = {}
            function m.activate()
              koren.commands.register_with_options("refresh", "Refresh", "Refresh workspace details", {shortcut="Ctrl+Alt+R", enablement="workspace_open"}, function(_args, context)
                koren.ui.update_tool_window("workspace", "Workspace: " .. (context.workspace_root or "none"))
                return tostring(koren.settings.get("show_path"))
              end)
              koren.ui.register_tool_window("workspace", "Workspace Info", "Not refreshed")
              koren.settings.register_boolean("show_path", "Show path", true)
              koren.themes.register("midnight", "Midnight", "dark", {accent="#123456"})
              koren.diagnostics.provide_detail("type-help", "lsp", "E001", "Type detail", "A more detailed explanation")
            end
            return m
        "##;
        let mut context = LuaHostContext::default();
        context.settings.insert(
            "org.example.sample.show_path".into(),
            SettingValue::Boolean(false),
        );
        let mut contributions = builder(&manifest);
        let runtime = LuaExtensionRuntime::activate_source(
            &manifest,
            source,
            context,
            LuaLimits::default(),
            &mut contributions,
        )
        .unwrap();
        let owned = contributions.into_contributions();
        assert_eq!(owned.tool_windows[0].id, "org.example.sample.workspace");
        assert_eq!(owned.settings[0].id, "org.example.sample.show_path");
        assert_eq!(
            owned.themes[0]
                .tokens
                .get(&DesignToken::Accent)
                .map(String::as_str),
            Some("#123456")
        );
        assert_eq!(owned.diagnostic_details[0].code.as_deref(), Some("E001"));
        assert_eq!(
            owned.commands["org.example.sample.refresh"]
                .shortcut
                .as_deref(),
            Some("Ctrl+Alt+R")
        );
        assert_eq!(
            owned.commands["org.example.sample.refresh"].enablement,
            CommandEnablement::WorkspaceOpen
        );
        assert_eq!(
            runtime
                .call_command(
                    "refresh",
                    &CommandContext {
                        workspace_root: Some(PathBuf::from("/work")),
                        ..CommandContext::default()
                    },
                    &[]
                )
                .unwrap(),
            "false"
        );
        assert_eq!(
            runtime.take_ui_updates().unwrap(),
            vec![("workspace".into(), "Workspace: /work".into())]
        );
        runtime.deactivate().unwrap();
        assert_eq!(runtime.take_ui_updates().unwrap(), []);
    }

    #[test]
    fn document_capabilities_expose_snapshots_and_queue_revision_checked_edits() {
        let manifest = manifest(&[
            Capability::Commands,
            Capability::DocumentRead,
            Capability::DocumentEdit,
        ]);
        let source = r#"
            local m = {}
            function m.activate()
              koren.commands.register("edit", "Edit", "Request edit", function(args, ctx)
                local active = koren.document.active()
                local text = koren.document.read(active.id)
                assert(text == "hello")
                koren.document.request_edit(active.id, active.revision, 0, 0, 0, 5, "world")
                return tostring(ctx.active_document.revision)
              end)
            end
            return m
        "#;
        let mut contributions = builder(&manifest);
        let runtime = LuaExtensionRuntime::activate_source(
            &manifest,
            source,
            LuaHostContext {
                workspace_root: None,
                active_document: Some(LuaDocumentContext {
                    id: 42,
                    path: Some("main.txt".into()),
                    revision: 7,
                    contents: "hello".into(),
                }),
                ..LuaHostContext::default()
            },
            LuaLimits::default(),
            &mut contributions,
        )
        .unwrap();
        let context = CommandContext {
            active_document_id: Some(42),
            active_document_path: Some("main.txt".into()),
            active_document_revision: Some(7),
            active_document_contents: Some("hello".into()),
            ..CommandContext::default()
        };
        assert_eq!(runtime.call_command("edit", &context, &[]).unwrap(), "7");
        assert_eq!(
            runtime.take_edit_requests().unwrap(),
            [DocumentEditRequest {
                document_id: 42,
                expected_revision: 7,
                range: TextRange {
                    start: TextPosition {
                        line: 0,
                        character: 0
                    },
                    end: TextPosition {
                        line: 0,
                        character: 5
                    }
                },
                new_text: "world".into(),
            }]
        );
    }

    #[test]
    fn undeclared_apis_are_unavailable_and_vm_quota_stops_loops() {
        let manifest = manifest(&[Capability::Commands]);
        let source = r#"local m={}; function m.activate() assert(koren.workspace == nil and koren.document == nil); koren.commands.register("loop", "Loop", "Run forever", function() while true do end end) end; return m"#;
        let mut contributions = builder(&manifest);
        let runtime = LuaExtensionRuntime::activate_source(
            &manifest,
            source,
            LuaHostContext::default(),
            LuaLimits {
                memory_bytes: 4 * 1024 * 1024,
                instructions: 20_000,
            },
            &mut contributions,
        )
        .unwrap();
        let error = runtime
            .call_command("loop", &CommandContext::default(), &[])
            .unwrap_err();
        assert!(error.to_string().contains("instruction budget"));
    }

    #[test]
    fn lua_runtime_errors_and_deactivation_release_owned_callbacks() {
        let manifest = manifest(&[Capability::Commands, Capability::Events]);
        let mut contributions = builder(&manifest);
        let runtime = LuaExtensionRuntime::activate_source(&manifest,
            r#"local m={}; function m.activate() koren.commands.register("fail", "Fail", "Throw", function() error("boom") end); koren.events.on("document_changed", function(payload) end) end; return m"#,
            LuaHostContext::default(), LuaLimits::default(), &mut contributions).unwrap();
        assert!(
            runtime
                .call_command("fail", &CommandContext::default(), &[])
                .is_err()
        );
        runtime.dispatch_event("document_changed", "{}").unwrap();
        runtime.deactivate().unwrap();
        assert!(
            runtime
                .call_command("fail", &CommandContext::default(), &[])
                .is_err()
        );
    }
}
