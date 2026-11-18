//! First-party Cargo project discovery and task definitions.
//!
//! Discovery reads bounded `Cargo.toml` files only. It never invokes Cargo, resolves
//! dependencies, executes build scripts, or accesses the network. The Cargo metadata
//! command is exposed as a normal task and therefore uses the host's task authorization flow.

use glob::Pattern;
use koren_projects::{
    DiscoveryLimits, ProjectError, ProjectId, ProjectInfo, ProjectManager, ProjectProvider,
    ProjectSpec, ProjectState, ProviderId,
};
use koren_tasks::{TaskCategory, TaskDefinition, TaskSnapshot, TaskState};
use koren_workspace::{EntryKind, WorkspaceError, WorkspaceManager};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt, fs,
    path::{Component, Path, PathBuf},
};

/// Stable provider identifier used for registered Cargo projects.
pub const CARGO_PROVIDER_ID: &str = "org.koren.cargo";
const MANIFEST_LIMIT_BYTES: u64 = 1024 * 1024;
const PROVIDER_DATA_MEMBERS: &str = "cargo.members";
const PROVIDER_DATA_MANIFEST: &str = "cargo.manifest";

/// Cargo target kind exposed by manifest inspection or Cargo metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CargoTargetKind {
    /// Library or proc-macro target.
    Library,
    /// Executable binary target.
    Binary,
    /// Example target.
    Example,
    /// Integration-test target.
    Test,
    /// Benchmark target.
    Benchmark,
    /// Target kind introduced by Cargo or a plugin not represented above.
    Other,
}

/// One package target, with a package-relative source path when it is inside the package root.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct CargoTarget {
    /// Cargo target name.
    pub name: String,
    /// Normalized target kind.
    pub kind: CargoTargetKind,
    /// Package-relative Rust source path when known.
    pub source_path: Option<PathBuf>,
}

/// Package as discovered from a local manifest or Cargo metadata output.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct CargoPackageMetadata {
    /// Package name.
    pub name: String,
    /// Package version when present in the manifest.
    pub version: Option<String>,
    /// Workspace-relative manifest path.
    pub manifest_path: PathBuf,
    /// Declared build targets.
    pub targets: Vec<CargoTarget>,
}

/// Cargo-specific information for one logical Koren project.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct CargoProjectMetadata {
    /// Workspace-relative project root.
    pub root: PathBuf,
    /// Workspace-relative manifest path.
    pub manifest_path: PathBuf,
    /// Whether this manifest declares `[workspace]`.
    pub is_workspace: bool,
    /// Package details for the root manifest, when it declares `[package]`.
    pub package: Option<CargoPackageMetadata>,
    /// Member packages represented by this workspace project.
    pub members: Vec<CargoPackageMetadata>,
}

/// An explicitly discovered Cargo project and its manager-local project identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CargoDiscoveredProject {
    /// Koren project identity.
    pub project_id: ProjectId,
    /// Cargo manifest and package information.
    pub metadata: CargoProjectMetadata,
}

/// Structured output from `cargo metadata --format-version 1 --no-deps`.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct CargoMetadata {
    /// Cargo workspace root path as reported by Cargo.
    pub workspace_root: PathBuf,
    /// Package identifiers belonging to the workspace.
    pub workspace_members: Vec<String>,
    /// Packages returned by Cargo.
    pub packages: Vec<CargoMetadataPackage>,
}

/// A package reported by Cargo's stable metadata JSON interface.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct CargoMetadataPackage {
    /// Cargo's opaque package identifier.
    pub id: String,
    /// Package name.
    pub name: String,
    /// Package version.
    pub version: String,
    /// Absolute manifest path.
    pub manifest_path: PathBuf,
    /// Package targets.
    pub targets: Vec<CargoMetadataTarget>,
}

/// A target reported by Cargo's stable metadata JSON interface.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct CargoMetadataTarget {
    /// Target name.
    pub name: String,
    /// Cargo target kinds, such as `bin`, `lib`, `test`, or `example`.
    pub kind: Vec<String>,
    /// Target source path.
    pub src_path: PathBuf,
}

/// Cargo provider failures.
#[derive(Debug)]
pub enum CargoError {
    /// Workspace or project manager failed.
    Project(ProjectError),
    /// Workspace path validation or traversal failed.
    Workspace(WorkspaceError),
    /// Manifest was too large, invalid TOML, or inconsistent with its workspace.
    InvalidManifest {
        /// Workspace-relative manifest path, when available.
        path: PathBuf,
        /// Parse or validation explanation.
        message: String,
    },
    /// Cargo metadata process did not produce usable offline metadata.
    MetadataUnavailable(String),
    /// Cargo metadata output was malformed or incompatible.
    InvalidMetadata(String),
}
impl fmt::Display for CargoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Project(error) => write!(f, "{error}"),
            Self::Workspace(error) => write!(f, "{error}"),
            Self::InvalidManifest { path, message } => {
                write!(f, "invalid Cargo manifest '{}': {message}", path.display())
            }
            Self::MetadataUnavailable(message) => {
                write!(f, "Cargo metadata unavailable offline: {message}")
            }
            Self::InvalidMetadata(message) => write!(f, "invalid Cargo metadata: {message}"),
        }
    }
}
impl std::error::Error for CargoError {}
impl From<ProjectError> for CargoError {
    fn from(value: ProjectError) -> Self {
        Self::Project(value)
    }
}
impl From<WorkspaceError> for CargoError {
    fn from(value: WorkspaceError) -> Self {
        Self::Workspace(value)
    }
}

/// Language-specific Cargo project provider.
#[derive(Clone, Debug)]
pub struct CargoProvider {
    id: ProviderId,
}
impl Default for CargoProvider {
    fn default() -> Self {
        Self::new()
    }
}
impl CargoProvider {
    /// Creates the first-party Cargo provider.
    ///
    /// # Panics
    ///
    /// Panics only if the provider's compile-time identifier constant is changed to an invalid
    /// empty value.
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: ProviderId::new(CARGO_PROVIDER_ID).expect("constant provider id is valid"),
        }
    }

    /// Registers this provider with the project manager so its task capabilities are available.
    ///
    /// # Errors
    ///
    /// Returns an error if a provider with this identifier is already registered.
    pub fn register(&self, projects: &mut ProjectManager) -> Result<(), ProjectError> {
        projects.register_provider(Box::new(self.clone()))
    }

    /// Explicitly discovers Cargo roots and workspace member packages within bounded limits.
    ///
    /// The workspace root manifest is one logical Koren project. Workspace packages are
    /// represented as typed member metadata on that project, avoiding duplicate projects for
    /// each `Cargo.toml`. A nested manifest with its own workspace is a separate project.
    /// Discovery reads manifests but never runs Cargo or project commands.
    ///
    /// # Errors
    ///
    /// Returns an error for traversal limits, invalid manifests, inconsistent workspace
    /// membership, duplicate registrations, or workspace/project manager failures.
    pub fn discover(
        &self,
        workspace: &WorkspaceManager,
        projects: &mut ProjectManager,
        limits: DiscoveryLimits,
    ) -> Result<Vec<CargoDiscoveredProject>, CargoError> {
        let ids = projects.discover(workspace, self, limits)?;
        ids.into_iter()
            .map(|project_id| {
                let info = projects.info(project_id)?;
                Ok(CargoDiscoveredProject {
                    project_id,
                    metadata: self.project_metadata(info)?,
                })
            })
            .collect()
    }

    /// Returns the explicit, offline metadata task for a discovered project.
    ///
    /// The task uses the generic task registry and must pass through the host's normal
    /// execution authorization flow. Merely discovering a project does not launch Cargo.
    #[must_use]
    pub fn metadata_task(&self, project: &ProjectInfo) -> TaskDefinition {
        let manifest = project.configuration_path.as_ref().map_or_else(
            || {
                project
                    .workspace_root
                    .join(&project.root)
                    .join("Cargo.toml")
            },
            |path| project.workspace_root.join(path),
        );
        cargo_task(
            format!("cargo.project.{}.metadata", project.id.get()),
            "Cargo offline metadata",
            TaskCategory::Custom,
            &project.workspace_root,
            &manifest,
            vec![
                "metadata".into(),
                "--offline".into(),
                "--format-version".into(),
                "1".into(),
                "--no-deps".into(),
            ],
            Some(project.id.get()),
        )
    }

    /// Parses successful task output from Cargo's JSON metadata interface.
    ///
    /// # Errors
    ///
    /// Returns a useful offline failure for spawn errors/non-zero exits, or a structured
    /// parse error when Cargo's output is not one valid metadata JSON document.
    pub fn parse_metadata_output(snapshot: &TaskSnapshot) -> Result<CargoMetadata, CargoError> {
        match &snapshot.state {
            TaskState::Succeeded { .. } => serde_json::from_str(&snapshot.stdout)
                .map_err(|error| CargoError::InvalidMetadata(error.to_string())),
            TaskState::SpawnFailed { message } => Err(CargoError::MetadataUnavailable(format!(
                "could not start Cargo: {message}"
            ))),
            TaskState::Failed { exit_code } => Err(CargoError::MetadataUnavailable(format!(
                "Cargo exited with status {exit_code:?}: {}",
                snapshot.stderr.trim()
            ))),
            state => Err(CargoError::MetadataUnavailable(format!(
                "metadata task is not complete: {state:?}"
            ))),
        }
    }

    /// Generates workspace- and package-scoped Cargo tasks from an inspected project.
    #[must_use]
    pub fn tasks_for_project(&self, project: &ProjectInfo) -> Vec<TaskDefinition> {
        <Self as ProjectProvider>::task_definitions(self, project)
    }

    /// Reads typed, manifest-derived metadata stored during discovery.
    ///
    /// # Errors
    ///
    /// Returns an error if the project belongs to another provider or metadata is unavailable.
    pub fn project_metadata(
        &self,
        project: &ProjectInfo,
    ) -> Result<CargoProjectMetadata, CargoError> {
        if project.provider_id != self.id {
            return Err(CargoError::InvalidMetadata(
                "project belongs to a different provider".into(),
            ));
        }
        project
            .provider_data
            .get("cargo.metadata")
            .ok_or_else(|| CargoError::InvalidMetadata("project metadata is unavailable".into()))
            .and_then(|json| {
                serde_json::from_str(json)
                    .map_err(|error| CargoError::InvalidMetadata(error.to_string()))
            })
    }

    /// Enriches manifest-derived project data from an explicitly authorized Cargo metadata task.
    ///
    /// The metadata workspace root must match the logical project's root. Package targets and
    /// versions are taken from Cargo's structured JSON response rather than human-readable output.
    ///
    /// # Errors
    ///
    /// Returns an error if metadata belongs to another project or contains paths outside the
    /// opened workspace.
    pub fn enrich_project_metadata(
        &self,
        project: &ProjectInfo,
        metadata: &CargoMetadata,
    ) -> Result<CargoProjectMetadata, CargoError> {
        let mut project_metadata = self.project_metadata(project)?;
        let expected_root = project.workspace_root.join(&project.root);
        if metadata.workspace_root != expected_root {
            return Err(CargoError::InvalidMetadata(
                "Cargo metadata workspace root does not match the selected project".into(),
            ));
        }
        let workspace_members = metadata.workspace_members.iter().collect::<BTreeSet<_>>();
        let root_manifest = project.workspace_root.join(&project_metadata.manifest_path);
        let mut packages = metadata
            .packages
            .iter()
            .filter(|package| workspace_members.contains(&package.id))
            .map(|package| metadata_package(project.workspace_root.as_path(), package))
            .collect::<Result<Vec<_>, _>>()?;
        packages.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then(a.manifest_path.cmp(&b.manifest_path))
        });
        let root_package = packages.iter().position(|package| {
            project.workspace_root.join(&package.manifest_path) == root_manifest
        });
        project_metadata.package = root_package.map(|index| packages.remove(index));
        project_metadata.members = packages;
        Ok(project_metadata)
    }

    fn project_spec(&self, metadata: &CargoProjectMetadata) -> Result<ProjectSpec, CargoError> {
        let mut provider_data = BTreeMap::new();
        provider_data.insert(
            PROVIDER_DATA_MANIFEST.into(),
            metadata.manifest_path.to_string_lossy().into_owned(),
        );
        provider_data.insert(
            PROVIDER_DATA_MEMBERS.into(),
            serde_json::to_string(&metadata.members)
                .map_err(|error| CargoError::InvalidMetadata(error.to_string()))?,
        );
        provider_data.insert(
            "cargo.metadata".into(),
            serde_json::to_string(&metadata)
                .map_err(|error| CargoError::InvalidMetadata(error.to_string()))?,
        );
        if let Some(package) = &metadata.package {
            provider_data.insert("cargo.package_name".into(), package.name.clone());
            if let Some(version) = &package.version {
                provider_data.insert("cargo.package_version".into(), version.clone());
            }
        }
        provider_data.insert(
            "cargo.is_workspace".into(),
            metadata.is_workspace.to_string(),
        );
        Ok(ProjectSpec {
            display_name: metadata
                .package
                .as_ref()
                .map(|package| package.name.clone())
                .or_else(|| {
                    metadata
                        .root
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "Cargo Workspace".into()),
            root: metadata.root.clone(),
            provider_id: self.id.clone(),
            configuration_path: Some(metadata.manifest_path.clone()),
            provider_data,
        })
    }
}

impl ProjectProvider for CargoProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn detect(
        &self,
        directory: &Path,
        entries: &[koren_workspace::WorkspaceEntry],
    ) -> Option<ProjectSpec> {
        let manifest = entries
            .iter()
            .find(|entry| entry.name == "Cargo.toml" && entry.kind == EntryKind::File)?;
        Some(ProjectSpec {
            display_name: directory.file_name().map_or_else(
                || "Cargo Project".into(),
                |name| name.to_string_lossy().into_owned(),
            ),
            root: directory.to_path_buf(),
            provider_id: self.id.clone(),
            configuration_path: Some(manifest.path.clone()),
            provider_data: BTreeMap::new(),
        })
    }

    fn discover_candidates(
        &self,
        workspace: &WorkspaceManager,
        limits: DiscoveryLimits,
    ) -> Result<Vec<ProjectSpec>, ProjectError> {
        collect_project_metadata(workspace, limits)
            .map_err(|error| ProjectError::Invalid(error.to_string()))?
            .into_iter()
            .map(|metadata| {
                self.project_spec(&metadata)
                    .map_err(|error| ProjectError::Invalid(error.to_string()))
            })
            .collect()
    }

    fn task_definitions(&self, project: &ProjectInfo) -> Vec<TaskDefinition> {
        if project.provider_id != self.id || project.state != ProjectState::Registered {
            return Vec::new();
        }
        let manifest = project.configuration_path.as_ref().map_or_else(
            || {
                project
                    .workspace_root
                    .join(&project.root)
                    .join("Cargo.toml")
            },
            |path| project.workspace_root.join(path),
        );
        let is_workspace = project
            .provider_data
            .get("cargo.is_workspace")
            .is_some_and(|value| value == "true");
        let members = project
            .provider_data
            .get(PROVIDER_DATA_MEMBERS)
            .and_then(|json| serde_json::from_str::<Vec<CargoPackageMetadata>>(json).ok())
            .unwrap_or_default();
        let mut tasks = Vec::new();
        if is_workspace {
            for (command, category, title) in cargo_commands() {
                tasks.push(cargo_task(
                    format!("cargo.workspace.{command}"),
                    &format!("Cargo workspace {title}"),
                    category,
                    &project.workspace_root,
                    &manifest,
                    vec![command.into(), "--workspace".into()],
                    Some(project.id.get()),
                ));
            }
            if let Some(name) = project.provider_data.get("cargo.package_name") {
                let targets = read_project_targets(project, name);
                tasks.extend(package_tasks(project, &manifest, name, &targets));
            }
            for package in &members {
                tasks.extend(package_tasks(
                    project,
                    &manifest,
                    &package.name,
                    &package.targets,
                ));
            }
        } else if let Some(name) = project.provider_data.get("cargo.package_name") {
            let targets = read_project_targets(project, name);
            tasks.extend(package_tasks(project, &manifest, name, &targets));
        }
        tasks.push(self.metadata_task(project));
        tasks
    }
}

fn cargo_commands() -> [(&'static str, TaskCategory, &'static str); 5] {
    [
        ("build", TaskCategory::Build, "build"),
        ("check", TaskCategory::Check, "check"),
        ("test", TaskCategory::Test, "test"),
        ("clippy", TaskCategory::Check, "clippy"),
        ("clean", TaskCategory::Clean, "clean"),
    ]
}

fn package_tasks(
    project: &ProjectInfo,
    manifest: &Path,
    package_name: &str,
    targets: &[CargoTarget],
) -> Vec<TaskDefinition> {
    let mut tasks = cargo_commands()
        .into_iter()
        .map(|(command, category, title)| {
            cargo_task(
                format!("cargo.package.{package_name}.{command}"),
                &format!("Cargo {title} {package_name}"),
                category,
                &project.workspace_root,
                manifest,
                vec![command.into(), "--package".into(), package_name.into()],
                Some(project.id.get()),
            )
        })
        .collect::<Vec<_>>();
    for target in targets
        .iter()
        .filter(|target| target.kind == CargoTargetKind::Binary)
    {
        tasks.push(cargo_task(
            format!("cargo.package.{package_name}.run.{}", target.name),
            &format!("Cargo run {} ({})", target.name, package_name),
            TaskCategory::Run,
            &project.workspace_root,
            manifest,
            vec![
                "run".into(),
                "--package".into(),
                package_name.into(),
                "--bin".into(),
                target.name.clone(),
            ],
            Some(project.id.get()),
        ));
    }
    tasks
}

fn cargo_task(
    key: String,
    display_name: &str,
    category: TaskCategory,
    working_directory: &Path,
    manifest: &Path,
    mut arguments: Vec<String>,
    project_id: Option<u64>,
) -> TaskDefinition {
    arguments.extend([
        "--manifest-path".into(),
        manifest.to_string_lossy().into_owned(),
    ]);
    TaskDefinition {
        key,
        display_name: display_name.into(),
        category,
        executable: "cargo".into(),
        arguments,
        working_directory: working_directory.to_path_buf(),
        environment: BTreeMap::new(),
        timeout: None,
        project_id,
    }
}

fn read_project_targets(project: &ProjectInfo, package_name: &str) -> Vec<CargoTarget> {
    let manifest = project
        .configuration_path
        .as_ref()
        .map(|path| project.workspace_root.join(path));
    manifest
        .and_then(|path| {
            let root = path.parent()?.to_path_buf();
            let source = fs::read_to_string(path).ok()?;
            Some((source, root))
        })
        .and_then(|(source, root)| {
            toml::from_str::<toml::Value>(&source)
                .ok()
                .map(|document| (document, root))
        })
        .map(|(document, root)| manifest_targets(&document, package_name, &root))
        .unwrap_or_default()
}

fn collect_project_metadata(
    workspace: &WorkspaceManager,
    limits: DiscoveryLimits,
) -> Result<Vec<CargoProjectMetadata>, CargoError> {
    let manifests = collect_manifests(workspace, limits)?;
    let roots = manifests
        .iter()
        .filter(|manifest| manifest.summary.is_workspace)
        .map(|manifest| manifest.directory.clone())
        .collect::<BTreeSet<_>>();
    let mut grouped = BTreeMap::<PathBuf, Vec<CargoPackageMetadata>>::new();
    let mut standalone = Vec::new();
    for manifest in &manifests {
        if manifest.summary.is_workspace {
            continue;
        }
        let owner = owning_workspace(manifest, &manifests, &roots)?;
        if let Some(root) = owner {
            grouped
                .entry(root)
                .or_default()
                .push(package_metadata(manifest));
        } else if manifest.summary.package_name.is_some() {
            standalone.push(manifest);
        }
    }
    let mut discovered = Vec::new();
    for manifest in manifests.iter().filter(|item| item.summary.is_workspace) {
        if discovered.len() >= limits.max_projects {
            return Err(CargoError::InvalidManifest {
                path: manifest.relative_manifest.clone(),
                message: "Cargo project result limit exceeded".into(),
            });
        }
        let mut members = grouped.remove(&manifest.directory).unwrap_or_default();
        members.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then(a.manifest_path.cmp(&b.manifest_path))
        });
        discovered.push(project_metadata(manifest, members));
    }
    for manifest in standalone {
        if discovered.len() >= limits.max_projects {
            return Err(CargoError::InvalidManifest {
                path: manifest.relative_manifest.clone(),
                message: "Cargo project result limit exceeded".into(),
            });
        }
        discovered.push(project_metadata(manifest, Vec::new()));
    }
    Ok(discovered)
}

#[derive(Clone)]
struct ManifestFile {
    directory: PathBuf,
    relative_manifest: PathBuf,
    summary: ManifestSummary,
}

#[derive(Clone, Default)]
struct ManifestSummary {
    is_workspace: bool,
    package_name: Option<String>,
    version: Option<String>,
    workspace_members: Vec<String>,
    workspace_exclude: Vec<String>,
    package_workspace: Option<PathBuf>,
    targets: Vec<CargoTarget>,
}

fn collect_manifests(
    workspace: &WorkspaceManager,
    limits: DiscoveryLimits,
) -> Result<Vec<ManifestFile>, CargoError> {
    if limits.max_directories == 0 || limits.max_projects == 0 {
        return Err(CargoError::InvalidManifest {
            path: PathBuf::new(),
            message: "discovery limits must be greater than zero".into(),
        });
    }
    let mut queue = VecDeque::from([(PathBuf::new(), 0usize)]);
    let mut inspected = 0usize;
    let mut manifests = Vec::new();
    while let Some((directory, depth)) = queue.pop_front() {
        if inspected >= limits.max_directories {
            return Err(CargoError::InvalidManifest {
                path: directory,
                message: "Cargo discovery directory limit exceeded".into(),
            });
        }
        inspected += 1;
        let entries = if directory.as_os_str().is_empty() {
            workspace.entries()?
        } else {
            workspace.list_dir(&directory)?
        };
        if entries
            .iter()
            .any(|entry| entry.name == "Cargo.toml" && entry.kind == EntryKind::File)
        {
            let relative_manifest = directory.join("Cargo.toml");
            let absolute = workspace.resolve_file(&relative_manifest)?;
            let summary = parse_manifest(&absolute, &relative_manifest)?;
            if summary.package_name.is_some() || summary.is_workspace {
                manifests.push(ManifestFile {
                    directory: directory.clone(),
                    relative_manifest,
                    summary,
                });
            }
        }
        if depth < limits.max_depth {
            for entry in entries
                .into_iter()
                .filter(|entry| entry.kind == EntryKind::Directory)
            {
                if inspected + queue.len() >= limits.max_directories {
                    return Err(CargoError::InvalidManifest {
                        path: directory,
                        message: "Cargo discovery directory limit exceeded".into(),
                    });
                }
                queue.push_back((entry.path, depth + 1));
            }
        }
    }
    Ok(manifests)
}

fn parse_manifest(absolute: &Path, relative: &Path) -> Result<ManifestSummary, CargoError> {
    let metadata = fs::metadata(absolute).map_err(|error| CargoError::InvalidManifest {
        path: relative.to_path_buf(),
        message: error.to_string(),
    })?;
    if metadata.len() > MANIFEST_LIMIT_BYTES {
        return Err(CargoError::InvalidManifest {
            path: relative.to_path_buf(),
            message: format!("manifest exceeds {MANIFEST_LIMIT_BYTES} bytes"),
        });
    }
    let source = fs::read_to_string(absolute).map_err(|error| CargoError::InvalidManifest {
        path: relative.to_path_buf(),
        message: error.to_string(),
    })?;
    let document =
        toml::from_str::<toml::Value>(&source).map_err(|error| CargoError::InvalidManifest {
            path: relative.to_path_buf(),
            message: error.to_string(),
        })?;
    let package = document.get("package").and_then(toml::Value::as_table);
    let workspace = document.get("workspace").and_then(toml::Value::as_table);
    let package_name = package
        .and_then(|table| table.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned);
    let version = package
        .and_then(|table| table.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned);
    let package_workspace = package
        .and_then(|table| table.get("workspace"))
        .and_then(toml::Value::as_str)
        .map(PathBuf::from);
    let workspace_members = workspace
        .and_then(|table| table.get("members"))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .map(str::to_owned)
        .collect();
    let workspace_exclude = workspace
        .and_then(|table| table.get("exclude"))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .map(str::to_owned)
        .collect();
    let package_root = absolute.parent().unwrap_or(Path::new("."));
    let targets = manifest_targets(
        &document,
        package_name.as_deref().unwrap_or(""),
        package_root,
    );
    Ok(ManifestSummary {
        is_workspace: workspace.is_some(),
        package_name,
        version,
        workspace_members,
        workspace_exclude,
        package_workspace,
        targets,
    })
}

fn manifest_targets(
    document: &toml::Value,
    package_name: &str,
    package_root: &Path,
) -> Vec<CargoTarget> {
    let mut targets = Vec::new();
    let explicit = [
        ("lib", CargoTargetKind::Library),
        ("bin", CargoTargetKind::Binary),
        ("example", CargoTargetKind::Example),
        ("test", CargoTargetKind::Test),
        ("bench", CargoTargetKind::Benchmark),
    ];
    for (section, kind) in explicit {
        if let Some(value) = document.get(section) {
            let items = value
                .as_array()
                .map_or_else(|| vec![value], |array| array.iter().collect());
            for item in items {
                if let Some(table) = item.as_table() {
                    let path = table
                        .get("path")
                        .and_then(toml::Value::as_str)
                        .map(PathBuf::from);
                    let name = table
                        .get("name")
                        .and_then(toml::Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| {
                            path.as_ref()
                                .and_then(|path| path.file_stem())
                                .map(|stem| stem.to_string_lossy().into_owned())
                        })
                        .unwrap_or_else(|| package_name.to_owned());
                    targets.push(CargoTarget {
                        name,
                        kind,
                        source_path: path,
                    });
                }
            }
        }
    }
    if targets
        .iter()
        .all(|target| target.kind != CargoTargetKind::Library)
        && package_root.join("src/lib.rs").is_file()
    {
        targets.push(CargoTarget {
            name: package_name.replace('-', "_"),
            kind: CargoTargetKind::Library,
            source_path: Some(PathBuf::from("src/lib.rs")),
        });
    }
    if targets
        .iter()
        .all(|target| target.kind != CargoTargetKind::Binary)
        && package_root.join("src/main.rs").is_file()
    {
        targets.push(CargoTarget {
            name: package_name.to_owned(),
            kind: CargoTargetKind::Binary,
            source_path: Some(PathBuf::from("src/main.rs")),
        });
    }
    if let Ok(entries) = fs::read_dir(package_root.join("src/bin")) {
        for entry in entries.flatten().take(1024) {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "rs") {
                if let Some(name) = path.file_stem() {
                    targets.push(CargoTarget {
                        name: name.to_string_lossy().into_owned(),
                        kind: CargoTargetKind::Binary,
                        source_path: path.strip_prefix(package_root).ok().map(Path::to_path_buf),
                    });
                }
            }
        }
    }
    targets
}

fn package_metadata(manifest: &ManifestFile) -> CargoPackageMetadata {
    CargoPackageMetadata {
        name: manifest.summary.package_name.clone().unwrap_or_default(),
        version: manifest.summary.version.clone(),
        manifest_path: manifest.relative_manifest.clone(),
        targets: manifest.summary.targets.clone(),
    }
}

fn project_metadata(
    manifest: &ManifestFile,
    members: Vec<CargoPackageMetadata>,
) -> CargoProjectMetadata {
    CargoProjectMetadata {
        root: manifest.directory.clone(),
        manifest_path: manifest.relative_manifest.clone(),
        is_workspace: manifest.summary.is_workspace,
        package: manifest
            .summary
            .package_name
            .as_ref()
            .map(|_| package_metadata(manifest)),
        members,
    }
}

fn owning_workspace(
    manifest: &ManifestFile,
    manifests: &[ManifestFile],
    roots: &BTreeSet<PathBuf>,
) -> Result<Option<PathBuf>, CargoError> {
    if let Some(explicit) = &manifest.summary.package_workspace {
        let root = normalize_join(&manifest.directory, explicit).ok_or_else(|| {
            CargoError::InvalidManifest {
                path: manifest.relative_manifest.clone(),
                message: "package.workspace escapes the opened workspace".into(),
            }
        })?;
        if !roots.contains(&root) {
            return Err(CargoError::InvalidManifest {
                path: manifest.relative_manifest.clone(),
                message: "package.workspace does not identify a discovered [workspace] manifest"
                    .into(),
            });
        }
        return if workspace_includes(&root, &manifest.directory, manifests, roots)? {
            Ok(Some(root))
        } else {
            Ok(None)
        };
    }
    let nearest = roots
        .iter()
        .filter(|root| manifest.directory.starts_with(root.as_path()))
        .max_by_key(|root| root.components().count());
    let Some(root) = nearest else { return Ok(None) };
    if workspace_includes(root, &manifest.directory, manifests, roots)? {
        Ok(Some(root.clone()))
    } else {
        Ok(None)
    }
}

fn workspace_includes(
    root: &Path,
    package_directory: &Path,
    manifests: &[ManifestFile],
    _roots: &BTreeSet<PathBuf>,
) -> Result<bool, CargoError> {
    let workspace = manifests
        .iter()
        .find(|manifest| manifest.directory == root)
        .ok_or_else(|| CargoError::InvalidManifest {
            path: root.to_path_buf(),
            message: "workspace root manifest is unavailable".into(),
        })?;
    let relative = package_directory
        .strip_prefix(root)
        .unwrap_or(package_directory);
    for pattern in &workspace.summary.workspace_exclude {
        if pattern_matches(pattern, relative, &workspace.relative_manifest)? {
            return Ok(false);
        }
    }
    for pattern in &workspace.summary.workspace_members {
        if pattern_matches(pattern, relative, &workspace.relative_manifest)? {
            return Ok(true);
        }
    }
    Ok(relative.as_os_str().is_empty() && workspace.summary.package_name.is_some())
}

fn pattern_matches(pattern: &str, path: &Path, manifest: &Path) -> Result<bool, CargoError> {
    let matcher = Pattern::new(pattern).map_err(|error| CargoError::InvalidManifest {
        path: manifest.to_path_buf(),
        message: format!("invalid workspace member glob '{pattern}': {error}"),
    })?;
    Ok(matcher.matches_path(path))
}

fn normalize_join(base: &Path, child: &Path) -> Option<PathBuf> {
    let mut result = base.to_path_buf();
    for component in child.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => result.push(value),
            Component::ParentDir => {
                result.pop();
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(result)
}

/// Converts a Cargo metadata output path to package target kinds without exposing Cargo internals.
#[must_use]
pub fn normalize_target_kind(kinds: &[String]) -> CargoTargetKind {
    if kinds.iter().any(|kind| kind == "bin") {
        CargoTargetKind::Binary
    } else if kinds.iter().any(|kind| {
        kind == "lib"
            || kind == "proc-macro"
            || kind == "cdylib"
            || kind == "staticlib"
            || kind == "rlib"
    }) {
        CargoTargetKind::Library
    } else if kinds.iter().any(|kind| kind == "example") {
        CargoTargetKind::Example
    } else if kinds.iter().any(|kind| kind == "test") {
        CargoTargetKind::Test
    } else if kinds.iter().any(|kind| kind == "bench") {
        CargoTargetKind::Benchmark
    } else {
        CargoTargetKind::Other
    }
}

/// Maps Cargo metadata JSON into the stable platform-facing target model.
#[must_use]
pub fn metadata_targets(package: &CargoMetadataPackage) -> Vec<CargoTarget> {
    let package_root = package.manifest_path.parent();
    package
        .targets
        .iter()
        .map(|target| CargoTarget {
            name: target.name.clone(),
            kind: normalize_target_kind(&target.kind),
            source_path: Some(
                package_root
                    .and_then(|root| target.src_path.strip_prefix(root).ok())
                    .unwrap_or(&target.src_path)
                    .to_path_buf(),
            ),
        })
        .collect()
}

fn metadata_package(
    workspace_root: &Path,
    package: &CargoMetadataPackage,
) -> Result<CargoPackageMetadata, CargoError> {
    let manifest_path = package
        .manifest_path
        .strip_prefix(workspace_root)
        .map_err(|_| {
            CargoError::InvalidMetadata(format!(
                "package manifest '{}' is outside the opened workspace",
                package.manifest_path.display()
            ))
        })?
        .to_path_buf();
    Ok(CargoPackageMetadata {
        name: package.name.clone(),
        version: Some(package.version.clone()),
        manifest_path,
        targets: metadata_targets(package),
    })
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
                "koren-cargo-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn workspace(&self) -> WorkspaceManager {
            let mut workspace = WorkspaceManager::new();
            workspace.open(&self.0).unwrap();
            workspace
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn package_manifest(name: &str, extra: &str) -> String {
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{extra}")
    }

    #[test]
    fn discovers_single_package_without_running_cargo_and_generates_tasks() {
        let temp = Temp::new();
        fs::write(temp.0.join("Cargo.toml"), package_manifest("solo", "")).unwrap();
        fs::create_dir_all(temp.0.join("src")).unwrap();
        fs::write(temp.0.join("src/main.rs"), "fn main() {}\n").unwrap();
        let workspace = temp.workspace();
        let mut projects = ProjectManager::new();
        let provider = CargoProvider::new();
        provider.register(&mut projects).unwrap();
        let found = provider
            .discover(&workspace, &mut projects, DiscoveryLimits::default())
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].metadata.package.as_ref().unwrap().name, "solo");
        let info = projects.info(found[0].project_id).unwrap();
        let tasks = projects.provider_tasks(&provider.id, info).unwrap();
        assert!(
            tasks
                .iter()
                .any(|task| task.category == TaskCategory::Build)
        );
        assert!(
            tasks
                .iter()
                .any(|task| task.category == TaskCategory::Check)
        );
        assert!(tasks.iter().any(|task| task.category == TaskCategory::Test));
        assert!(
            tasks
                .iter()
                .any(|task| task.category == TaskCategory::Clean)
        );
        assert!(tasks.iter().any(|task| task.category == TaskCategory::Run));
        assert!(tasks.iter().any(|task| task.key.ends_with("metadata")));
        assert_eq!(
            tasks
                .iter()
                .find(|task| task.category == TaskCategory::Run)
                .unwrap()
                .executable,
            PathBuf::from("cargo")
        );
        let cargo_metadata = CargoMetadata {
            workspace_root: temp.0.clone(),
            workspace_members: vec!["solo 0.1.0".into()],
            packages: vec![CargoMetadataPackage {
                id: "solo 0.1.0".into(),
                name: "solo".into(),
                version: "0.1.0".into(),
                manifest_path: temp.0.join("Cargo.toml"),
                targets: vec![CargoMetadataTarget {
                    name: "solo".into(),
                    kind: vec!["bin".into()],
                    src_path: temp.0.join("src/main.rs"),
                }],
            }],
        };
        let enriched = provider
            .enrich_project_metadata(info, &cargo_metadata)
            .unwrap();
        assert_eq!(
            enriched.package.as_ref().unwrap().version.as_deref(),
            Some("0.1.0")
        );
        assert_eq!(
            enriched.package.as_ref().unwrap().targets[0].source_path,
            Some(PathBuf::from("src/main.rs"))
        );
    }

    #[test]
    fn groups_workspace_members_but_keeps_nested_workspaces_separate() {
        let temp = Temp::new();
        fs::create_dir_all(temp.0.join("crates/a/src")).unwrap();
        fs::create_dir_all(temp.0.join("crates/b/src")).unwrap();
        fs::create_dir_all(temp.0.join("nested/src")).unwrap();
        fs::write(
            temp.0.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();
        fs::write(
            temp.0.join("crates/a/Cargo.toml"),
            package_manifest("member-a", ""),
        )
        .unwrap();
        fs::write(temp.0.join("crates/a/src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(
            temp.0.join("crates/b/Cargo.toml"),
            package_manifest("member-b", ""),
        )
        .unwrap();
        fs::write(temp.0.join("crates/b/src/lib.rs"), "").unwrap();
        fs::write(
            temp.0.join("nested/Cargo.toml"),
            format!("{}\n[workspace]\n", package_manifest("nested", "")),
        )
        .unwrap();
        fs::write(temp.0.join("nested/src/lib.rs"), "").unwrap();
        let workspace = temp.workspace();
        let mut projects = ProjectManager::new();
        let provider = CargoProvider::new();
        provider.register(&mut projects).unwrap();
        let provider_id = ProviderId::new(CARGO_PROVIDER_ID).unwrap();
        let ids = projects
            .discover_registered(&workspace, &provider_id, DiscoveryLimits::default())
            .unwrap();
        assert_eq!(ids.len(), 2);
        let records = ids
            .iter()
            .map(|id| {
                provider
                    .project_metadata(projects.info(*id).unwrap())
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let root = records
            .iter()
            .find(|entry| entry.root.as_os_str().is_empty())
            .unwrap();
        assert_eq!(
            root.members
                .iter()
                .map(|member| member.name.as_str())
                .collect::<Vec<_>>(),
            ["member-a", "member-b"]
        );
        assert!(
            records
                .iter()
                .any(|entry| entry.root == Path::new("nested"))
        );
        let root_id = ids
            .iter()
            .copied()
            .find(|id| projects.info(*id).unwrap().root.as_os_str().is_empty())
            .unwrap();
        let root_info = projects.info(root_id).unwrap();
        let tasks = projects.provider_tasks(&provider_id, root_info).unwrap();
        assert!(tasks.iter().any(|task| task.key == "cargo.workspace.build"));
        assert!(
            tasks
                .iter()
                .any(|task| task.key == "cargo.package.member-a.build")
        );
        assert!(
            tasks
                .iter()
                .any(|task| task.key == "cargo.package.member-b.test")
        );
        assert!(
            tasks
                .iter()
                .any(|task| task.key == "cargo.package.member-a.run.member-a")
        );
        assert_eq!(
            provider
                .discover(&workspace, &mut projects, DiscoveryLimits::default())
                .unwrap()
                .iter()
                .map(|project| project.project_id)
                .collect::<Vec<_>>(),
            ids
        );
        projects.unregister(root_id).unwrap();
        assert!(projects.info(root_id).is_err());
    }

    #[test]
    fn invalid_manifests_and_discovery_limits_are_reported() {
        let temp = Temp::new();
        fs::write(temp.0.join("Cargo.toml"), "[package\n").unwrap();
        let workspace = temp.workspace();
        let error = CargoProvider::new()
            .discover(
                &workspace,
                &mut ProjectManager::new(),
                DiscoveryLimits::default(),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            CargoError::InvalidManifest { .. } | CargoError::Project(ProjectError::Invalid(_))
        ));

        let temp = Temp::new();
        fs::create_dir(temp.0.join("child")).unwrap();
        let workspace = temp.workspace();
        let error = CargoProvider::new()
            .discover(
                &workspace,
                &mut ProjectManager::new(),
                DiscoveryLimits {
                    max_directories: 1,
                    max_projects: 10,
                    max_depth: 10,
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("directory limit"));
    }

    #[test]
    fn metadata_parser_reports_offline_and_missing_executable_failures() {
        let spawn = TaskSnapshot {
            run_id: koren_tasks::TaskRunId::from_raw(1),
            task_id: koren_tasks::TaskId::from_raw(1),
            state: TaskState::SpawnFailed {
                message: "not found".into(),
            },
            stdout: String::new(),
            stderr: String::new(),
            output_truncated: false,
        };
        assert!(
            CargoProvider::parse_metadata_output(&spawn)
                .unwrap_err()
                .to_string()
                .contains("could not start Cargo")
        );
        let failed = TaskSnapshot {
            state: TaskState::Failed {
                exit_code: Some(101),
            },
            stderr: "no cached package".into(),
            ..spawn
        };
        assert!(
            CargoProvider::parse_metadata_output(&failed)
                .unwrap_err()
                .to_string()
                .contains("offline")
        );
    }

    #[test]
    fn cargo_metadata_target_mapping_is_typed() {
        let package = CargoMetadataPackage {
            id: "test 0.1.0".into(),
            name: "test".into(),
            version: "0.1.0".into(),
            manifest_path: "Cargo.toml".into(),
            targets: vec![CargoMetadataTarget {
                name: "test-bin".into(),
                kind: vec!["bin".into()],
                src_path: "src/main.rs".into(),
            }],
        };
        assert_eq!(metadata_targets(&package)[0].kind, CargoTargetKind::Binary);
    }
}
