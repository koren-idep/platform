//! Language-neutral logical projects associated with an open workspace.

use koren_core::PlatformEvent;
use koren_tasks::TaskDefinition;
use koren_workspace::{EntryKind, WorkspaceError, WorkspaceManager};
use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    path::{Path, PathBuf},
};
const PROJECT_EVENT_LIMIT: usize = 1024;

/// Manager-local project identity. IDs are not stable across manager instances or sessions.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProjectId(u64);
impl ProjectId {
    /// Reconstructs an identifier received from a trusted host protocol.
    #[must_use]
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }
    /// Numeric manager-local value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Provider identity, such as a future language-neutral build-system integration.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProviderId(String);
impl ProviderId {
    /// Creates a non-empty provider identifier.
    ///
    /// # Errors
    ///
    /// Returns an error if the identifier is empty or whitespace only.
    pub fn new(value: impl Into<String>) -> Result<Self, ProjectError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ProjectError::Invalid(
                "provider id must not be empty".into(),
            ));
        }
        Ok(Self(value))
    }
    /// Text identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Lifecycle of a registered project.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectState {
    /// Registered and available.
    Registered,
    /// Closing and unavailable to new work.
    Closing,
}

/// Language-neutral project registration request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSpec {
    /// Human-readable name.
    pub display_name: String,
    /// Workspace-relative project directory.
    pub root: PathBuf,
    /// Identifier for the detecting or owning provider.
    pub provider_id: ProviderId,
    /// Optional workspace-relative configuration file.
    pub configuration_path: Option<PathBuf>,
    /// Provider-specific metadata.
    pub provider_data: BTreeMap<String, String>,
}

/// Registered project information with manager-local identity and workspace association.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectInfo {
    /// Manager-local identity.
    pub id: ProjectId,
    /// Display name.
    pub display_name: String,
    /// Canonical associated workspace root.
    pub workspace_root: PathBuf,
    /// Workspace-relative project root.
    pub root: PathBuf,
    /// Provider identifier.
    pub provider_id: ProviderId,
    /// Optional configuration path.
    pub configuration_path: Option<PathBuf>,
    /// Provider metadata.
    pub provider_data: BTreeMap<String, String>,
    /// Current lifecycle state.
    pub state: ProjectState,
}

/// Project operation failure.
#[derive(Debug)]
pub enum ProjectError {
    /// Workspace isn't open.
    Workspace(WorkspaceError),
    /// Duplicate (workspace, root, provider) registration.
    Duplicate(ProjectId),
    /// Unknown identifier.
    NotFound(ProjectId),
    /// Invalid specification or discovery limits.
    Invalid(String),
}
impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Workspace(e) => write!(f, "{e}"),
            Self::Duplicate(id) => write!(f, "project duplicates registered project {}", id.get()),
            Self::NotFound(id) => write!(f, "project {} was not found", id.get()),
            Self::Invalid(s) => f.write_str(s),
        }
    }
}
impl std::error::Error for ProjectError {}
impl From<WorkspaceError> for ProjectError {
    fn from(e: WorkspaceError) -> Self {
        Self::Workspace(e)
    }
}

/// Discovery provider. Implementations inspect directory entries only; discovery never runs commands.
pub trait ProjectProvider: Send + Sync {
    /// Provider identifier.
    fn id(&self) -> &ProviderId;
    /// Return a candidate when this directory matches. Paths are relative to the workspace root.
    fn detect(
        &self,
        directory: &Path,
        entries: &[koren_workspace::WorkspaceEntry],
    ) -> Option<ProjectSpec>;
    /// Task capabilities provided by this project system. Empty means no build tasks are exposed.
    fn task_definitions(&self, _project: &ProjectInfo) -> Vec<TaskDefinition> {
        Vec::new()
    }
}

/// A generic marker-file provider for tests and simple user-configured project types.
pub struct MarkerFileProvider {
    id: ProviderId,
    marker: String,
}
impl MarkerFileProvider {
    /// Creates a provider that recognizes a configured marker filename, without reading or executing it.
    pub fn new(id: ProviderId, marker: impl Into<String>) -> Self {
        Self {
            id,
            marker: marker.into(),
        }
    }
}
impl ProjectProvider for MarkerFileProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn detect(
        &self,
        directory: &Path,
        entries: &[koren_workspace::WorkspaceEntry],
    ) -> Option<ProjectSpec> {
        let marker = entries
            .iter()
            .find(|e| e.name == std::ffi::OsStr::new(&self.marker) && e.kind == EntryKind::File)?;
        let name = directory
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Project")
            .to_owned();
        Some(ProjectSpec {
            display_name: name,
            root: directory.to_path_buf(),
            provider_id: self.id.clone(),
            configuration_path: Some(marker.path.clone()),
            provider_data: BTreeMap::new(),
        })
    }
}

/// Explicit limits for provider discovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiscoveryLimits {
    /// Maximum directories inspected.
    pub max_directories: usize,
    /// Maximum candidates returned.
    pub max_projects: usize,
    /// Maximum nesting depth; workspace root is depth zero.
    pub max_depth: usize,
}
impl Default for DiscoveryLimits {
    fn default() -> Self {
        Self {
            max_directories: 10_000,
            max_projects: 1_000,
            max_depth: 32,
        }
    }
}

/// Owns project records and FIFO project lifecycle events.
pub struct ProjectManager {
    next_id: u64,
    projects: BTreeMap<ProjectId, ProjectInfo>,
    events: VecDeque<PlatformEvent>,
    providers: BTreeMap<ProviderId, Box<dyn ProjectProvider>>,
}
// `Default` starts IDs at one so it has the same identity semantics as `new()`.
#[allow(clippy::derivable_impls)]
impl Default for ProjectManager {
    fn default() -> Self {
        Self {
            next_id: 1,
            projects: BTreeMap::new(),
            events: VecDeque::new(),
            providers: BTreeMap::new(),
        }
    }
}
impl fmt::Debug for ProjectManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectManager")
            .field("projects", &self.projects.len())
            .field("providers", &self.providers.len())
            .finish_non_exhaustive()
    }
}
impl ProjectManager {
    /// Creates an empty manager.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Registers one explicit discovery/build capability provider.
    ///
    /// # Errors
    ///
    /// Returns an error if a provider with the same identifier is already registered.
    pub fn register_provider(
        &mut self,
        provider: Box<dyn ProjectProvider>,
    ) -> Result<(), ProjectError> {
        let id = provider.id().clone();
        if self.providers.contains_key(&id) {
            return Err(ProjectError::Invalid(format!(
                "provider '{}' is already registered",
                id.as_str()
            )));
        }
        self.providers.insert(id, provider);
        Ok(())
    }
    /// Discovers projects using a previously registered provider.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown provider, invalid limits, workspace errors,
    /// duplicate roots, or invalid provider candidates.
    pub fn discover_registered(
        &mut self,
        workspace: &WorkspaceManager,
        id: &ProviderId,
        limits: DiscoveryLimits,
    ) -> Result<Vec<ProjectId>, ProjectError> {
        let provider = self.providers.get(id).ok_or_else(|| {
            ProjectError::Invalid(format!("provider '{}' is not registered", id.as_str()))
        })?;
        let specs = collect_candidates(workspace, provider.as_ref(), limits)?;
        specs
            .into_iter()
            .map(|spec| self.register_discovered(workspace, spec))
            .collect()
    }
    /// Registers a project after validating its root and optional configuration path.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid paths, an empty name, a duplicate root, a closed workspace,
    /// or exhausted identifiers.
    pub fn register(
        &mut self,
        workspace: &WorkspaceManager,
        spec: ProjectSpec,
    ) -> Result<ProjectId, ProjectError> {
        let workspace_root = workspace.root()?.to_path_buf();
        let root = normalize(&spec.root)?;
        if root.as_os_str().is_empty() {
            workspace.entries()?;
        } else {
            workspace.list_dir(&root)?;
        }
        if let Some(config) = &spec.configuration_path {
            workspace.resolve_file(config)?;
        }
        if let Some(existing) = self
            .projects
            .values()
            .find(|p| p.workspace_root == workspace_root && p.root == root)
        {
            return Err(ProjectError::Duplicate(existing.id));
        }
        if spec.display_name.trim().is_empty() {
            return Err(ProjectError::Invalid(
                "project display name must not be empty".into(),
            ));
        }
        let id = ProjectId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| ProjectError::Invalid("project id space exhausted".into()))?;
        self.projects.insert(
            id,
            ProjectInfo {
                id,
                display_name: spec.display_name,
                workspace_root,
                root,
                provider_id: spec.provider_id,
                configuration_path: spec.configuration_path,
                provider_data: spec.provider_data,
                state: ProjectState::Registered,
            },
        );
        push_project_event(
            &mut self.events,
            PlatformEvent::ProjectRegistered {
                project_id: id.get(),
            },
        );
        Ok(id)
    }
    /// Unregisters and closes a project. Existing task services should cancel its work separately.
    ///
    /// # Errors
    ///
    /// Returns an error if the project identifier is unknown.
    pub fn unregister(&mut self, id: ProjectId) -> Result<ProjectInfo, ProjectError> {
        let mut info = self
            .projects
            .remove(&id)
            .ok_or(ProjectError::NotFound(id))?;
        info.state = ProjectState::Closing;
        push_project_event(
            &mut self.events,
            PlatformEvent::ProjectRemoved {
                project_id: id.get(),
            },
        );
        Ok(info)
    }
    /// Returns registered projects in identifier order.
    #[must_use]
    pub fn list(&self) -> Vec<&ProjectInfo> {
        self.projects.values().collect()
    }
    /// Retrieves project information.
    ///
    /// # Errors
    ///
    /// Returns an error if the project identifier is unknown.
    pub fn info(&self, id: ProjectId) -> Result<&ProjectInfo, ProjectError> {
        self.projects.get(&id).ok_or(ProjectError::NotFound(id))
    }
    /// Removes all projects associated with a workspace during close.
    pub fn close_workspace(&mut self, root: &Path) -> Vec<ProjectInfo> {
        let ids = self
            .projects
            .values()
            .filter(|p| p.workspace_root == root)
            .map(|p| p.id)
            .collect::<Vec<_>>();
        ids.into_iter()
            .filter_map(|id| self.unregister(id).ok())
            .collect()
    }
    /// Discovers projects only when explicitly called. Traversal does not follow symlinks.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid limits, workspace failures, duplicate roots,
    /// or invalid provider candidates.
    pub fn discover(
        &mut self,
        workspace: &WorkspaceManager,
        provider: &dyn ProjectProvider,
        limits: DiscoveryLimits,
    ) -> Result<Vec<ProjectId>, ProjectError> {
        collect_candidates(workspace, provider, limits)?
            .into_iter()
            .map(|spec| self.register_discovered(workspace, spec))
            .collect()
    }
    /// Returns task capabilities for a registered project provider.
    ///
    /// # Errors
    ///
    /// Returns an error if the provider is not registered.
    pub fn provider_tasks(
        &self,
        id: &ProviderId,
        project: &ProjectInfo,
    ) -> Result<Vec<TaskDefinition>, ProjectError> {
        let provider = self.providers.get(id).ok_or_else(|| {
            ProjectError::Invalid(format!("provider '{}' is not registered", id.as_str()))
        })?;
        if project.provider_id != *id {
            return Err(ProjectError::Invalid(
                "project belongs to a different provider".into(),
            ));
        }
        Ok(provider.task_definitions(project))
    }
    fn register_discovered(
        &mut self,
        workspace: &WorkspaceManager,
        spec: ProjectSpec,
    ) -> Result<ProjectId, ProjectError> {
        let workspace_root = workspace.root()?;
        let root = normalize(&spec.root)?;
        if let Some(existing) = self
            .projects
            .values()
            .find(|p| p.workspace_root == workspace_root && p.root == root)
        {
            return Ok(existing.id);
        }
        self.register(workspace, spec)
    }
    /// Drains project lifecycle events in FIFO order.
    pub fn drain_events(&mut self) -> Vec<PlatformEvent> {
        self.events.drain(..).collect()
    }
}
fn push_project_event(events: &mut VecDeque<PlatformEvent>, event: PlatformEvent) {
    if events.len() == PROJECT_EVENT_LIMIT {
        events.pop_front();
    }
    events.push_back(event);
}
fn collect_candidates(
    workspace: &WorkspaceManager,
    provider: &dyn ProjectProvider,
    limits: DiscoveryLimits,
) -> Result<Vec<ProjectSpec>, ProjectError> {
    if limits.max_directories == 0 || limits.max_projects == 0 {
        return Err(ProjectError::Invalid(
            "discovery limits must be greater than zero".into(),
        ));
    }
    let mut queue = VecDeque::from([(PathBuf::new(), 0usize)]);
    let mut inspected = 0usize;
    let mut candidates = Vec::new();
    while let Some((directory, depth)) = queue.pop_front() {
        if inspected >= limits.max_directories {
            return Err(ProjectError::Invalid(
                "project discovery directory limit exceeded".into(),
            ));
        }
        inspected += 1;
        let entries = if directory.as_os_str().is_empty() {
            workspace.entries()?
        } else {
            workspace.list_dir(&directory)?
        };
        if let Some(spec) = provider.detect(&directory, &entries) {
            if spec.provider_id != *provider.id() {
                return Err(ProjectError::Invalid(
                    "provider returned a candidate with a different provider id".into(),
                ));
            }
            if normalize(&spec.root)? != directory {
                return Err(ProjectError::Invalid(
                    "provider candidate root must match the inspected directory".into(),
                ));
            }
            candidates.push(spec);
            if candidates.len() > limits.max_projects {
                return Err(ProjectError::Invalid(
                    "project discovery result limit exceeded".into(),
                ));
            }
        }
        if depth < limits.max_depth {
            for entry in entries {
                if entry.kind == EntryKind::Directory {
                    if inspected + queue.len() >= limits.max_directories {
                        return Err(ProjectError::Invalid(
                            "project discovery directory limit exceeded".into(),
                        ));
                    }
                    queue.push_back((entry.path, depth + 1));
                }
            }
        }
    }
    Ok(candidates)
}
fn normalize(path: &Path) -> Result<PathBuf, ProjectError> {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(v) => out.push(v),
            _ => {
                return Err(ProjectError::Invalid(format!(
                    "invalid project root '{}'",
                    path.display()
                )));
            }
        }
    }
    Ok(out)
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
            let p = std::env::temp_dir().join(format!(
                "koren-project-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn setup() -> (Temp, WorkspaceManager) {
        let t = Temp::new();
        let mut w = WorkspaceManager::new();
        w.open(&t.0).unwrap();
        (t, w)
    }
    fn spec(root: &str) -> ProjectSpec {
        ProjectSpec {
            display_name: root.into(),
            root: root.into(),
            provider_id: ProviderId::new("fixture").unwrap(),
            configuration_path: None,
            provider_data: BTreeMap::new(),
        }
    }
    #[test]
    fn multiple_nested_projects_and_duplicates_have_explicit_lifecycle() {
        let (temporary, workspace) = setup();
        fs::create_dir(temporary.0.join("outer")).unwrap();
        fs::create_dir(temporary.0.join("outer/inner")).unwrap();
        let mut manager = ProjectManager::default();
        let first_id = manager.register(&workspace, spec("outer")).unwrap();
        assert_eq!(first_id.get(), 1);
        let second_id = manager.register(&workspace, spec("outer/inner")).unwrap();
        assert_ne!(first_id, second_id);
        assert!(matches!(
            manager.register(&workspace, spec("outer")),
            Err(ProjectError::Duplicate(_))
        ));
        let mut competing = spec("outer");
        competing.provider_id = ProviderId::new("other").unwrap();
        assert!(matches!(
            manager.register(&workspace, competing),
            Err(ProjectError::Duplicate(_))
        ));
        assert_eq!(manager.list().len(), 2);
        manager.unregister(first_id).unwrap();
        assert!(manager.info(first_id).is_err());
        assert_eq!(manager.drain_events().len(), 3);
    }
    #[test]
    fn discovery_is_explicit_and_bounded() {
        let (t, w) = setup();
        fs::create_dir(t.0.join("a")).unwrap();
        fs::create_dir(t.0.join("b")).unwrap();
        fs::write(t.0.join("a/marker"), "not executed").unwrap();
        fs::write(t.0.join("b/marker"), "not executed").unwrap();
        let id = ProviderId::new("fixture").unwrap();
        let mut m = ProjectManager::new();
        m.register_provider(Box::new(MarkerFileProvider::new(id.clone(), "marker")))
            .unwrap();
        assert!(
            m.register_provider(Box::new(MarkerFileProvider::new(id.clone(), "marker")))
                .is_err()
        );
        let found = m
            .discover_registered(&w, &id, DiscoveryLimits::default())
            .unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(
            m.discover_registered(&w, &id, DiscoveryLimits::default())
                .unwrap(),
            found
        );
        let mut bounded = ProjectManager::new();
        bounded
            .register_provider(Box::new(MarkerFileProvider::new(id.clone(), "marker")))
            .unwrap();
        assert!(
            bounded
                .discover_registered(
                    &w,
                    &id,
                    DiscoveryLimits {
                        max_directories: 1,
                        max_projects: 4,
                        max_depth: 9
                    }
                )
                .is_err()
        );
        assert_eq!(bounded.list().len(), 0);
    }
    #[test]
    fn registered_provider_exposes_optional_task_capabilities() {
        struct BuildFixture(MarkerFileProvider);
        impl ProjectProvider for BuildFixture {
            fn id(&self) -> &ProviderId {
                self.0.id()
            }
            fn detect(
                &self,
                directory: &Path,
                entries: &[koren_workspace::WorkspaceEntry],
            ) -> Option<ProjectSpec> {
                self.0.detect(directory, entries)
            }
            fn task_definitions(&self, project: &ProjectInfo) -> Vec<TaskDefinition> {
                vec![TaskDefinition {
                    key: "fixture.check".into(),
                    display_name: "Fixture check".into(),
                    category: koren_tasks::TaskCategory::Check,
                    executable: "fixture-tool".into(),
                    arguments: vec![],
                    working_directory: project.root.clone(),
                    environment: BTreeMap::new(),
                    timeout: None,
                    project_id: Some(project.id.get()),
                }]
            }
        }
        let (t, w) = setup();
        fs::create_dir(t.0.join("module")).unwrap();
        fs::write(t.0.join("module/manifest"), "opaque").unwrap();
        let id = ProviderId::new("fixture-build").unwrap();
        let mut manager = ProjectManager::new();
        manager
            .register_provider(Box::new(BuildFixture(MarkerFileProvider::new(
                id.clone(),
                "manifest",
            ))))
            .unwrap();
        let projects = manager
            .discover_registered(&w, &id, DiscoveryLimits::default())
            .unwrap();
        let info = manager.info(projects[0]).unwrap();
        let tasks = manager.provider_tasks(&id, info).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].project_id, Some(projects[0].get()));
    }
    #[test]
    fn project_event_history_is_bounded_and_keeps_fifo_order() {
        let (_temporary, workspace) = setup();
        let mut manager = ProjectManager::new();
        for _ in 0..513 {
            let id = manager.register(&workspace, spec(".")).unwrap();
            manager.unregister(id).unwrap();
        }
        let events = manager.drain_events();
        assert_eq!(events.len(), PROJECT_EVENT_LIMIT);
        assert_eq!(
            events.first(),
            Some(&PlatformEvent::ProjectRegistered { project_id: 2 })
        );
        assert_eq!(
            events.last(),
            Some(&PlatformEvent::ProjectRemoved { project_id: 513 })
        );
    }
}
