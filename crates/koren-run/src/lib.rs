//! Language-neutral run configuration definitions and exact argument resolution.

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// A persisted collection of configurations. Loading never executes a program.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunFile {
    /// Format version; currently one.
    pub schema: u32,
    /// Configuration definitions.
    #[serde(default, rename = "run")]
    pub runs: Vec<RunConfiguration>,
}

/// Origin of a run definition. Only user and workspace definitions are editable.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RunSource {
    /// User configuration directory.
    User,
    /// Active workspace's `.koren/runs.toml`.
    Workspace,
    /// Project provider identity.
    Project(String),
    /// Extension identity.
    Extension(String),
}

/// Stable identity scoped by source, owner, and definition ID.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RunId {
    /// Definition origin.
    pub source: RunSource,
    /// Owner identity within the source. Empty for user/workspace files.
    pub owner: String,
    /// Stable definition ID.
    pub id: String,
}

/// Registry of inert run definitions. Registration never starts a process.
#[derive(Default)]
pub struct RunRegistry {
    definitions: BTreeMap<RunId, RunConfiguration>,
}

impl RunRegistry {
    /// Register one validated definition under its owner.
    ///
    /// # Errors
    /// Rejects invalid definitions or duplicate owner-scoped IDs.
    pub fn register(&mut self, key: RunId, run: RunConfiguration) -> Result<(), RunError> {
        validate(&run)?;
        if key.id != run.id || key.owner.len() > 256 || key.owner.contains('\0') {
            return Err(RunError("invalid run owner or identity".into()));
        }
        if self.definitions.contains_key(&key) {
            return Err(RunError(format!("duplicate run ID '{}'", key.id)));
        }
        self.definitions.insert(key, run);
        Ok(())
    }

    /// Remove every definition owned by a disabled provider or extension.
    pub fn remove_owner(&mut self, source: &RunSource, owner: &str) {
        self.definitions
            .retain(|key, _| &key.source != source || key.owner != owner);
    }

    /// Remove one definition, for an explicit user edit or deletion.
    pub fn remove(&mut self, key: &RunId) -> Option<RunConfiguration> {
        self.definitions.remove(key)
    }

    /// Look up a definition without granting permission to run it.
    #[must_use]
    pub fn get(&self, key: &RunId) -> Option<&RunConfiguration> {
        self.definitions.get(key)
    }

    /// Enumerate definitions and their owner-scoped identities.
    pub fn list(&self) -> impl Iterator<Item = (&RunId, &RunConfiguration)> {
        self.definitions.iter()
    }
}

/// Read a versioned run file. Missing files contain no definitions.
///
/// # Errors
/// Reports file I/O and validation errors without executing any contents.
pub fn load_run_file(path: &Path) -> Result<RunFile, RunError> {
    match fs::read_to_string(path) {
        Ok(source) => parse_run_file(&source),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(RunFile {
            schema: 1,
            runs: Vec::new(),
        }),
        Err(error) => Err(RunError(format!("{}: {error}", path.display()))),
    }
}

/// Write a validated run file by staging it beside the destination and renaming it.
///
/// # Errors
/// Rejects invalid definitions and reports any I/O failure. A failed write leaves the old file.
pub fn save_run_file(path: &Path, file: &RunFile) -> Result<(), RunError> {
    if file.schema != 1 {
        return Err(RunError(format!("unsupported run schema {}", file.schema)));
    }
    let mut ids = BTreeSet::new();
    for run in &file.runs {
        validate(run)?;
        if !ids.insert(&run.id) {
            return Err(RunError(format!("duplicate run ID '{}'", run.id)));
        }
    }
    let serialized = toml::to_string_pretty(file).map_err(|error| RunError(error.to_string()))?;
    let parent = path
        .parent()
        .ok_or_else(|| RunError("run path has no parent".into()))?;
    fs::create_dir_all(parent).map_err(|error| RunError(error.to_string()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| RunError(error.to_string()))?
        .as_nanos();
    let temporary = parent.join(format!(".runs-{}-{nonce}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        output.write_all(serialized.as_bytes())?;
        output.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|error| RunError(error.to_string()))
}

/// Where execution output is displayed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputDestination {
    /// Bounded task output.
    #[default]
    Run,
    /// Interactive PTY session.
    Terminal,
}

/// What to do when the current file has unsaved changes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavePolicy {
    /// Ask the user before using on-disk file contents.
    #[default]
    Ask,
    /// Save through the document service and its conflict checks.
    Save,
    /// Use on-disk contents after showing an explicit warning.
    Disk,
}

/// A reusable definition; its presence never authorizes execution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunConfiguration {
    /// Stable ID within its owner and source.
    pub id: String,
    /// Display label.
    pub name: String,
    /// Direct executable name or absolute path. Exclusive with `tool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    /// Tool Manager identity. Exclusive with `executable`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Direct argument vector, without shell parsing.
    #[serde(default)]
    pub args: Vec<String>,
    /// Working directory with optional placeholders.
    #[serde(default)]
    pub cwd: String,
    /// Explicit environment overrides.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Applicable language identifiers. Empty means any language.
    #[serde(default)]
    pub languages: Vec<String>,
    /// Output destination.
    #[serde(default)]
    pub output: OutputDestination,
    /// Unsaved-document policy.
    #[serde(default)]
    pub save: SavePolicy,
    /// Optional execution timeout in milliseconds.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Optional debug adapter identity.
    #[serde(default)]
    pub debug_adapter: Option<String>,
}

/// A fully expanded request that must still pass host authorization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedRun {
    /// Executable name or absolute path.
    pub executable: String,
    /// Exact argument vector.
    pub args: Vec<String>,
    /// Absolute working directory.
    pub cwd: PathBuf,
    /// Exact environment overrides.
    pub environment: BTreeMap<String, String>,
}

/// Context supplied by the host at Run time.
pub struct RunContext<'a> {
    /// Active workspace root.
    pub workspace: &'a Path,
    /// Active on-disk document, if one exists.
    pub file: Option<&'a Path>,
}

/// Validation or resolution error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunError(pub String);
impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for RunError {}

/// Parse and validate versioned TOML without executing its contents.
///
/// # Errors
/// Returns an error for malformed content, unsupported versions, or invalid definitions.
pub fn parse_run_file(source: &str) -> Result<RunFile, RunError> {
    let file: RunFile = toml::from_str(source).map_err(|error| RunError(error.to_string()))?;
    if file.schema != 1 {
        return Err(RunError(format!("unsupported run schema {}", file.schema)));
    }
    let mut ids = std::collections::BTreeSet::new();
    for run in &file.runs {
        validate(run)?;
        if !ids.insert(&run.id) {
            return Err(RunError(format!("duplicate run ID '{}'", run.id)));
        }
    }
    Ok(file)
}

/// Validate one definition before registration or persistence.
///
/// # Errors
/// Returns an error for invalid identifiers, commands, or bounds.
pub fn validate(run: &RunConfiguration) -> Result<(), RunError> {
    if run.id.is_empty()
        || run.id.len() > 128
        || !run
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(RunError(
            "run ID must contain 1-128 letters, digits, dots, underscores, or hyphens".into(),
        ));
    }
    if run.name.trim().is_empty() || run.name.len() > 160 {
        return Err(RunError("run name is empty or too long".into()));
    }
    if run.executable.is_some() == run.tool.is_some() {
        return Err(RunError(
            "exactly one of executable or tool is required".into(),
        ));
    }
    if run
        .executable
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.len() > 4096 || value.contains('\0'))
        || run.tool.as_ref().is_some_and(|value| {
            value.trim().is_empty() || value.len() > 256 || value.contains('\0')
        })
    {
        return Err(RunError("executable or tool reference is invalid".into()));
    }
    if run.args.len() > 256
        || run
            .args
            .iter()
            .any(|arg| arg.len() > 16_384 || arg.contains('\0'))
    {
        return Err(RunError("arguments exceed limits or contain NUL".into()));
    }
    if run.cwd.len() > 4096 || run.cwd.contains('\0') {
        return Err(RunError("working directory is invalid".into()));
    }
    if run.environment.len() > 128
        || run.environment.iter().any(|(key, value)| {
            key.is_empty()
                || key.contains(['=', '\0'])
                || value.len() > 16_384
                || value.contains('\0')
        })
    {
        return Err(RunError("environment overrides are invalid".into()));
    }
    if run.timeout_ms == Some(0) {
        return Err(RunError("timeout must be positive".into()));
    }
    if run.languages.len() > 64
        || run
            .languages
            .iter()
            .any(|language| language.is_empty() || language.len() > 128)
    {
        return Err(RunError("language applicability is invalid".into()));
    }
    Ok(())
}

/// Resolve placeholders without invoking a shell. Tool references must first be resolved by the host.
///
/// # Errors
/// Returns an error for missing document context, unknown variables, or invalid paths.
pub fn resolve(run: &RunConfiguration, context: &RunContext<'_>) -> Result<ResolvedRun, RunError> {
    validate(run)?;
    if !context.workspace.is_absolute() {
        return Err(RunError("workspace root must be absolute".into()));
    }
    let executable = expand(
        run.executable
            .as_deref()
            .ok_or_else(|| RunError("tool must be resolved first".into()))?,
        context,
    )?;
    if executable.is_empty() {
        return Err(RunError("executable is empty".into()));
    }
    let args = run
        .args
        .iter()
        .map(|arg| expand(arg, context))
        .collect::<Result<Vec<_>, _>>()?;
    let cwd = if run.cwd.is_empty() {
        context.workspace.to_path_buf()
    } else {
        PathBuf::from(expand(&run.cwd, context)?)
    };
    if !cwd.is_absolute() {
        return Err(RunError(
            "working directory must resolve to an absolute path".into(),
        ));
    }
    let environment = run
        .environment
        .iter()
        .map(|(key, value)| Ok((key.clone(), expand(value, context)?)))
        .collect::<Result<_, RunError>>()?;
    Ok(ResolvedRun {
        executable,
        args,
        cwd,
        environment,
    })
}

/// Parse a readable argument string into a direct argv vector. Quotes group text; backslashes escape the next character.
///
/// # Errors
/// Returns an error for an unmatched quote or trailing escape.
pub fn parse_arguments(input: &str) -> Result<Vec<String>, RunError> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            started = true;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            started = true;
            continue;
        }
        if Some(ch) == quote {
            quote = None;
            continue;
        }
        if quote.is_none() && matches!(ch, '\'' | '"') {
            quote = Some(ch);
            started = true;
            continue;
        }
        if quote.is_none() && ch.is_whitespace() {
            if started {
                args.push(std::mem::take(&mut current));
                started = false;
            }
            continue;
        }
        current.push(ch);
        started = true;
    }
    if escaped || quote.is_some() {
        return Err(RunError("unterminated quote or escape in arguments".into()));
    }
    if started {
        args.push(current);
    }
    Ok(args)
}

fn expand(input: &str, context: &RunContext<'_>) -> Result<String, RunError> {
    let mut result = String::new();
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .ok_or_else(|| RunError("unclosed run variable".into()))?;
        let value = match &tail[..end] {
            "workspace" => context.workspace.to_str(),
            "file" => context.file.and_then(Path::to_str),
            "fileDir" => context.file.and_then(Path::parent).and_then(Path::to_str),
            "fileName" => context
                .file
                .and_then(Path::file_name)
                .and_then(|name| name.to_str()),
            "fileStem" => context
                .file
                .and_then(Path::file_stem)
                .and_then(|name| name.to_str()),
            name => return Err(RunError(format!("unknown run variable '${{{name}}}'"))),
        }
        .ok_or_else(|| {
            RunError(format!(
                "run variable '${{{}}}' requires an active file",
                &tail[..end]
            ))
        })?;
        result.push_str(value);
        rest = &tail[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RunConfiguration {
        RunConfiguration {
            id: "run-lua".into(),
            name: "Run Lua".into(),
            executable: Some("lua5.4".into()),
            tool: None,
            args: vec!["${file}".into(), String::new()],
            cwd: "${workspace}".into(),
            environment: BTreeMap::new(),
            languages: vec!["lua".into()],
            output: OutputDestination::Run,
            save: SavePolicy::Ask,
            timeout_ms: None,
            debug_adapter: None,
        }
    }

    #[test]
    fn file_path_with_spaces_remains_one_argument() {
        let run = sample();
        let context = RunContext {
            workspace: Path::new("/tmp/a b"),
            file: Some(Path::new("/tmp/a b/naïve.lua")),
        };
        let resolved = resolve(&run, &context).unwrap();
        assert_eq!(resolved.args, ["/tmp/a b/naïve.lua", ""]);
    }

    #[test]
    fn rejects_unknown_or_missing_variables() {
        let mut run = sample();
        run.args = vec!["${oops}".into()];
        assert!(
            resolve(
                &run,
                &RunContext {
                    workspace: Path::new("/tmp"),
                    file: None
                }
            )
            .is_err()
        );
        run.args = vec!["${file}".into()];
        assert!(
            resolve(
                &run,
                &RunContext {
                    workspace: Path::new("/tmp"),
                    file: None
                }
            )
            .is_err()
        );
    }

    #[test]
    fn parses_quoted_and_empty_arguments() {
        assert_eq!(
            parse_arguments("one 'two words' \"\" three\\ four").unwrap(),
            ["one", "two words", "", "three four"]
        );
        assert!(parse_arguments("'unfinished").is_err());
    }

    #[test]
    fn rejects_duplicate_ids_and_ambiguous_executable() {
        let mut run = sample();
        run.tool = Some("lua".into());
        assert!(validate(&run).is_err());
        let source = "schema = 1\n[[run]]\nid = 'a'\nname = 'A'\nexecutable = 'lua'\n[[run]]\nid = 'a'\nname = 'B'\nexecutable = 'lua'\n";
        assert!(parse_run_file(source).is_err());
    }

    #[test]
    fn registry_scopes_ids_and_cleans_up_owner() {
        let mut registry = RunRegistry::default();
        let first = RunId {
            source: RunSource::Extension("one".into()),
            owner: "one".into(),
            id: "run-lua".into(),
        };
        let second = RunId {
            source: RunSource::Extension("two".into()),
            owner: "two".into(),
            id: "run-lua".into(),
        };
        registry.register(first.clone(), sample()).unwrap();
        registry.register(second.clone(), sample()).unwrap();
        assert!(registry.register(first.clone(), sample()).is_err());
        registry.remove_owner(&RunSource::Extension("one".into()), "one");
        assert!(registry.get(&first).is_none());
        assert!(registry.get(&second).is_some());
    }

    #[test]
    fn run_files_round_trip_and_reject_invalid_update() {
        let directory = std::env::temp_dir().join(format!(
            "koren-run-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join(".koren/runs.toml");
        let missing = load_run_file(&path).unwrap();
        assert_eq!(missing.runs.len(), 0);
        let file = RunFile {
            schema: 1,
            runs: vec![sample()],
        };
        save_run_file(&path, &file).unwrap();
        assert_eq!(load_run_file(&path).unwrap().runs[0].id, "run-lua");
        let invalid = RunFile {
            schema: 1,
            runs: vec![sample(), sample()],
        };
        assert!(save_run_file(&path, &invalid).is_err());
        assert_eq!(load_run_file(&path).unwrap().runs.len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
