//! Shared, language-neutral inspection contracts and bounded execution.
//!
//! Providers return findings against explicit document snapshots. The host remains responsible
//! for deciding whether a result still matches a live document revision and for applying edits.

use koren_protocol::{TextPosition, TextRange};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

/// Maximum SARIF document size accepted on import or export.
pub const MAX_SARIF_BYTES: usize = 5 * 1024 * 1024;
/// Maximum number of SARIF results accepted in one import.
pub const MAX_SARIF_RESULTS: usize = 20_000;
/// Maximum findings retained from one provider job.
pub const MAX_JOB_FINDINGS: usize = 50_000;
/// Maximum documents in one explicit project snapshot job.
pub const MAX_PROJECT_DOCUMENTS: usize = 10_000;
/// Maximum combined text retained by one project snapshot job.
pub const MAX_PROJECT_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;

/// Stable provider identity, separate from rule identity.
pub type ProviderId = String;
/// Stable rule identity, preferably namespaced by provider.
pub type RuleId = String;

/// Severity used by diagnostics from any supported source.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational finding.
    Information,
    /// Low-priority hint.
    Hint,
    /// Warning that should be reviewed.
    Warning,
    /// Error that prevents a correct result.
    Error,
}

/// Origin of a finding, preserved when sources are merged.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FindingOrigin {
    /// Live document analysis.
    Document,
    /// Explicit project analysis tied to a captured snapshot.
    ProjectSnapshot {
        /// Stable identifier for the captured project snapshot.
        snapshot_id: String,
    },
    /// Imported CI or external analysis report.
    Imported {
        /// Stable identifier for this imported report.
        report_id: String,
        /// Source revision associated with the report, when established.
        source_revision: Option<String>,
    },
}

/// A related location belonging to a finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelatedLocation {
    /// Workspace-relative path or file URI.
    pub path: String,
    /// Zero-based UTF-16 source range.
    pub range: TextRange,
    /// Explanation at the related location.
    pub message: String,
}

/// A proposed safe text edit. It is data only and never applies itself.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProposedTextEdit {
    /// Workspace-relative path.
    pub path: String,
    /// Document revision used to calculate this edit, when the file is open.
    pub expected_revision: Option<u64>,
    /// Range in zero-based UTF-16 coordinates.
    pub range: TextRange,
    /// Replacement text.
    pub new_text: String,
}

/// A provider command reference. Hosts must resolve it through authorized host APIs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderCommand {
    /// Provider-owned stable command identifier.
    pub id: String,
    /// User-facing title.
    pub title: String,
}

/// A code action or intention that requires explicit user selection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodeAction {
    /// Request-scoped action identifier.
    pub id: String,
    /// User-facing action title.
    pub title: String,
    /// Provider that proposed the action.
    pub provider_id: ProviderId,
    /// Action kind, such as quickfix, refactor, source, or intention.
    pub kind: String,
    /// Related finding IDs, if any.
    pub diagnostic_ids: Vec<String>,
    /// Supported text-only edits, if supplied.
    pub edits: Vec<ProposedTextEdit>,
    /// Optional provider command, never executed by the inspection service.
    pub command: Option<ProviderCommand>,
    /// Why the action cannot currently be applied.
    pub disabled_reason: Option<String>,
}

/// Unified language-neutral inspection finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// Stable identity for this finding instance.
    pub id: String,
    /// Provider identity, independent from `rule_id`.
    pub provider_id: ProviderId,
    /// Stable rule or diagnostic identity.
    pub rule_id: RuleId,
    /// Source file path or URI.
    pub path: String,
    /// Manager-local document ID when the source is open.
    pub document_id: Option<u64>,
    /// Document revision or analysis snapshot revision, if known.
    pub document_revision: Option<u64>,
    /// SARIF or external source revision, if known.
    pub source_revision: Option<String>,
    /// UTF-16 range in the source document.
    pub range: TextRange,
    /// Effective severity after profile overrides.
    pub severity: Severity,
    /// User-facing message.
    pub message: String,
    /// Optional explanation.
    pub explanation: Option<String>,
    /// Optional category for grouping.
    pub category: Option<String>,
    /// Additional related locations.
    pub related: Vec<RelatedLocation>,
    /// Actions associated with this finding.
    pub actions: Vec<CodeAction>,
    /// Documentation reference; consumers must not fetch it during report import.
    pub documentation: Option<String>,
    /// Result source class.
    pub origin: FindingOrigin,
}

/// Provider rule declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuleMetadata {
    /// Stable rule ID.
    pub id: RuleId,
    /// Owning provider.
    pub provider_id: ProviderId,
    /// User-facing title.
    pub title: String,
    /// Description and remediation guidance.
    pub description: String,
    /// Category used for filtering.
    pub category: String,
    /// Languages the rule supports; empty means language-neutral.
    pub languages: BTreeSet<String>,
    /// Default severity.
    pub default_severity: Severity,
    /// Whether the provider may offer an automatic text edit.
    pub supports_fixes: bool,
    /// Typed provider-specific option schema and defaults.
    pub options: BTreeMap<String, OptionValue>,
}

/// Primitive typed option values used by rules and profiles.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OptionValue {
    /// Boolean option.
    Boolean(bool),
    /// Integer option.
    Integer(i64),
    /// Text option.
    Text(String),
}

/// Per-rule profile override.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuleProfile {
    /// Whether this rule is enabled.
    pub enabled: bool,
    /// Optional severity override.
    pub severity: Option<Severity>,
    /// Rule options, validated against the rule's typed defaults.
    pub options: BTreeMap<String, OptionValue>,
}

/// Named inspection profile with optional workspace-local overrides.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InspectionProfile {
    /// User-visible profile name.
    pub name: String,
    /// Global rule settings; unknown rules are preserved while providers are unavailable.
    pub rules: BTreeMap<RuleId, RuleProfile>,
    /// Workspace ID to rule overrides.
    pub workspace_overrides: BTreeMap<String, BTreeMap<RuleId, RuleProfile>>,
}

impl InspectionProfile {
    /// Creates an empty profile with the given display name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            rules: BTreeMap::new(),
            workspace_overrides: BTreeMap::new(),
        }
    }

    /// Resolves an effective setting, applying workspace overrides over the profile default.
    #[must_use]
    pub fn effective_rule(&self, workspace: Option<&str>, rule: &RuleMetadata) -> RuleProfile {
        let default = || RuleProfile {
            enabled: true,
            severity: Some(rule.default_severity),
            options: rule.options.clone(),
        };
        let mut result = default();
        if let Some(setting) = self.rules.get(&rule.id) {
            result.enabled = setting.enabled;
            if setting.severity.is_some() {
                result.severity = setting.severity;
            }
            result.options.extend(setting.options.clone());
        }
        if let Some(override_value) = workspace
            .and_then(|id| self.workspace_overrides.get(id))
            .and_then(|rules| rules.get(&rule.id))
        {
            result.enabled = override_value.enabled;
            if override_value.severity.is_some() {
                result.severity = override_value.severity;
            }
            result.options.extend(override_value.options.clone());
        }
        result
    }

    /// Validates and stores one rule override, preserving settings for unavailable providers.
    ///
    /// # Errors
    /// Returns an error if the override contains an unknown or mismatched typed option.
    pub fn set_rule(
        &mut self,
        workspace: Option<&str>,
        rule: &RuleMetadata,
        setting: RuleProfile,
    ) -> Result<(), InspectionError> {
        validate_profile_options(rule, &setting.options)?;
        let destination = if let Some(workspace) = workspace {
            self.workspace_overrides
                .entry(workspace.to_owned())
                .or_default()
        } else {
            &mut self.rules
        };
        destination.insert(rule.id.clone(), setting);
        Ok(())
    }
}

/// Collection of named inspection profiles and workspace-specific selection.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct InspectionProfiles {
    /// Profile name to settings. Unavailable providers' rule settings remain stored here.
    pub profiles: BTreeMap<String, InspectionProfile>,
    /// Globally selected profile name.
    pub selected: Option<String>,
    /// Workspace ID to selected profile name.
    pub workspace_selection: BTreeMap<String, String>,
}

impl InspectionProfiles {
    /// Stores or replaces a named profile without dropping unknown rule settings.
    pub fn insert(&mut self, profile: InspectionProfile) {
        self.profiles.insert(profile.name.clone(), profile);
    }

    /// Selects a profile globally or for one workspace.
    ///
    /// # Errors
    /// Returns an error when the named profile has not been registered.
    pub fn select(&mut self, workspace: Option<&str>, name: &str) -> Result<(), InspectionError> {
        if !self.profiles.contains_key(name) {
            return Err(InspectionError::UnknownProfile(name.to_owned()));
        }
        if let Some(workspace) = workspace {
            self.workspace_selection
                .insert(workspace.to_owned(), name.to_owned());
        } else {
            self.selected = Some(name.to_owned());
        }
        Ok(())
    }

    /// Resolves the selected profile for a workspace, falling back to the global profile.
    #[must_use]
    pub fn selected_for(&self, workspace: Option<&str>) -> Option<&InspectionProfile> {
        workspace
            .and_then(|id| self.workspace_selection.get(id))
            .and_then(|name| self.profiles.get(name))
            .or_else(|| {
                self.selected
                    .as_ref()
                    .and_then(|name| self.profiles.get(name))
            })
    }
}

fn validate_profile_options(
    rule: &RuleMetadata,
    options: &BTreeMap<String, OptionValue>,
) -> Result<(), InspectionError> {
    for (name, value) in options {
        match rule.options.get(name) {
            Some(OptionValue::Boolean(_)) if matches!(value, OptionValue::Boolean(_)) => {}
            Some(OptionValue::Integer(_)) if matches!(value, OptionValue::Integer(_)) => {}
            Some(OptionValue::Text(_)) if matches!(value, OptionValue::Text(_)) => {}
            Some(_) => {
                return Err(InspectionError::InvalidProfile(format!(
                    "option '{name}' has the wrong type"
                )));
            }
            None => {
                return Err(InspectionError::InvalidProfile(format!(
                    "unknown option '{name}'"
                )));
            }
        }
    }
    Ok(())
}

/// Explicit snapshot passed to document and project inspection providers.
#[derive(Clone, Debug)]
pub struct InspectionDocument {
    /// Stable path or URI.
    pub path: String,
    /// Manager-local document ID, if open.
    pub document_id: Option<u64>,
    /// Current document revision, if open.
    pub revision: Option<u64>,
    /// Language identifier.
    pub language: String,
    /// Immutable text snapshot.
    pub text: Arc<str>,
}

/// Cooperative cancellation token for an inspection job.
#[derive(Clone, Debug)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// Creates a token that is not yet cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
    /// Returns whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    /// Stops a provider cooperatively.
    ///
    /// # Errors
    /// Returns [`InspectionError::Cancelled`] after cancellation.
    pub fn checkpoint(&self) -> Result<(), InspectionError> {
        if self.is_cancelled() {
            Err(InspectionError::Cancelled)
        } else {
            Ok(())
        }
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

/// One provider's document-level inspection API.
pub trait InspectionProvider: Send + Sync + 'static {
    /// Stable provider identity.
    fn id(&self) -> &str;
    /// Public rule declarations.
    fn rules(&self) -> Vec<RuleMetadata>;
    /// Analyzes one immutable document snapshot; it must not access arbitrary files or processes.
    ///
    /// # Errors
    /// Returns a provider-defined failure message.
    fn inspect(
        &self,
        document: &InspectionDocument,
        profile: &InspectionProfile,
        workspace: Option<&str>,
        cancellation: &CancellationToken,
    ) -> Result<Vec<Finding>, String>;
}

/// Bounded explicit document-inspection service.
pub struct InspectionService {
    providers: BTreeMap<ProviderId, Arc<dyn InspectionProvider>>,
    active_jobs: Arc<AtomicUsize>,
    max_concurrent_jobs: usize,
}

impl Default for InspectionService {
    fn default() -> Self {
        Self::with_concurrency(4)
    }
}

impl fmt::Debug for InspectionService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InspectionService")
            .field("provider_count", &self.providers.len())
            .field("active_jobs", &self.active_jobs.load(Ordering::Acquire))
            .field("max_concurrent_jobs", &self.max_concurrent_jobs)
            .finish()
    }
}

impl InspectionService {
    /// Creates a service with a hard concurrency limit.
    #[must_use]
    pub fn with_concurrency(max_concurrent_jobs: usize) -> Self {
        Self {
            providers: BTreeMap::new(),
            active_jobs: Arc::new(AtomicUsize::new(0)),
            max_concurrent_jobs: max_concurrent_jobs.max(1),
        }
    }

    /// Registers a provider, rejecting duplicate IDs, invalid rules, and cross-owner declarations.
    ///
    /// # Errors
    /// Returns an error when the provider or one of its rule declarations is invalid.
    pub fn register(
        &mut self,
        provider: Arc<dyn InspectionProvider>,
    ) -> Result<(), InspectionError> {
        let id = provider.id().to_owned();
        validate_identifier(&id)?;
        if self.providers.contains_key(&id) {
            return Err(InspectionError::DuplicateProvider(id));
        }
        let mut rules = BTreeSet::new();
        for rule in provider.rules() {
            validate_identifier(&rule.id)?;
            if rule.provider_id != id || !rules.insert(rule.id.clone()) {
                return Err(InspectionError::InvalidProvider(format!(
                    "provider '{id}' has invalid or duplicate rule '{}'",
                    rule.id
                )));
            }
        }
        self.providers.insert(id, provider);
        Ok(())
    }

    /// Removes a provider and prevents it from receiving future jobs.
    pub fn unregister(&mut self, provider_id: &str) -> bool {
        self.providers.remove(provider_id).is_some()
    }

    /// Returns a snapshot of registered rules.
    #[must_use]
    pub fn rules(&self) -> Vec<RuleMetadata> {
        let mut rules = self
            .providers
            .values()
            .flat_map(|provider| provider.rules())
            .collect::<Vec<_>>();
        rules.sort_by(|left, right| left.id.cmp(&right.id));
        rules
    }

    /// Starts one explicit document inspection job without blocking the caller.
    ///
    /// Jobs beyond the configured concurrency bound are rejected; no hidden queue retains
    /// document text. Providers are called sequentially within each bounded job thread.
    ///
    /// # Errors
    /// Returns an error when the job limit is full or the worker thread cannot be created.
    pub fn inspect_document(
        &self,
        document: InspectionDocument,
        profile: InspectionProfile,
        workspace: Option<String>,
    ) -> Result<InspectionJob, InspectionError> {
        reserve_job(&self.active_jobs, self.max_concurrent_jobs)?;
        let providers = self.providers.values().cloned().collect::<Vec<_>>();
        let active_jobs = self.active_jobs.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let token = CancellationToken(cancelled.clone());
        let (sender, receiver) = mpsc::sync_channel(1);
        let (progress_sender, progress_receiver) = mpsc::sync_channel(32);
        let spawn = thread::Builder::new()
            .name("koren-inspection-document".into())
            .spawn(move || {
                let _job = ActiveJob(active_jobs);
                let result = run_providers(
                    &providers,
                    &document,
                    &profile,
                    workspace.as_deref(),
                    &token,
                    &FindingOrigin::Document,
                );
                let _ = progress_sender.try_send(InspectionProgress {
                    completed_documents: usize::from(result.is_ok()),
                    total_documents: 1,
                    current_path: document.path.clone(),
                });
                let _ = sender.send(result);
            });
        if let Err(error) = spawn {
            self.active_jobs.fetch_sub(1, Ordering::AcqRel);
            return Err(InspectionError::WorkerStart(error.to_string()));
        }
        Ok(InspectionJob {
            receiver,
            cancelled,
            progress_receiver,
        })
    }

    /// Starts an explicit bounded project snapshot analysis.
    ///
    /// The host supplies immutable document snapshots; the service does not walk the filesystem
    /// or invoke external tools. Findings are labeled with the supplied snapshot identity.
    ///
    /// # Errors
    /// Returns an error when input limits are exceeded, concurrency is full, or a worker cannot start.
    pub fn inspect_project(
        &self,
        snapshot_id: impl Into<String>,
        documents: Vec<InspectionDocument>,
        profile: InspectionProfile,
        workspace: Option<String>,
    ) -> Result<InspectionJob, InspectionError> {
        if documents.len() > MAX_PROJECT_DOCUMENTS {
            return Err(InspectionError::DocumentLimit(MAX_PROJECT_DOCUMENTS));
        }
        let bytes = documents
            .iter()
            .try_fold(0usize, |sum, document| sum.checked_add(document.text.len()))
            .ok_or(InspectionError::SnapshotTooLarge(
                MAX_PROJECT_SNAPSHOT_BYTES,
            ))?;
        if bytes > MAX_PROJECT_SNAPSHOT_BYTES {
            return Err(InspectionError::SnapshotTooLarge(
                MAX_PROJECT_SNAPSHOT_BYTES,
            ));
        }
        reserve_job(&self.active_jobs, self.max_concurrent_jobs)?;
        let providers = self.providers.values().cloned().collect::<Vec<_>>();
        let active_jobs = self.active_jobs.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let token = CancellationToken(cancelled.clone());
        let (sender, receiver) = mpsc::sync_channel(1);
        let (progress_sender, progress_receiver) = mpsc::sync_channel(32);
        let snapshot_id = snapshot_id.into();
        let total = documents.len();
        let spawn = thread::Builder::new()
            .name("koren-inspection-project".into())
            .spawn(move || {
                let _job = ActiveJob(active_jobs);
                let mut findings = Vec::new();
                let result = (|| {
                    for (index, document) in documents.iter().enumerate() {
                        token.checkpoint()?;
                        let mut current = run_providers(
                            &providers,
                            document,
                            &profile,
                            workspace.as_deref(),
                            &token,
                            &FindingOrigin::ProjectSnapshot {
                                snapshot_id: snapshot_id.clone(),
                            },
                        )?;
                        if findings.len().saturating_add(current.len()) > MAX_JOB_FINDINGS {
                            return Err(InspectionError::ResultLimit(MAX_JOB_FINDINGS));
                        }
                        findings.append(&mut current);
                        let _ = progress_sender.try_send(InspectionProgress {
                            completed_documents: index + 1,
                            total_documents: total,
                            current_path: document.path.clone(),
                        });
                    }
                    findings.sort_by(|left, right| {
                        left.path
                            .cmp(&right.path)
                            .then_with(|| left.range.start.line.cmp(&right.range.start.line))
                    });
                    Ok(deduplicate_provider_duplicates(findings))
                })();
                let _ = sender.send(result);
            });
        if let Err(error) = spawn {
            self.active_jobs.fetch_sub(1, Ordering::AcqRel);
            return Err(InspectionError::WorkerStart(error.to_string()));
        }
        Ok(InspectionJob {
            receiver,
            cancelled,
            progress_receiver,
        })
    }
}

fn reserve_job(active: &AtomicUsize, limit: usize) -> Result<(), InspectionError> {
    let mut current = active.load(Ordering::Acquire);
    loop {
        if current >= limit {
            return Err(InspectionError::ConcurrencyLimit);
        }
        match active.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return Ok(()),
            Err(observed) => current = observed,
        }
    }
}

struct ActiveJob(Arc<AtomicUsize>);
impl Drop for ActiveJob {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn run_providers(
    providers: &[Arc<dyn InspectionProvider>],
    document: &InspectionDocument,
    profile: &InspectionProfile,
    workspace: Option<&str>,
    cancellation: &CancellationToken,
    origin: &FindingOrigin,
) -> Result<Vec<Finding>, InspectionError> {
    let mut findings = Vec::new();
    for provider in providers {
        cancellation.checkpoint()?;
        let applicable = provider
            .rules()
            .iter()
            .any(|rule| rule.languages.is_empty() || rule.languages.contains(&document.language));
        if !applicable {
            continue;
        }
        let result = provider
            .inspect(document, profile, workspace, cancellation)
            .map_err(|message| InspectionError::ProviderFailure {
                provider: provider.id().to_owned(),
                message,
            })?;
        if findings.len().saturating_add(result.len()) > MAX_JOB_FINDINGS {
            return Err(InspectionError::ResultLimit(MAX_JOB_FINDINGS));
        }
        for mut finding in result {
            if finding.provider_id != provider.id() {
                return Err(InspectionError::InvalidFinding(
                    "provider identity mismatch".into(),
                ));
            }
            let rule = provider
                .rules()
                .into_iter()
                .find(|rule| rule.id == finding.rule_id)
                .ok_or_else(|| {
                    InspectionError::InvalidFinding(format!("unknown rule '{}'", finding.rule_id))
                })?;
            if !rule.languages.is_empty() && !rule.languages.contains(&document.language) {
                return Err(InspectionError::InvalidFinding(
                    "provider returned a finding for an unsupported language".into(),
                ));
            }
            let setting = profile.effective_rule(workspace, &rule);
            if !setting.enabled {
                continue;
            }
            if let Some(severity) = setting.severity {
                finding.severity = severity;
            }
            finding.path.clone_from(&document.path);
            finding.document_id = document.document_id;
            finding.document_revision = document.revision;
            finding.origin.clone_from(origin);
            findings.push(finding);
        }
    }
    findings.sort_by(|left, right| {
        (
            &left.path,
            left.range.start.line,
            left.range.start.character,
            &left.provider_id,
            &left.rule_id,
            &left.id,
        )
            .cmp(&(
                &right.path,
                right.range.start.line,
                right.range.start.character,
                &right.provider_id,
                &right.rule_id,
                &right.id,
            ))
    });
    Ok(deduplicate_provider_duplicates(findings))
}

// Equivalent findings are collapsed only when the same provider, rule, location, revision, and
// message repeat. Cross-provider results remain visible and retain attribution.
fn deduplicate_provider_duplicates(findings: Vec<Finding>) -> Vec<Finding> {
    let mut seen = BTreeSet::new();
    findings
        .into_iter()
        .filter(|finding| {
            seen.insert((
                finding.provider_id.clone(),
                finding.rule_id.clone(),
                finding.path.clone(),
                finding.document_revision,
                finding.range.start.line,
                finding.range.start.character,
                finding.range.end.line,
                finding.range.end.character,
                finding.message.clone(),
            ))
        })
        .collect()
}

/// Nonblocking inspection job with cooperative cancellation.
pub struct InspectionJob {
    receiver: Receiver<Result<Vec<Finding>, InspectionError>>,
    cancelled: Arc<AtomicBool>,
    progress_receiver: Receiver<InspectionProgress>,
}

/// Progress snapshot from a project inspection job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectionProgress {
    /// Number of documents whose provider work completed.
    pub completed_documents: usize,
    /// Total documents in the captured project snapshot.
    pub total_documents: usize,
    /// Workspace-relative path of the most recently completed document.
    pub current_path: String,
}

impl InspectionJob {
    /// Requests cooperative cancellation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    /// Checks for a project progress update without waiting.
    #[must_use]
    pub fn try_progress(&self) -> Option<InspectionProgress> {
        self.progress_receiver.try_iter().last()
    }
    /// Waits up to a caller-selected timeout for the completed findings.
    ///
    /// # Errors
    /// Returns provider, cancellation, worker, or timeout failures.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Vec<Finding>, InspectionError> {
        self.receiver
            .recv_timeout(timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => {
                    self.cancelled.store(true, Ordering::Release);
                    InspectionError::RequestTimeout
                }
                mpsc::RecvTimeoutError::Disconnected => InspectionError::WorkerTerminated,
            })?
    }
}

impl Drop for InspectionJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

/// Stable key for suppressing one individual finding in the UI or profile store.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct SuppressionKey {
    /// Provider identity.
    pub provider_id: ProviderId,
    /// Rule identity.
    pub rule_id: RuleId,
    /// Workspace-relative source path.
    pub path: String,
    /// Finding identity supplied by the provider or report adapter.
    pub finding_id: String,
}

/// Generic source-neutral suppression set. It never modifies document text.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SuppressionStore {
    entries: BTreeSet<SuppressionKey>,
}

impl SuppressionStore {
    /// Suppresses one finding in presentation and exported filtered results.
    pub fn suppress(&mut self, key: SuppressionKey) {
        self.entries.insert(key);
    }

    /// Removes a finding suppression.
    pub fn unsuppress(&mut self, key: &SuppressionKey) -> bool {
        self.entries.remove(key)
    }

    /// Returns whether this exact provider/rule/location/finding tuple is suppressed.
    #[must_use]
    pub fn contains(&self, key: &SuppressionKey) -> bool {
        self.entries.contains(key)
    }

    /// Filters individually suppressed findings without altering source files.
    #[must_use]
    pub fn visible<'a>(&self, findings: &'a [Finding]) -> Vec<&'a Finding> {
        findings
            .iter()
            .filter(|finding| {
                !self.contains(&SuppressionKey {
                    provider_id: finding.provider_id.clone(),
                    rule_id: finding.rule_id.clone(),
                    path: finding.path.clone(),
                    finding_id: finding.id.clone(),
                })
            })
            .collect()
    }
}

/// Structured SARIF import outcome. Unsupported but valid fields are reported, not guessed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SarifImportReport {
    /// Accepted findings.
    pub findings: Vec<Finding>,
    /// Unsupported fields or entries.
    pub warnings: Vec<String>,
}

/// SARIF export with explicit reporting of omitted result features.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SarifExportReport {
    /// Serialized SARIF 2.1.0 document.
    pub document: String,
    /// Actions and fields not represented by this adapter.
    pub warnings: Vec<String>,
}

/// Exports findings to the supported SARIF 2.1.0 subset.
///
/// # Errors
/// Returns an error if the result exceeds the size/count bounds or cannot be serialized.
pub fn export_sarif(
    tool_name: &str,
    findings: &[Finding],
) -> Result<SarifExportReport, InspectionError> {
    if findings.len() > MAX_SARIF_RESULTS {
        return Err(InspectionError::ResultLimit(MAX_SARIF_RESULTS));
    }
    let rules = findings
        .iter()
        .map(|finding| finding.rule_id.clone())
        .collect::<BTreeSet<_>>();
    let mut warnings = Vec::new();
    let results = findings
        .iter()
        .map(|finding| {
            let path = normalize_report_path(&finding.path)?;
            let related = finding
                .related
                .iter()
                .enumerate()
                .map(|(index, location)| {
                    Ok(serde_json::json!({
                        "id": index + 1,
                        "physicalLocation": {
                            "artifactLocation": { "uri": normalize_report_path(&location.path)? },
                            "region": sarif_region(location.range),
                        },
                        "message": { "text": location.message },
                    }))
                })
                .collect::<Result<Vec<_>, InspectionError>>()?;
            let mut result = serde_json::json!({
                "ruleId": finding.rule_id,
                "level": sarif_level(finding.severity),
                "message": { "text": finding.message },
                "locations": [{ "physicalLocation": {
                    "artifactLocation": { "uri": path },
                    "region": sarif_region(finding.range),
                }}],
                "properties": { "providerId": finding.provider_id, "korenFindingId": finding.id }
            });
            if let Some(explanation) = &finding.explanation {
                result["properties"]["explanation"] = serde_json::json!(explanation);
            }
            if let Some(category) = &finding.category {
                result["properties"]["category"] = serde_json::json!(category);
            }
            if let Some(revision) = &finding.source_revision {
                result["properties"]["sourceRevision"] = serde_json::json!(revision);
            }
            if let Some(uri) = &finding.documentation {
                result["helpUri"] = serde_json::json!(uri);
            }
            if !related.is_empty() {
                result["relatedLocations"] = serde_json::Value::Array(related);
            }
            if !finding.actions.is_empty() {
                warnings.push(format!(
                    "finding '{}' has code actions not represented in SARIF export",
                    finding.id
                ));
            }
            Ok(result)
        })
        .collect::<Result<Vec<_>, InspectionError>>()?;
    let output = serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{ "tool": { "driver": {
            "name": tool_name,
            "rules": rules.into_iter().map(|id| serde_json::json!({ "id": id })).collect::<Vec<_>>()
        }}, "results": results }]
    });
    let serialized = serde_json::to_string_pretty(&output)
        .map_err(|error| InspectionError::Sarif(error.to_string()))?;
    if serialized.len() > MAX_SARIF_BYTES {
        return Err(InspectionError::SarifSize(MAX_SARIF_BYTES));
    }
    Ok(SarifExportReport {
        document: serialized,
        warnings,
    })
}

/// Imports untrusted SARIF 2.1.0 findings without accessing referenced resources.
///
/// Only relative workspace paths and file URIs inside the workspace are accepted. Unsupported
/// fields are ignored with warnings; invalid core fields reject the import.
///
/// # Errors
/// Returns an error for oversized, malformed, unsupported-version, or unsafe reports.
// Keep envelope, path, and coordinate validation in one import transaction so no partially
// validated results escape to callers.
#[allow(clippy::too_many_lines)]
pub fn import_sarif(
    input: &str,
    report_id: &str,
    workspace_revision: Option<&str>,
) -> Result<SarifImportReport, InspectionError> {
    if input.len() > MAX_SARIF_BYTES {
        return Err(InspectionError::SarifSize(MAX_SARIF_BYTES));
    }
    let value: serde_json::Value =
        serde_json::from_str(input).map_err(|error| InspectionError::Sarif(error.to_string()))?;
    if value.get("version").and_then(serde_json::Value::as_str) != Some("2.1.0") {
        return Err(InspectionError::Sarif(
            "only SARIF 2.1.0 is supported".into(),
        ));
    }
    let runs = value
        .get("runs")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| InspectionError::Sarif("runs must be an array".into()))?;
    let mut findings = Vec::new();
    let mut warnings = Vec::new();
    for (run_index, run) in runs.iter().enumerate() {
        let provider = run
            .pointer("/tool/driver/name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("imported-sarif")
            .to_owned();
        let results = run
            .get("results")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| InspectionError::Sarif("run results must be an array".into()))?;
        if findings.len().saturating_add(results.len()) > MAX_SARIF_RESULTS {
            return Err(InspectionError::ResultLimit(MAX_SARIF_RESULTS));
        }
        for (result_index, result) in results.iter().enumerate() {
            let rule_id = result
                .get("ruleId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| InspectionError::Sarif("result omitted ruleId".into()))?;
            let message = result
                .pointer("/message/text")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| InspectionError::Sarif("result omitted message.text".into()))?;
            let location = result
                .pointer("/locations/0/physicalLocation")
                .ok_or_else(|| {
                    InspectionError::Sarif("result omitted primary physicalLocation".into())
                })?;
            let raw_path = location
                .pointer("/artifactLocation/uri")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| InspectionError::Sarif("result omitted artifact URI".into()))?;
            let path = normalize_report_path(raw_path)?;
            let region = location
                .get("region")
                .ok_or_else(|| InspectionError::Sarif("result omitted region".into()))?;
            let start_line = sarif_coordinate(region, "startLine")?;
            let start_column = optional_sarif_coordinate(region, "startColumn")?.unwrap_or(1);
            let end_line = optional_sarif_coordinate(region, "endLine")?.unwrap_or(start_line);
            let end_column =
                optional_sarif_coordinate(region, "endColumn")?.unwrap_or(start_column);
            if start_line == 0 || end_line == 0 || start_column == 0 || end_column == 0 {
                return Err(InspectionError::Sarif(
                    "SARIF coordinates are one-based positive integers".into(),
                ));
            }
            let severity = match result
                .get("level")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("warning")
            {
                "error" => Severity::Error,
                "warning" => Severity::Warning,
                "note" | "none" => Severity::Information,
                level => {
                    warnings.push(format!(
                        "unsupported SARIF level '{level}' mapped to information"
                    ));
                    Severity::Information
                }
            };
            if result.get("fixes").is_some() || result.get("codeFlows").is_some() {
                warnings.push(format!(
                    "run {run_index} result {result_index}: fixes or codeFlows are unsupported"
                ));
            }
            let mut related = Vec::new();
            if let Some(locations) = result
                .get("relatedLocations")
                .and_then(serde_json::Value::as_array)
            {
                for related_location in locations {
                    let Some(physical) = related_location.get("physicalLocation") else {
                        warnings.push(format!("run {run_index} result {result_index}: related location omitted physical location"));
                        continue;
                    };
                    let Some(related_path) = physical
                        .pointer("/artifactLocation/uri")
                        .and_then(serde_json::Value::as_str)
                    else {
                        warnings.push(format!("run {run_index} result {result_index}: related location omitted artifact URI"));
                        continue;
                    };
                    let Ok(related_path) = normalize_report_path(related_path) else {
                        warnings.push(format!(
                            "run {run_index} result {result_index}: unsafe related location skipped"
                        ));
                        continue;
                    };
                    let Some(related_range) = physical.get("region").and_then(parse_sarif_region)
                    else {
                        warnings.push(format!("run {run_index} result {result_index}: malformed related location skipped"));
                        continue;
                    };
                    related.push(RelatedLocation {
                        path: related_path,
                        range: related_range,
                        message: related_location
                            .pointer("/message/text")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                    });
                }
            }
            let properties = result.get("properties");
            findings.push(Finding {
                id: properties
                    .and_then(|v| v.get("korenFindingId"))
                    .and_then(serde_json::Value::as_str)
                    .map_or_else(
                        || format!("sarif:{run_index}:{result_index}"),
                        str::to_owned,
                    ),
                provider_id: properties
                    .and_then(|v| v.get("providerId"))
                    .and_then(serde_json::Value::as_str)
                    .map_or_else(|| provider.clone(), str::to_owned),
                rule_id: rule_id.to_owned(),
                path,
                document_id: None,
                document_revision: None,
                source_revision: properties
                    .and_then(|v| v.get("sourceRevision"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| workspace_revision.map(str::to_owned)),
                range: TextRange {
                    start: TextPosition {
                        line: start_line - 1,
                        character: start_column - 1,
                    },
                    end: TextPosition {
                        line: end_line - 1,
                        character: end_column - 1,
                    },
                },
                severity,
                message: message.to_owned(),
                explanation: properties
                    .and_then(|v| v.get("explanation"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                category: properties
                    .and_then(|v| v.get("category"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                related,
                actions: Vec::new(),
                documentation: result
                    .get("helpUri")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                origin: FindingOrigin::Imported {
                    report_id: report_id.to_owned(),
                    source_revision: workspace_revision.map(str::to_owned),
                },
            });
        }
    }
    Ok(SarifImportReport { findings, warnings })
}

fn normalize_report_path(path: &str) -> Result<String, InspectionError> {
    let raw = path.strip_prefix("file://").unwrap_or(path);
    if raw.is_empty() || raw.starts_with('/') || raw.contains(':') || raw.contains('\\') {
        return Err(InspectionError::UnsafeReportPath(path.to_owned()));
    }
    let components = Path::new(raw).components().collect::<Vec<_>>();
    if components.iter().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        return Err(InspectionError::UnsafeReportPath(path.to_owned()));
    }
    Ok(components
        .iter()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn sarif_region(range: TextRange) -> serde_json::Value {
    serde_json::json!({ "startLine": range.start.line + 1, "startColumn": range.start.character + 1, "endLine": range.end.line + 1, "endColumn": range.end.character + 1 })
}

fn parse_sarif_region(region: &serde_json::Value) -> Option<TextRange> {
    let start_line = sarif_coordinate(region, "startLine").ok()?;
    let start_column = optional_sarif_coordinate(region, "startColumn")
        .ok()?
        .unwrap_or(1);
    let end_line = optional_sarif_coordinate(region, "endLine")
        .ok()?
        .unwrap_or(start_line);
    let end_column = optional_sarif_coordinate(region, "endColumn")
        .ok()?
        .unwrap_or(start_column);
    if start_line == 0 || end_line == 0 || start_column == 0 || end_column == 0 {
        return None;
    }
    Some(TextRange {
        start: TextPosition {
            line: start_line - 1,
            character: start_column - 1,
        },
        end: TextPosition {
            line: end_line - 1,
            character: end_column - 1,
        },
    })
}
fn sarif_coordinate(region: &serde_json::Value, key: &str) -> Result<usize, InspectionError> {
    region
        .get(key)
        .map(|value| {
            value
                .as_u64()
                .and_then(|number| usize::try_from(number).ok())
                .ok_or_else(|| InspectionError::Sarif(format!("{key} must be a positive integer")))
        })
        .transpose()?
        .ok_or_else(|| InspectionError::Sarif(format!("{key} is required")))
}
fn optional_sarif_coordinate(
    region: &serde_json::Value,
    key: &str,
) -> Result<Option<usize>, InspectionError> {
    region
        .get(key)
        .map(|value| {
            value
                .as_u64()
                .and_then(|number| usize::try_from(number).ok())
                .ok_or_else(|| InspectionError::Sarif(format!("{key} must be a positive integer")))
        })
        .transpose()
}
fn sarif_level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Information | Severity::Hint => "note",
    }
}

fn validate_identifier(id: &str) -> Result<(), InspectionError> {
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return Err(InspectionError::InvalidProvider(
            "identifiers must be 1–256 printable characters".into(),
        ));
    }
    Ok(())
}

/// Inspection service errors.
#[derive(Debug, Eq, PartialEq)]
pub enum InspectionError {
    /// Duplicate provider ID.
    DuplicateProvider(String),
    /// Invalid provider declaration.
    InvalidProvider(String),
    /// Invalid finding returned by a provider.
    InvalidFinding(String),
    /// Invalid profile setting.
    InvalidProfile(String),
    /// Selected profile does not exist.
    UnknownProfile(String),
    /// Concurrency bound is full.
    ConcurrencyLimit,
    /// Project snapshot contains too many documents.
    DocumentLimit(usize),
    /// Project snapshot exceeds its combined text byte bound.
    SnapshotTooLarge(usize),
    /// Job cancelled.
    Cancelled,
    /// Provider reported failure.
    ProviderFailure {
        /// Provider ID.
        provider: String,
        /// Error message.
        message: String,
    },
    /// Provider returned too many findings.
    ResultLimit(usize),
    /// Worker thread could not be started.
    WorkerStart(String),
    /// Worker exited before returning.
    WorkerTerminated,
    /// Inspection job did not finish before the caller timeout.
    RequestTimeout,
    /// Invalid SARIF content.
    Sarif(String),
    /// SARIF exceeded byte limit.
    SarifSize(usize),
    /// SARIF contained too many results.
    SarifResultLimit(usize),
    /// Report path escaped or did not map to a workspace-relative file.
    UnsafeReportPath(String),
}

impl fmt::Display for InspectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for InspectionError {}

#[cfg(test)]
mod tests {
    use super::*;

    struct TextPattern;
    impl InspectionProvider for TextPattern {
        fn id(&self) -> &'static str {
            "example.text"
        }
        fn rules(&self) -> Vec<RuleMetadata> {
            vec![RuleMetadata {
                id: "example.text.no-todo".into(),
                provider_id: self.id().into(),
                title: "No TODO".into(),
                description: "Find TODO markers".into(),
                category: "style".into(),
                languages: BTreeSet::new(),
                default_severity: Severity::Warning,
                supports_fixes: true,
                options: BTreeMap::from([(
                    "enabled_by_default".into(),
                    OptionValue::Boolean(true),
                )]),
            }]
        }
        fn inspect(
            &self,
            document: &InspectionDocument,
            profile: &InspectionProfile,
            workspace: Option<&str>,
            cancellation: &CancellationToken,
        ) -> Result<Vec<Finding>, String> {
            cancellation
                .checkpoint()
                .map_err(|error| error.to_string())?;
            let rule = self.rules().remove(0);
            if !profile.effective_rule(workspace, &rule).enabled {
                return Ok(Vec::new());
            }
            Ok(document
                .text
                .match_indices("TODO")
                .map(|(offset, _)| Finding {
                    id: format!("todo:{offset}"),
                    provider_id: self.id().into(),
                    rule_id: rule.id.clone(),
                    path: document.path.clone(),
                    document_id: document.document_id,
                    document_revision: document.revision,
                    source_revision: None,
                    range: TextRange {
                        start: TextPosition {
                            line: 0,
                            character: offset,
                        },
                        end: TextPosition {
                            line: 0,
                            character: offset + 4,
                        },
                    },
                    severity: rule.default_severity,
                    message: "TODO marker".into(),
                    explanation: Some(rule.description.clone()),
                    category: Some(rule.category.clone()),
                    related: Vec::new(),
                    actions: Vec::new(),
                    documentation: None,
                    origin: FindingOrigin::Document,
                })
                .collect())
        }
    }

    #[test]
    fn profiles_apply_workspace_overrides_and_preserve_unknown_rules() {
        let provider = TextPattern;
        let rule = provider.rules().remove(0);
        let mut profile = InspectionProfile::new("Default");
        profile.rules.insert(
            "other.unavailable.rule".into(),
            RuleProfile {
                enabled: false,
                severity: None,
                options: BTreeMap::new(),
            },
        );
        profile
            .set_rule(
                Some("workspace-a"),
                &rule,
                RuleProfile {
                    enabled: false,
                    severity: Some(Severity::Error),
                    options: BTreeMap::from([(
                        "enabled_by_default".into(),
                        OptionValue::Boolean(false),
                    )]),
                },
            )
            .unwrap();
        assert!(!profile.effective_rule(Some("workspace-a"), &rule).enabled);
        assert_eq!(
            profile.effective_rule(None, &rule).severity,
            Some(Severity::Warning)
        );
        assert_eq!(
            profile
                .effective_rule(None, &rule)
                .options
                .get("enabled_by_default"),
            Some(&OptionValue::Boolean(true))
        );
        assert!(profile.rules.contains_key("other.unavailable.rule"));
    }

    #[test]
    fn profile_selection_supports_global_and_workspace_specific_choices() {
        let mut profiles = InspectionProfiles::default();
        profiles.insert(InspectionProfile::new("Default"));
        profiles.insert(InspectionProfile::new("Strict"));
        profiles.select(None, "Default").unwrap();
        profiles.select(Some("workspace-a"), "Strict").unwrap();
        assert_eq!(profiles.selected_for(None).unwrap().name, "Default");
        assert_eq!(
            profiles.selected_for(Some("workspace-a")).unwrap().name,
            "Strict"
        );
        assert_eq!(
            profiles.selected_for(Some("workspace-b")).unwrap().name,
            "Default"
        );
        assert!(matches!(
            profiles.select(None, "missing"),
            Err(InspectionError::UnknownProfile(_))
        ));
    }

    #[test]
    fn explicit_jobs_return_revisioned_results_and_respect_profiles() {
        let mut service = InspectionService::with_concurrency(1);
        service.register(Arc::new(TextPattern)).unwrap();
        let job = service
            .inspect_document(
                InspectionDocument {
                    path: "src/main.txt".into(),
                    document_id: Some(3),
                    revision: Some(8),
                    language: "plain-text".into(),
                    text: Arc::from("TODO TODO"),
                },
                InspectionProfile::new("Default"),
                None,
            )
            .unwrap();
        let results = job.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].document_revision, Some(8));
        assert_eq!(results[0].provider_id, "example.text");
    }

    #[test]
    fn duplicate_findings_collapse_only_within_the_same_provider() {
        let provider = TextPattern;
        let document = InspectionDocument {
            path: "src/main.txt".into(),
            document_id: Some(3),
            revision: Some(8),
            language: "plain-text".into(),
            text: Arc::from("TODO"),
        };
        let mut first = provider
            .inspect(
                &document,
                &InspectionProfile::new("Default"),
                None,
                &CancellationToken::new(),
            )
            .unwrap();
        let repeated = first[0].clone();
        first.push(repeated);
        let mut other_provider = first[0].clone();
        other_provider.provider_id = "another.provider".into();
        first.push(other_provider);
        let merged = deduplicate_provider_duplicates(first);
        assert_eq!(merged.len(), 2);
        assert_ne!(merged[0].provider_id, merged[1].provider_id);
    }

    #[test]
    fn project_jobs_are_explicit_bounded_revisioned_and_report_progress() {
        let mut service = InspectionService::default();
        service.register(Arc::new(TextPattern)).unwrap();
        let documents = ["a.txt", "b.txt"]
            .map(|path| InspectionDocument {
                path: path.into(),
                document_id: None,
                revision: Some(3),
                language: "plain-text".into(),
                text: Arc::from("TODO"),
            })
            .into();
        let job = service
            .inspect_project(
                "snapshot-1",
                documents,
                InspectionProfile::new("Default"),
                Some("workspace-a".into()),
            )
            .unwrap();
        let findings = job.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(findings.len(), 2);
        assert!(
            matches!(&findings[0].origin, FindingOrigin::ProjectSnapshot { snapshot_id } if snapshot_id == "snapshot-1")
        );
        assert!(job.try_progress().is_some());
        let oversized = vec![InspectionDocument {
            path: "huge".into(),
            document_id: None,
            revision: None,
            language: "plain-text".into(),
            text: Arc::from("x".repeat(MAX_PROJECT_SNAPSHOT_BYTES + 1)),
        }];
        assert!(matches!(
            service.inspect_project(
                "snapshot-2",
                oversized,
                InspectionProfile::new("Default"),
                None
            ),
            Err(InspectionError::SnapshotTooLarge(_))
        ));
    }

    #[test]
    fn individual_suppression_is_provider_scoped_and_never_rewrites_source() {
        let key = SuppressionKey {
            provider_id: "a".into(),
            rule_id: "rule".into(),
            path: "a.txt".into(),
            finding_id: "one".into(),
        };
        let mut suppressions = SuppressionStore::default();
        suppressions.suppress(key.clone());
        assert!(suppressions.contains(&key));
        assert!(suppressions.unsuppress(&key));
        assert!(!suppressions.contains(&key));
    }

    #[test]
    fn sarif_round_trip_preserves_provider_rule_severity_path_and_coordinates() {
        let finding = Finding {
            id: "example-id".into(),
            provider_id: "example.provider".into(),
            rule_id: "example.rule".into(),
            path: "src/main.rs".into(),
            document_id: None,
            document_revision: None,
            source_revision: Some("commit-1".into()),
            range: TextRange {
                start: TextPosition {
                    line: 2,
                    character: 4,
                },
                end: TextPosition {
                    line: 2,
                    character: 8,
                },
            },
            severity: Severity::Error,
            message: "bad text".into(),
            explanation: Some("explanation".into()),
            category: Some("correctness".into()),
            related: vec![RelatedLocation {
                path: "src/other.rs".into(),
                range: TextRange {
                    start: TextPosition {
                        line: 0,
                        character: 1,
                    },
                    end: TextPosition {
                        line: 0,
                        character: 2,
                    },
                },
                message: "related item".into(),
            }],
            actions: vec![CodeAction {
                id: "replace".into(),
                title: "Replace".into(),
                provider_id: "example.provider".into(),
                kind: "quickfix".into(),
                diagnostic_ids: vec!["example-id".into()],
                edits: Vec::new(),
                command: None,
                disabled_reason: None,
            }],
            documentation: None,
            origin: FindingOrigin::Document,
        };
        let sarif = export_sarif("Koren", std::slice::from_ref(&finding)).unwrap();
        assert_eq!(sarif.warnings.len(), 1);
        let imported = import_sarif(&sarif.document, "ci-run", Some("commit-1")).unwrap();
        assert_eq!(imported.findings[0].provider_id, finding.provider_id);
        assert_eq!(imported.findings[0].rule_id, finding.rule_id);
        assert_eq!(imported.findings[0].range, finding.range);
        assert_eq!(imported.findings[0].related, finding.related);
        assert_eq!(
            imported.findings[0].source_revision.as_deref(),
            Some("commit-1")
        );
    }

    #[test]
    fn sarif_rejects_unsafe_paths_and_oversized_reports() {
        let input = r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"CI"}},"results":[{"ruleId":"x","message":{"text":"bad"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"../outside"},"region":{"startLine":1}}}]}]}]}"#;
        assert!(matches!(
            import_sarif(input, "report", None),
            Err(InspectionError::UnsafeReportPath(_))
        ));
        assert!(matches!(
            import_sarif(&" ".repeat(MAX_SARIF_BYTES + 1), "report", None),
            Err(InspectionError::SarifSize(_))
        ));
    }
}
