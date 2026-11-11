//! Revisioned, UTF-8 text documents backed by Ropey and optional local files.

use koren_core::{EventQueue, PlatformEvent};
use koren_protocol::{TextEdit, TextPosition, TextRange};
use koren_workspace::{WorkspaceError, WorkspaceManager};
use ropey::{Rope, RopeSlice};
use std::{
    collections::{BTreeMap, HashMap, hash_map::DefaultHasher},
    fs::{self, File, OpenOptions},
    hash::Hasher,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

static TEMP_ID: AtomicU64 = AtomicU64::new(1);

/// Initial maximum UTF-8 document size, configurable per manager.
pub const DEFAULT_MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;

/// Unique identifier within one [`DocumentManager`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DocumentId(pub u64);

/// Public document metadata without copying the document's text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentInfo {
    /// Manager-local document identifier.
    pub id: DocumentId,
    /// Workspace-relative backing path, or `None` for an untitled document.
    pub path: Option<PathBuf>,
    /// Current monotonically increasing revision.
    pub revision: u64,
    /// Whether the current revision differs from the last saved revision.
    pub dirty: bool,
}

/// Complete immutable read result for one document revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSnapshot {
    /// Document metadata.
    pub info: DocumentInfo,
    /// A copy of the current UTF-8 text.
    pub text: String,
}

/// A specific backing-file conflict encountered by a save.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SaveConflict {
    /// Document whose backing file changed.
    pub document_id: DocumentId,
    /// In-memory revision whose save was rejected.
    pub revision: u64,
    /// Backing path relative to its workspace root.
    pub path: PathBuf,
}

/// Structured document operation failures.
#[derive(Debug)]
pub enum DocumentError {
    /// Identifier does not refer to an open document.
    NotOpen(DocumentId),
    /// Closing a dirty document requires explicit discard authorization.
    UnsavedChanges(DocumentId),
    /// Document has no backing filesystem file to save to.
    NoBackingFile(DocumentId),
    /// File is larger than the configured document size limit.
    TooLarge {
        /// Measured or minimum known file size.
        size_bytes: u64,
        /// Configured maximum size.
        limit_bytes: u64,
    },
    /// File contents are not valid UTF-8.
    InvalidUtf8(PathBuf),
    /// File appears to contain binary data (a NUL byte was found).
    BinaryFile(PathBuf),
    /// Caller provided a stale revision.
    StaleRevision {
        /// Revision supplied by the caller.
        expected: u64,
        /// Current document revision.
        actual: u64,
    },
    /// Position is outside the document or splits a UTF-16 surrogate pair.
    InvalidPosition(TextPosition),
    /// An edit set contains overlapping or ambiguous ranges.
    OverlappingEdits,
    /// Revision counter cannot advance further.
    RevisionExhausted,
    /// Save detected a changed backing file and did not overwrite it.
    Conflict(SaveConflict),
    /// No conflict is available to resolve.
    NoPendingConflict(DocumentId),
    /// Backing file changed again after the conflict was presented.
    ConflictChangedAgain(SaveConflict),
    /// File changed while it was being read for a snapshot or conflict check.
    ChangedDuringRead(PathBuf),
    /// A conflict reload was requested after the backing file was deleted.
    ExternalFileMissing(PathBuf),
    /// Workspace access failed.
    Workspace(WorkspaceError),
    /// Operating system file operation failed.
    Io {
        /// Operation that failed.
        operation: &'static str,
        /// Path involved in the operation.
        path: PathBuf,
        /// Underlying operating system error.
        source: io::Error,
    },
}

impl std::fmt::Display for DocumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotOpen(id) => write!(f, "document {} is not open", id.0),
            Self::UnsavedChanges(id) => write!(f, "document {} has unsaved changes", id.0),
            Self::NoBackingFile(id) => write!(f, "document {} has no backing file", id.0),
            Self::TooLarge {
                size_bytes,
                limit_bytes,
            } => write!(
                f,
                "document size {size_bytes} bytes exceeds limit {limit_bytes} bytes"
            ),
            Self::InvalidUtf8(path) => write!(f, "file '{}' is not valid UTF-8", path.display()),
            Self::BinaryFile(path) => write!(f, "file '{}' contains binary data", path.display()),
            Self::StaleRevision { expected, actual } => write!(
                f,
                "stale document revision {expected}; current revision is {actual}"
            ),
            Self::InvalidPosition(position) => write!(
                f,
                "invalid UTF-16 position ({}, {})",
                position.line, position.character
            ),
            Self::OverlappingEdits => {
                f.write_str("edit ranges overlap or have ambiguous equal starts")
            }
            Self::RevisionExhausted => f.write_str("document revision counter is exhausted"),
            Self::Conflict(conflict) => write!(
                f,
                "backing file '{}' changed externally",
                conflict.path.display()
            ),
            Self::NoPendingConflict(id) => {
                write!(f, "document {} has no pending save conflict", id.0)
            }
            Self::ConflictChangedAgain(conflict) => write!(
                f,
                "backing file '{}' changed again during conflict resolution",
                conflict.path.display()
            ),
            Self::ChangedDuringRead(path) => {
                write!(f, "file '{}' changed while being read", path.display())
            }
            Self::ExternalFileMissing(path) => {
                write!(f, "cannot reload deleted backing file '{}'", path.display())
            }
            Self::Workspace(error) => error.fmt(f),
            Self::Io {
                operation,
                path,
                source,
            } => write!(f, "failed to {operation} '{}': {source}", path.display()),
        }
    }
}

impl std::error::Error for DocumentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Workspace(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<WorkspaceError> for DocumentError {
    fn from(value: WorkspaceError) -> Self {
        Self::Workspace(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    hash: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PendingConflict {
    ExternalVersion(FileStamp),
    Deleted,
}

impl From<Option<FileStamp>> for PendingConflict {
    fn from(value: Option<FileStamp>) -> Self {
        match value {
            Some(stamp) => Self::ExternalVersion(stamp),
            None => Self::Deleted,
        }
    }
}

#[derive(Debug)]
struct Document {
    id: DocumentId,
    workspace_root: Option<PathBuf>,
    path: Option<PathBuf>,
    absolute_path: Option<PathBuf>,
    text: Rope,
    revision: u64,
    saved_revision: u64,
    stamp: Option<FileStamp>,
    pending_conflict: Option<PendingConflict>,
    had_utf8_bom: bool,
}

impl Document {
    fn info(&self) -> DocumentInfo {
        DocumentInfo {
            id: self.id,
            path: self.path.clone(),
            revision: self.revision,
            dirty: self.revision != self.saved_revision,
        }
    }
}

/// Manages a set of independent in-memory documents and their backing files.
///
/// Opening an already-open path returns its existing identifier, ensuring that
/// one manager never creates two writable buffers for the same file.
#[derive(Debug)]
pub struct DocumentManager {
    documents: BTreeMap<DocumentId, Document>,
    paths: HashMap<PathBuf, DocumentId>,
    next_id: u64,
    max_document_bytes: u64,
    events: EventQueue,
}

impl Default for DocumentManager {
    fn default() -> Self {
        Self::with_max_document_bytes(DEFAULT_MAX_DOCUMENT_BYTES)
    }
}

impl DocumentManager {
    /// Creates a manager with a 16 MiB maximum document size.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a manager with a caller-selected UTF-8 file size limit.
    #[must_use]
    pub fn with_max_document_bytes(max_document_bytes: u64) -> Self {
        Self {
            documents: BTreeMap::new(),
            paths: HashMap::new(),
            next_id: 1,
            max_document_bytes,
            events: EventQueue::new(),
        }
    }

    /// Opens a workspace-relative text file. Reopening a path returns its existing document id.
    ///
    /// UTF-8 BOM is removed from the in-memory text and restored when saving.
    /// LF and CRLF bytes are preserved exactly. Files containing NUL bytes or
    /// invalid UTF-8 are rejected without lossy conversion.
    ///
    /// # Errors
    ///
    /// Returns errors for workspace path violations, read failures, binary or
    /// invalid UTF-8 content, or files above this manager's size limit.
    pub fn open_file(
        &mut self,
        workspace: &WorkspaceManager,
        relative: impl AsRef<Path>,
    ) -> Result<DocumentId, DocumentError> {
        let absolute = workspace.resolve_file(relative)?;
        if let Some(id) = self.paths.get(&absolute) {
            return Ok(*id);
        }
        let root = workspace.root()?.to_path_buf();
        let relative = absolute
            .strip_prefix(&root)
            .map_err(|_| {
                DocumentError::Workspace(WorkspaceError::InvalidRelativePath(absolute.clone()))
            })?
            .to_path_buf();
        let (text, had_utf8_bom, stamp) = read_text_file(&absolute, self.max_document_bytes)?;
        let id = self.allocate_id()?;
        let document = Document {
            id,
            workspace_root: Some(root.clone()),
            path: Some(relative.clone()),
            absolute_path: Some(absolute.clone()),
            text: Rope::from_str(&text),
            revision: 0,
            saved_revision: 0,
            stamp: Some(stamp),
            pending_conflict: None,
            had_utf8_bom,
        };
        self.paths.insert(absolute, id);
        self.documents.insert(id, document);
        self.events.push(PlatformEvent::DocumentOpened {
            document_id: id.0,
            workspace_root: Some(root),
            path: Some(relative),
            revision: 0,
        });
        Ok(id)
    }

    /// Creates an untitled, initially empty document without a backing path.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::RevisionExhausted`] if the manager has exhausted
    /// its identifier space.
    pub fn create_untitled(&mut self) -> Result<DocumentId, DocumentError> {
        let id = self.allocate_id()?;
        self.documents.insert(
            id,
            Document {
                id,
                workspace_root: None,
                path: None,
                absolute_path: None,
                text: Rope::new(),
                revision: 0,
                saved_revision: 0,
                stamp: None,
                pending_conflict: None,
                had_utf8_bom: false,
            },
        );
        self.events.push(PlatformEvent::DocumentOpened {
            document_id: id.0,
            workspace_root: None,
            path: None,
            revision: 0,
        });
        Ok(id)
    }

    /// Returns metadata for an open document.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::NotOpen`] for an unknown id.
    pub fn info(&self, id: DocumentId) -> Result<DocumentInfo, DocumentError> {
        Ok(self.document(id)?.info())
    }

    /// Returns a complete document snapshot. This copies the current text into a `String`.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::NotOpen`] for an unknown id.
    pub fn snapshot(&self, id: DocumentId) -> Result<DocumentSnapshot, DocumentError> {
        let document = self.document(id)?;
        Ok(DocumentSnapshot {
            info: document.info(),
            text: document.text.to_string(),
        })
    }

    /// Converts a UTF-16 line/column position to Ropey's Unicode scalar index.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::InvalidPosition`] for an out-of-range line or
    /// column, or for a column splitting an astral scalar's surrogate pair.
    pub fn position_to_char(
        &self,
        id: DocumentId,
        position: TextPosition,
    ) -> Result<usize, DocumentError> {
        let document = self.document(id)?;
        position_to_char(&document.text, position)
    }

    /// Converts a Unicode scalar index to a UTF-16 line/column position.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::InvalidPosition`] if the scalar index is beyond
    /// the end of the document.
    pub fn char_to_position(
        &self,
        id: DocumentId,
        char_index: usize,
    ) -> Result<TextPosition, DocumentError> {
        let document = self.document(id)?;
        char_to_position(&document.text, char_index)
    }

    /// Inserts UTF-8 text at a UTF-16 editor position, conditional on a revision.
    ///
    /// # Errors
    ///
    /// Returns the validation errors documented by [`Self::apply_edits`].
    pub fn insert_text(
        &mut self,
        id: DocumentId,
        expected_revision: u64,
        at: TextPosition,
        text: &str,
    ) -> Result<DocumentInfo, DocumentError> {
        self.apply_edits(
            id,
            expected_revision,
            &[TextEdit {
                range: TextRange { start: at, end: at },
                new_text: text.to_owned(),
            }],
        )
    }

    /// Deletes a half-open UTF-16 range, conditional on a revision.
    ///
    /// # Errors
    ///
    /// Returns the validation errors documented by [`Self::apply_edits`].
    pub fn delete_range(
        &mut self,
        id: DocumentId,
        expected_revision: u64,
        range: TextRange,
    ) -> Result<DocumentInfo, DocumentError> {
        self.apply_edits(
            id,
            expected_revision,
            &[TextEdit {
                range,
                new_text: String::new(),
            }],
        )
    }

    /// Replaces a half-open UTF-16 range, conditional on a revision.
    ///
    /// # Errors
    ///
    /// Returns the validation errors documented by [`Self::apply_edits`].
    pub fn replace_range(
        &mut self,
        id: DocumentId,
        expected_revision: u64,
        range: TextRange,
        text: &str,
    ) -> Result<DocumentInfo, DocumentError> {
        self.apply_edits(
            id,
            expected_revision,
            &[TextEdit {
                range,
                new_text: text.to_owned(),
            }],
        )
    }

    /// Applies a non-overlapping edit set atomically against one revision.
    ///
    /// Ranges are interpreted against the original revision, sorted by their
    /// scalar offsets, and applied from end to start. Equal start positions are
    /// rejected to avoid ambiguous insertion ordering. Invalid input leaves the
    /// document unchanged. A non-empty successful edit batch advances one revision.
    ///
    /// # Errors
    ///
    /// Returns errors for an unknown document, stale revision, invalid UTF-16
    /// position, overlapping edit ranges, revision exhaustion, or size limit.
    pub fn apply_edits(
        &mut self,
        id: DocumentId,
        expected_revision: u64,
        edits: &[TextEdit],
    ) -> Result<DocumentInfo, DocumentError> {
        let max_document_bytes = self.max_document_bytes;
        let document = self
            .documents
            .get_mut(&id)
            .ok_or(DocumentError::NotOpen(id))?;
        check_revision(document, expected_revision)?;
        let mut converted = Vec::with_capacity(edits.len());
        for edit in edits {
            let start = position_to_char(&document.text, edit.range.start)?;
            let end = position_to_char(&document.text, edit.range.end)?;
            if end < start {
                return Err(DocumentError::InvalidPosition(edit.range.end));
            }
            converted.push((start, end, edit.new_text.clone()));
        }
        converted.sort_by_key(|(start, end, _)| (*start, *end));
        for pair in converted.windows(2) {
            if pair[1].0 < pair[0].1 || pair[1].0 == pair[0].0 {
                return Err(DocumentError::OverlappingEdits);
            }
        }
        if converted.is_empty() {
            return Ok(document.info());
        }
        let old_bytes = converted
            .iter()
            .map(|(start, end, _)| {
                i128::try_from(document.text.slice(*start..*end).len_bytes()).unwrap_or(i128::MAX)
            })
            .sum::<i128>();
        let inserted_bytes = converted
            .iter()
            .map(|(_, _, text)| i128::try_from(text.len()).unwrap_or(i128::MAX))
            .sum::<i128>();
        let current_bytes = i128::try_from(document.text.len_bytes()).unwrap_or(i128::MAX);
        let new_bytes = current_bytes - old_bytes + inserted_bytes;
        if new_bytes > i128::from(max_document_bytes) {
            return Err(DocumentError::TooLarge {
                size_bytes: u64::try_from(new_bytes).unwrap_or(u64::MAX),
                limit_bytes: max_document_bytes,
            });
        }
        let revision = document
            .revision
            .checked_add(1)
            .ok_or(DocumentError::RevisionExhausted)?;
        for (start, end, text) in converted.into_iter().rev() {
            if start < end {
                document.text.remove(start..end);
            }
            if !text.is_empty() {
                document.text.insert(start, &text);
            }
        }
        document.revision = revision;
        document.pending_conflict = None;
        let info = document.info();
        self.events.push(PlatformEvent::DocumentChanged {
            document_id: id.0,
            revision,
        });
        Ok(info)
    }

    /// Saves a document after checking its expected revision and backing file.
    ///
    /// If the file changed since it was loaded or last saved, the operation
    /// returns [`DocumentError::Conflict`] and does not overwrite it. Use
    /// [`Self::resolve_conflict`] after reviewing the external version.
    ///
    /// # Errors
    ///
    /// Returns errors for stale revisions, untitled documents, external
    /// conflicts, oversized files, or filesystem failures.
    pub fn save(
        &mut self,
        id: DocumentId,
        expected_revision: u64,
    ) -> Result<DocumentInfo, DocumentError> {
        let max_document_bytes = self.max_document_bytes;
        let document = self
            .documents
            .get_mut(&id)
            .ok_or(DocumentError::NotOpen(id))?;
        check_revision(document, expected_revision)?;
        let path = document
            .absolute_path
            .clone()
            .ok_or(DocumentError::NoBackingFile(id))?;
        let current = read_stamp_optional(&path, max_document_bytes)?;
        if document.stamp.as_ref() != current.as_ref() {
            document.pending_conflict = Some(current.into());
            let conflict = SaveConflict {
                document_id: id,
                revision: document.revision,
                path: document.path.clone().unwrap_or_default(),
            };
            self.events.push(PlatformEvent::SaveConflictDetected {
                document_id: id.0,
                revision: document.revision,
            });
            return Err(DocumentError::Conflict(conflict));
        }
        let expected_disk = document.stamp.clone();
        if let ReplaceOutcome::Conflict(changed) =
            write_document(document, &path, expected_disk.as_ref(), max_document_bytes)?
        {
            document.pending_conflict = Some(changed.into());
            let conflict = SaveConflict {
                document_id: id,
                revision: document.revision,
                path: document.path.clone().unwrap_or_default(),
            };
            self.events.push(PlatformEvent::SaveConflictDetected {
                document_id: id.0,
                revision: document.revision,
            });
            return Err(DocumentError::Conflict(conflict));
        }
        document.stamp = Some(read_stamp(&path, max_document_bytes)?);
        document.saved_revision = document.revision;
        document.pending_conflict = None;
        let info = document.info();
        self.events.push(PlatformEvent::DocumentSaved {
            document_id: id.0,
            revision: document.revision,
        });
        Ok(info)
    }

    /// Reads the bounded external file contents shown for a pending save conflict.
    ///
    /// Returns `None` when the file was deleted. This is a preview only; the later
    /// conflict decision rechecks that the disk version has not changed again.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::NoPendingConflict`] if no save conflict is pending,
    /// or the same size, encoding, and filesystem errors as a document read.
    pub fn conflict_contents(&self, id: DocumentId) -> Result<Option<String>, DocumentError> {
        let document = self.document(id)?;
        if document.pending_conflict.is_none() {
            return Err(DocumentError::NoPendingConflict(id));
        }
        let path = document
            .absolute_path
            .as_ref()
            .ok_or(DocumentError::NoBackingFile(id))?;
        match read_text_file(path, self.max_document_bytes) {
            Ok((contents, _, _)) => Ok(Some(contents)),
            Err(DocumentError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Resolves a previously detected external modification.
    ///
    /// `Overwrite` writes the in-memory revision only if the file still matches
    /// the exact external version observed at conflict detection. `Reload`
    /// replaces the buffer with that observed version and advances its revision.
    ///
    /// # Errors
    ///
    /// Returns errors if no conflict is pending, the revision is stale, the
    /// backing file changed again, or filesystem operations fail.
    pub fn resolve_conflict(
        &mut self,
        id: DocumentId,
        expected_revision: u64,
        decision: koren_protocol::ConflictDecision,
    ) -> Result<DocumentInfo, DocumentError> {
        let max_document_bytes = self.max_document_bytes;
        let document = self
            .documents
            .get_mut(&id)
            .ok_or(DocumentError::NotOpen(id))?;
        check_revision(document, expected_revision)?;
        let expected_disk = document
            .pending_conflict
            .clone()
            .ok_or(DocumentError::NoPendingConflict(id))?;
        let path = document
            .absolute_path
            .clone()
            .ok_or(DocumentError::NoBackingFile(id))?;
        let current = read_stamp_optional(&path, max_document_bytes)?;
        let current_state = PendingConflict::from(current.clone());
        if current_state != expected_disk {
            document.pending_conflict = Some(current_state);
            let conflict = SaveConflict {
                document_id: id,
                revision: document.revision,
                path: document.path.clone().unwrap_or_default(),
            };
            self.events.push(PlatformEvent::SaveConflictDetected {
                document_id: id.0,
                revision: document.revision,
            });
            return Err(DocumentError::ConflictChangedAgain(conflict));
        }
        match decision {
            koren_protocol::ConflictDecision::Overwrite => {
                let expected_stamp = match &expected_disk {
                    PendingConflict::ExternalVersion(stamp) => Some(stamp),
                    PendingConflict::Deleted => None,
                };
                if let ReplaceOutcome::Conflict(changed) =
                    write_document(document, &path, expected_stamp, max_document_bytes)?
                {
                    document.pending_conflict = Some(changed.into());
                    let conflict = SaveConflict {
                        document_id: id,
                        revision: document.revision,
                        path: document.path.clone().unwrap_or_default(),
                    };
                    self.events.push(PlatformEvent::SaveConflictDetected {
                        document_id: id.0,
                        revision: document.revision,
                    });
                    return Err(DocumentError::ConflictChangedAgain(conflict));
                }
                document.stamp = Some(read_stamp(&path, max_document_bytes)?);
                document.saved_revision = document.revision;
                document.pending_conflict = None;
                self.events.push(PlatformEvent::DocumentSaved {
                    document_id: id.0,
                    revision: document.revision,
                });
            }
            koren_protocol::ConflictDecision::Reload => {
                if expected_disk == PendingConflict::Deleted {
                    return Err(DocumentError::ExternalFileMissing(path));
                }
                let (text, bom, stamp) = read_text_file(&path, max_document_bytes)?;
                document.revision = document
                    .revision
                    .checked_add(1)
                    .ok_or(DocumentError::RevisionExhausted)?;
                document.text = Rope::from_str(&text);
                document.saved_revision = document.revision;
                document.stamp = Some(stamp);
                document.pending_conflict = None;
                document.had_utf8_bom = bom;
                self.events.push(PlatformEvent::DocumentChanged {
                    document_id: id.0,
                    revision: document.revision,
                });
            }
        }
        Ok(document.info())
    }

    /// Closes a document; dirty buffers require explicit discard authorization.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::UnsavedChanges`] unless `discard_unsaved_changes`
    /// is true, or [`DocumentError::NotOpen`] for an unknown id.
    pub fn close(
        &mut self,
        id: DocumentId,
        discard_unsaved_changes: bool,
    ) -> Result<(), DocumentError> {
        let document = self.document(id)?;
        if document.info().dirty && !discard_unsaved_changes {
            return Err(DocumentError::UnsavedChanges(id));
        }
        let document = self
            .documents
            .remove(&id)
            .ok_or(DocumentError::NotOpen(id))?;
        if let Some(path) = document.absolute_path {
            self.paths.remove(&path);
        }
        self.events
            .push(PlatformEvent::DocumentClosed { document_id: id.0 });
        Ok(())
    }

    /// Returns whether the manager contains a document associated with a workspace root.
    #[must_use]
    pub fn has_documents_for_workspace(&self, root: &Path) -> bool {
        self.documents
            .values()
            .any(|document| document.workspace_root.as_deref() == Some(root))
    }

    /// Returns the number of open documents.
    #[must_use]
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    /// Returns whether this manager has no open documents.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    /// Drains document lifecycle and edit events in FIFO order.
    pub fn drain_events(&mut self) -> Vec<PlatformEvent> {
        self.events.drain()
    }

    fn allocate_id(&mut self) -> Result<DocumentId, DocumentError> {
        let id = DocumentId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(DocumentError::RevisionExhausted)?;
        Ok(id)
    }

    fn document(&self, id: DocumentId) -> Result<&Document, DocumentError> {
        self.documents.get(&id).ok_or(DocumentError::NotOpen(id))
    }
}

fn check_revision(document: &Document, expected: u64) -> Result<(), DocumentError> {
    if document.revision == expected {
        Ok(())
    } else {
        Err(DocumentError::StaleRevision {
            expected,
            actual: document.revision,
        })
    }
}

fn position_to_char(text: &Rope, position: TextPosition) -> Result<usize, DocumentError> {
    if position.line >= text.len_lines() {
        return Err(DocumentError::InvalidPosition(position));
    }
    let line_start = text.line_to_char(position.line);
    let line = text.line(position.line);
    let content_chars = line_content_chars(line);
    let content_end = line_start + content_chars;
    let line_utf16_start = text.char_to_utf16_cu(line_start);
    let requested_utf16 = line_utf16_start
        .checked_add(position.character)
        .ok_or(DocumentError::InvalidPosition(position))?;
    let content_utf16_end = text.char_to_utf16_cu(content_end);
    if requested_utf16 > content_utf16_end {
        return Err(DocumentError::InvalidPosition(position));
    }
    let char_index = text
        .try_utf16_cu_to_char(requested_utf16)
        .map_err(|_| DocumentError::InvalidPosition(position))?;
    if char_index > content_end || text.char_to_utf16_cu(char_index) != requested_utf16 {
        return Err(DocumentError::InvalidPosition(position));
    }
    Ok(char_index)
}

fn char_to_position(text: &Rope, char_index: usize) -> Result<TextPosition, DocumentError> {
    if char_index > text.len_chars() {
        return Err(DocumentError::InvalidPosition(TextPosition {
            line: text.len_lines(),
            character: 0,
        }));
    }
    let line_index = text.char_to_line(char_index);
    let line_start = text.line_to_char(line_index);
    let relative_char = char_index - line_start;
    let line = text.line(line_index);
    let content_chars = line_content_chars(line);
    if relative_char > content_chars {
        return Err(DocumentError::InvalidPosition(TextPosition {
            line: line_index,
            character: 0,
        }));
    }
    let character = text.char_to_utf16_cu(char_index) - text.char_to_utf16_cu(line_start);
    Ok(TextPosition {
        line: line_index,
        character,
    })
}

fn line_content_chars(line: RopeSlice<'_>) -> usize {
    let mut chars = line.len_chars();
    if chars > 0 && line.char(chars - 1) == '\n' {
        chars -= 1;
        if chars > 0 && line.char(chars - 1) == '\r' {
            chars -= 1;
        }
    }
    chars
}

fn read_text_file(path: &Path, max_bytes: u64) -> Result<(String, bool, FileStamp), DocumentError> {
    let before = fs::metadata(path).map_err(|source| io_error("inspect", path, source))?;
    if before.len() > max_bytes {
        return Err(DocumentError::TooLarge {
            size_bytes: before.len(),
            limit_bytes: max_bytes,
        });
    }
    let bytes = fs::read(path).map_err(|source| io_error("read", path, source))?;
    let after = fs::metadata(path).map_err(|source| io_error("inspect", path, source))?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(DocumentError::ChangedDuringRead(path.to_path_buf()));
    }
    if bytes.len() as u64 > max_bytes {
        return Err(DocumentError::TooLarge {
            size_bytes: bytes.len() as u64,
            limit_bytes: max_bytes,
        });
    }
    if bytes.contains(&0) {
        return Err(DocumentError::BinaryFile(path.to_path_buf()));
    }
    let stamp = stamp_for(&after, &bytes);
    let (bytes, had_bom) = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        (&bytes[3..], true)
    } else {
        (&bytes[..], false)
    };
    let text = std::str::from_utf8(bytes)
        .map_err(|_| DocumentError::InvalidUtf8(path.to_path_buf()))?
        .to_owned();
    Ok((text, had_bom, stamp))
}

fn read_stamp(path: &Path, max_bytes: u64) -> Result<FileStamp, DocumentError> {
    let before = fs::metadata(path).map_err(|source| io_error("inspect", path, source))?;
    if before.len() > max_bytes {
        return Err(DocumentError::TooLarge {
            size_bytes: before.len(),
            limit_bytes: max_bytes,
        });
    }
    let mut file = File::open(path).map_err(|source| io_error("read", path, source))?;
    let mut hasher = DefaultHasher::new();
    let mut buffer = [0u8; 16 * 1024];
    let mut length = 0u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|source| io_error("read", path, source))?;
        if count == 0 {
            break;
        }
        length += count as u64;
        if length > max_bytes {
            return Err(DocumentError::TooLarge {
                size_bytes: length,
                limit_bytes: max_bytes,
            });
        }
        hasher.write(&buffer[..count]);
    }
    let after = file
        .metadata()
        .map_err(|source| io_error("inspect", path, source))?;
    if before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || length != after.len()
    {
        return Err(DocumentError::ChangedDuringRead(path.to_path_buf()));
    }
    Ok(FileStamp {
        len: after.len(),
        modified: after.modified().ok(),
        hash: hasher.finish(),
    })
}

fn read_stamp_optional(path: &Path, max_bytes: u64) -> Result<Option<FileStamp>, DocumentError> {
    match read_stamp(path, max_bytes) {
        Ok(stamp) => Ok(Some(stamp)),
        Err(DocumentError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn stamp_for(metadata: &fs::Metadata, bytes: &[u8]) -> FileStamp {
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    FileStamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        hash: hasher.finish(),
    }
}

enum ReplaceOutcome {
    Saved,
    Conflict(Option<FileStamp>),
}

fn write_document(
    document: &Document,
    path: &Path,
    expected_disk: Option<&FileStamp>,
    max_document_bytes: u64,
) -> Result<ReplaceOutcome, DocumentError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(source) => return Err(io_error("inspect before save", path, source)),
    };
    if metadata
        .as_ref()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(DocumentError::Io {
            operation: "save through symbolic link",
            path: path.to_path_buf(),
            source: io::Error::new(
                io::ErrorKind::PermissionDenied,
                "symbolic link backing path is not allowed",
            ),
        });
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut temp_path = None;
    let mut temp_file = None;
    for _ in 0..100 {
        let sequence = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{file_name}.koren-{}-{sequence}.tmp",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temp_path = Some(candidate);
                temp_file = Some(file);
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => return Err(io_error("create save temporary file", &candidate, source)),
        }
    }
    let temp_path = temp_path.ok_or_else(|| DocumentError::Io {
        operation: "create unique save temporary file",
        path: parent.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary filename attempts exhausted",
        ),
    })?;
    let mut temp_file = temp_file.expect("temporary file path and handle are created together");
    let result = (|| {
        if document.had_utf8_bom {
            temp_file
                .write_all(&[0xef, 0xbb, 0xbf])
                .map_err(|source| io_error("write temporary file", &temp_path, source))?;
        }
        document
            .text
            .write_to(&mut temp_file)
            .map_err(|source| io_error("write temporary file", &temp_path, source))?;
        if let Some(metadata) = &metadata {
            temp_file
                .set_permissions(metadata.permissions())
                .map_err(|source| io_error("preserve file permissions", &temp_path, source))?;
        }
        temp_file
            .sync_all()
            .map_err(|source| io_error("sync temporary file", &temp_path, source))?;
        drop(temp_file);
        let current = read_stamp_optional(path, max_document_bytes)?;
        if current.as_ref() != expected_disk {
            return Ok(ReplaceOutcome::Conflict(current));
        }
        fs::rename(&temp_path, path)
            .map_err(|source| io_error("replace backing file", path, source))?;
        Ok(ReplaceOutcome::Saved)
    })();
    if !matches!(result, Ok(ReplaceOutcome::Saved)) {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> DocumentError {
    DocumentError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "koren-documents-test-{}-{}",
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
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn setup(content: &[u8]) -> (TempDir, WorkspaceManager, DocumentManager, DocumentId) {
        let temp = TempDir::new();
        fs::write(temp.0.join("source.rs"), content).unwrap();
        let mut workspace = WorkspaceManager::new();
        workspace.open(&temp.0).unwrap();
        let mut documents = DocumentManager::new();
        let id = documents.open_file(&workspace, "source.rs").unwrap();
        (temp, workspace, documents, id)
    }

    #[test]
    fn opening_same_file_reuses_single_buffer_and_bom_is_preserved() {
        let (_temp, workspace, mut documents, id) = setup(b"\xef\xbb\xbfhello\r\n");
        assert_eq!(documents.open_file(&workspace, "source.rs").unwrap(), id);
        assert_eq!(documents.snapshot(id).unwrap().text, "hello\r\n");
        assert!(matches!(
            documents.drain_events().as_slice(),
            [PlatformEvent::DocumentOpened {
                document_id: 1,
                revision: 0,
                ..
            }]
        ));
        documents
            .insert_text(
                id,
                0,
                TextPosition {
                    line: 0,
                    character: 5,
                },
                "!",
            )
            .unwrap();
        documents.save(id, 1).unwrap();
        assert!(matches!(
            documents.drain_events().as_slice(),
            [
                PlatformEvent::DocumentChanged { revision: 1, .. },
                PlatformEvent::DocumentSaved { revision: 1, .. }
            ]
        ));
        assert_eq!(
            fs::read(workspace.root().unwrap().join("source.rs")).unwrap(),
            b"\xef\xbb\xbfhello!\r\n"
        );
    }

    #[test]
    fn utf16_positions_handle_emoji_unicode_empty_and_newlines() {
        let (_temp, _workspace, mut documents, id) = setup("a😀é\r\nz\n".as_bytes());
        assert_eq!(
            documents
                .position_to_char(
                    id,
                    TextPosition {
                        line: 0,
                        character: 3
                    }
                )
                .unwrap(),
            2
        );
        assert!(matches!(
            documents.position_to_char(
                id,
                TextPosition {
                    line: 0,
                    character: 2
                }
            ),
            Err(DocumentError::InvalidPosition(_))
        ));
        assert_eq!(
            documents.char_to_position(id, 2).unwrap(),
            TextPosition {
                line: 0,
                character: 3
            }
        );
        assert_eq!(
            documents
                .position_to_char(
                    id,
                    TextPosition {
                        line: 1,
                        character: 0
                    }
                )
                .unwrap(),
            5
        );
        let empty = documents.create_untitled().unwrap();
        assert_eq!(
            documents
                .position_to_char(
                    empty,
                    TextPosition {
                        line: 0,
                        character: 0
                    }
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn edits_are_revisioned_atomic_and_reject_stale_or_overlapping_ranges() {
        let (_temp, _workspace, mut documents, id) = setup(b"abcdef");
        let edit = |start, end, new_text: &str| TextEdit {
            range: TextRange {
                start: TextPosition {
                    line: 0,
                    character: start,
                },
                end: TextPosition {
                    line: 0,
                    character: end,
                },
            },
            new_text: new_text.into(),
        };
        let info = documents
            .apply_edits(id, 0, &[edit(0, 1, "A"), edit(4, 6, "EF")])
            .unwrap();
        assert_eq!(info.revision, 1);
        assert!(info.dirty);
        assert_eq!(documents.snapshot(id).unwrap().text, "AbcdEF");
        let before = documents.snapshot(id).unwrap();
        assert!(matches!(
            documents.apply_edits(id, 0, &[edit(0, 0, "x")]),
            Err(DocumentError::StaleRevision { .. })
        ));
        assert!(matches!(
            documents.apply_edits(id, 1, &[edit(1, 4, "x"), edit(2, 3, "y")]),
            Err(DocumentError::OverlappingEdits)
        ));
        assert_eq!(documents.snapshot(id).unwrap(), before);
    }

    #[test]
    fn detects_external_changes_and_requires_explicit_resolution() {
        let (temp, _workspace, mut documents, id) = setup(b"initial");
        assert!(matches!(
            documents.conflict_contents(id),
            Err(DocumentError::NoPendingConflict(document_id)) if document_id == id
        ));
        documents
            .insert_text(
                id,
                0,
                TextPosition {
                    line: 0,
                    character: 7,
                },
                " local",
            )
            .unwrap();
        fs::write(temp.0.join("source.rs"), "external").unwrap();
        assert!(matches!(
            documents.save(id, 1),
            Err(DocumentError::Conflict(_))
        ));
        assert_eq!(
            documents.conflict_contents(id).unwrap().as_deref(),
            Some("external")
        );
        assert_eq!(fs::read(temp.0.join("source.rs")).unwrap(), b"external");
        documents
            .resolve_conflict(id, 1, koren_protocol::ConflictDecision::Overwrite)
            .unwrap();
        assert_eq!(
            fs::read(temp.0.join("source.rs")).unwrap(),
            b"initial local"
        );
        assert!(!documents.info(id).unwrap().dirty);
    }

    #[test]
    fn a_second_external_change_requires_a_new_decision_and_reload_is_revisioned() {
        let (temp, _workspace, mut documents, id) = setup(b"initial");
        documents
            .insert_text(
                id,
                0,
                TextPosition {
                    line: 0,
                    character: 7,
                },
                " local",
            )
            .unwrap();
        fs::write(temp.0.join("source.rs"), "external one").unwrap();
        assert!(matches!(
            documents.save(id, 1),
            Err(DocumentError::Conflict(_))
        ));
        fs::write(temp.0.join("source.rs"), "external two").unwrap();
        assert!(matches!(
            documents.resolve_conflict(id, 1, koren_protocol::ConflictDecision::Overwrite),
            Err(DocumentError::ConflictChangedAgain(_))
        ));
        let info = documents
            .resolve_conflict(id, 1, koren_protocol::ConflictDecision::Reload)
            .unwrap();
        assert_eq!(info.revision, 2);
        assert!(!info.dirty);
        assert_eq!(documents.snapshot(id).unwrap().text, "external two");
    }

    #[test]
    fn failed_save_keeps_the_buffer_dirty() {
        let (temp, _workspace, mut documents, id) = setup(b"initial");
        documents
            .insert_text(
                id,
                0,
                TextPosition {
                    line: 0,
                    character: 7,
                },
                " edited",
            )
            .unwrap();
        fs::remove_file(temp.0.join("source.rs")).unwrap();
        assert!(matches!(
            documents.save(id, 1),
            Err(DocumentError::Conflict(_))
        ));
        assert!(documents.info(id).unwrap().dirty);
        assert_eq!(documents.snapshot(id).unwrap().text, "initial edited");
    }

    #[test]
    fn deleted_backing_file_requires_an_explicit_overwrite_decision() {
        let (temp, _workspace, mut documents, id) = setup(b"original");
        documents
            .insert_text(
                id,
                0,
                TextPosition {
                    line: 0,
                    character: 8,
                },
                " edited",
            )
            .unwrap();
        fs::remove_file(temp.0.join("source.rs")).unwrap();
        assert!(matches!(
            documents.save(id, 1),
            Err(DocumentError::Conflict(_))
        ));
        assert!(!temp.0.join("source.rs").exists());
        assert!(matches!(
            documents.resolve_conflict(id, 1, koren_protocol::ConflictDecision::Reload),
            Err(DocumentError::ExternalFileMissing(_))
        ));
        documents
            .resolve_conflict(id, 1, koren_protocol::ConflictDecision::Overwrite)
            .unwrap();
        assert_eq!(
            fs::read(temp.0.join("source.rs")).unwrap(),
            b"original edited"
        );
        assert!(!documents.info(id).unwrap().dirty);
    }

    #[test]
    fn close_requires_discard_for_dirty_document_and_untitled_cannot_save() {
        let (_temp, _workspace, mut documents, id) = setup(b"x");
        documents
            .insert_text(
                id,
                0,
                TextPosition {
                    line: 0,
                    character: 1,
                },
                "y",
            )
            .unwrap();
        assert!(matches!(
            documents.close(id, false),
            Err(DocumentError::UnsavedChanges(_))
        ));
        let untitled = documents.create_untitled().unwrap();
        assert!(matches!(
            documents.save(untitled, 0),
            Err(DocumentError::NoBackingFile(_))
        ));
        documents.close(id, true).unwrap();
    }

    #[test]
    fn invalid_utf8_binary_and_oversized_files_are_rejected() {
        let temp = TempDir::new();
        fs::write(temp.0.join("bad.txt"), [0xff, 0xfe]).unwrap();
        fs::write(temp.0.join("binary"), [0, 1, 2]).unwrap();
        fs::write(temp.0.join("large"), "12345").unwrap();
        let mut workspace = WorkspaceManager::new();
        workspace.open(&temp.0).unwrap();
        let mut documents = DocumentManager::with_max_document_bytes(4);
        assert!(matches!(
            documents.open_file(&workspace, "bad.txt"),
            Err(DocumentError::InvalidUtf8(_))
        ));
        assert!(matches!(
            documents.open_file(&workspace, "binary"),
            Err(DocumentError::BinaryFile(_))
        ));
        assert!(matches!(
            documents.open_file(&workspace, "large"),
            Err(DocumentError::TooLarge { .. })
        ));
    }
}
