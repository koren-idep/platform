import { redo, redoDepth, selectAll, undo, undoDepth } from '@codemirror/commands';
import { Compartment, EditorState } from '@codemirror/state';
import { basicSetup, EditorView } from 'codemirror';
import { linter, setDiagnostics, type Diagnostic as CodeMirrorDiagnostic } from '@codemirror/lint';
import { KorenClient, KorenError, createDesktopTransport } from './services/client';
import type { CargoProjectSummary, ExecutionApprovalSummary, ExtensionCatalog, ExtensionDiagnosticDetailSummary, ExtensionMenuSummary, ExtensionProjectWizardSummary, ExtensionSettingSummary, ExtensionSettingValue, ExtensionSummary, ExtensionThemeSummary, ExtensionToolWindowSummary, ExtensionUiDocument, GitRepositorySummary, GitStatusSummary, InspectionConfigurationSummary, InspectionFinding, LanguageDiagnosticSet, LanguageServerApprovalSummary, LanguageServerSummary, PlatformEvent, ProjectInspectionSummary, ResponseValue, TaskSummary, TextRange } from './services/protocol';
import type { SearchMatch } from './services/protocol';
import { approvalChoice, conflictChoice } from './app/dialogs';
import { commandShortcutMatches, filterCommands, nextCommandSelection } from './app/commands';
import { activePanelLayout, diagnosticDetailMatches, extensionMenusAt, safeThemeVariables, validExtensionSettingValue } from './app/extensions';
import { readLayout, persistLayout, type LayoutPreferences } from './app/layout';
import { formatCursorPosition, lineEndingFor } from './app/status';
import { runConfigurationTasks, selectedRunTask } from './app/toolbar';
import { groupSearchMatches, normalizedCommitMessage, partitionGitFiles, searchSummaryText } from './app/developerTools';
import { nextTheme, persistTheme, readTheme } from './app/theme';
import { taskStatus, visibleOutput } from './app/taskPresentation';
import { extensionCanActivate, extensionToggleLabel, languageServerEnabled } from './app/extensions';
import { diagnosticsForRevision, withDiagnosticPublication } from './app/diagnostics';
import { filterInspectionFindings, inspectionFindingGroupKey, revisionBoundEdits, type InspectionGrouping, type InspectionSeverityFilter } from './app/inspectionPresentation';
import { ProjectTreeState, TabState, type EditorTab } from './app/state';
import { DocumentEditPipeline } from './editor/sync';
import { editsFromViewUpdate } from './editor/transactions';
import { languageForPath, languageIdForPath, languageNameForPath } from './editor/languages';
import { flushAndSave } from './editor/save';
import { languageServiceExtensions } from './editor/languageServices';
import './styles/main.css';

const client = new KorenClient(createDesktopTransport());
const app = document.querySelector<HTMLDivElement>('#app')!;
app.innerHTML = `
  <header class="menu-bar">
    <div class="brand" aria-label="Koren Code Editor"><span class="brand-mark">&lt;/&gt;</span><span class="brand-name">Koren Code Editor</span></div>
    <span class="brand-divider" aria-hidden="true"></span>
    <nav class="menus" aria-label="Main menu">
      <details class="menu-group"><summary>File</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="open">Open Folder… <kbd>Ctrl+O</kbd></button>
        <button role="menuitem" data-action="open-file" id="menu-open-file" disabled>Open File… <kbd>Ctrl+Shift+O</kbd></button>
        <button role="menuitem" data-action="new-file">New File <kbd>Ctrl+N</kbd></button>
        <div class="menu-separator"></div>
        <button role="menuitem" data-action="save">Save <kbd>Ctrl+S</kbd></button>
        <button role="menuitem" data-action="save-all">Save All</button>
        <button role="menuitem" data-action="close-active">Close Active Tab <kbd>Ctrl+W</kbd></button>
      </div></details>
      <details class="menu-group"><summary>Edit</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="undo">Undo <kbd>Ctrl+Z</kbd></button>
        <button role="menuitem" data-action="redo">Redo <kbd>Ctrl+Y</kbd></button>
        <div class="menu-separator"></div><button role="menuitem" data-action="select-all">Select All <kbd>Ctrl+A</kbd></button>
      </div></details>
      <details class="menu-group"><summary>View</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="toggle-explorer">Toggle Project Explorer <kbd>Ctrl+B</kbd></button>
        <button role="menuitem" data-action="toggle-bottom">Toggle Bottom Panel <kbd>Ctrl+J</kbd></button>
        <div class="menu-separator"></div><button role="menuitem" data-action="toggle-theme">Toggle Theme <kbd>Alt+T</kbd></button>
        <div class="menu-separator"></div>
        <button role="menuitem" data-action="show-panel" data-panel-target="output">Output</button>
        <button role="menuitem" data-action="show-panel" data-panel-target="tasks">Tasks</button>
        <button role="menuitem" data-action="show-panel" data-panel-target="problems">Problems</button>
        <button role="menuitem" data-action="show-panel" data-panel-target="projects">Projects</button>
        <button role="menuitem" data-action="show-panel" data-panel-target="search">Search <kbd>Ctrl+Shift+F</kbd></button>
        <button role="menuitem" data-action="show-panel" data-panel-target="terminal">Terminal <kbd>Ctrl+grave</kbd></button>
        <button role="menuitem" data-action="source-control">Source Control</button>
        <div id="extension-panel-menu-items"></div>
        <div id="extension-wizard-menu-items"></div>
      </div></details>
      <details class="menu-group"><summary>Navigate</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="focus-explorer">Focus Project Explorer <kbd>Ctrl+B</kbd></button>
        <button role="menuitem" data-action="focus-editor">Focus Editor <kbd>F6</kbd></button>
        <div class="menu-separator"></div><button role="menuitem" data-action="previous-tab">Previous Tab <kbd>Ctrl+Shift+Tab</kbd></button>
        <button role="menuitem" data-action="next-tab">Next Tab <kbd>Ctrl+Tab</kbd></button>
        <div id="extension-navigate-items"></div>
      </div></details>
      <details class="menu-group"><summary>Run</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="discover">Discover Cargo Projects</button>
        <div class="menu-separator"></div>
        <button role="menuitem" data-action="run-cargo" data-command="build">Build Workspace</button>
        <button role="menuitem" data-action="run-cargo" data-command="check">Check Workspace</button>
        <button role="menuitem" data-action="run-cargo" data-command="test">Test Workspace</button>
        <button role="menuitem" data-action="run-cargo" data-command="clean">Clean Workspace</button>
        <div class="menu-separator"></div><button role="menuitem" data-action="cancel-task" disabled>Cancel Active Task <kbd>F5</kbd></button>
      </div></details>
      <details class="menu-group"><summary>Tools</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="command-palette">Command Palette… <kbd>Ctrl+Shift+P</kbd></button>
        <button role="menuitem" data-action="extensions">Extensions…</button>
        <button role="menuitem" data-action="import-sarif">Import SARIF Report…</button>
        <button role="menuitem" data-action="export-sarif">Export Current Inspections…</button>
        <div id="extension-tools-items"></div>
        <button role="menuitem" data-action="copy-path">Copy Current File Path</button>
        <button role="menuitem" data-action="refresh">Refresh Project Explorer</button>
      </div></details>
      <details class="menu-group"><summary>Help</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="shortcuts">Keyboard Shortcuts</button>
        <button role="menuitem" data-action="about">About Koren Code Editor</button>
      </div></details>
    </nav>
    <span class="menu-spacer"></span>
  </header>
  <div class="toolbar" role="toolbar" aria-label="Editor actions">
    <div class="toolbar-group">
      <button class="toolbar-button" data-action="new-file" title="New File (Ctrl+N)" aria-label="New File">＋</button>
      <button class="toolbar-button" data-action="open" title="Open Folder (Ctrl+O)" aria-label="Open Folder">▱</button>
      <button class="toolbar-button" data-action="save" id="toolbar-save" disabled title="Save (Ctrl+S)" aria-label="Save">▣</button>
    </div>
    <span class="toolbar-divider" aria-hidden="true"></span>
    <div class="toolbar-group">
      <button class="toolbar-button" data-action="undo" id="toolbar-undo" disabled title="Undo (Ctrl+Z)" aria-label="Undo">↶</button>
      <button class="toolbar-button" data-action="redo" id="toolbar-redo" disabled title="Redo (Ctrl+Y)" aria-label="Redo">↷</button>
    </div>
    <span class="toolbar-divider" aria-hidden="true"></span>
    <button class="toolbar-button" data-action="source-control" title="Source Control" aria-label="Source Control">⑂</button>
    <div class="run-controls">
      <button class="run-button" data-action="run-selected" id="toolbar-run" disabled title="Run selected configuration after approval"><span aria-hidden="true">▶</span><span>Run</span></button>
      <select id="run-configuration" aria-label="Run configuration" disabled><option value="">No run configuration</option></select>
    </div>
    <span class="toolbar-spacer"></span>
    <button class="toolbar-button toolbar-utility" data-action="command-palette" title="Command Palette (Ctrl+Shift+P)" aria-label="Command Palette">⌁</button>
    <button class="toolbar-button toolbar-utility" data-action="settings" title="Settings" aria-label="Settings">⚙</button>
  </div>
  <main class="workbench" id="workbench">
    <aside class="explorer" id="explorer">
      <div class="panel-heading"><span>PROJECT</span><span class="panel-actions"><button data-action="refresh" title="Refresh explorer" aria-label="Refresh explorer">↻</button><details class="panel-options"><summary title="Project panel options" aria-label="Project panel options">⋮</summary><div class="panel-options-menu" role="menu"><button role="menuitem" data-action="toggle-explorer">Collapse Project Panel</button></div></details><button data-action="toggle-explorer" title="Collapse explorer" aria-label="Collapse explorer">‹</button></span></div>
      <div id="tree" class="tree" role="tree" aria-label="Project files"></div>
      <div class="explorer-footer"><button data-action="discover">Discover Cargo</button></div>
    </aside>
    <div id="explorer-resizer" class="explorer-resizer" role="separator" aria-label="Resize project explorer" aria-orientation="vertical" tabindex="0"></div>
    <section class="main-column">
      <section class="editor-frame" aria-label="Editor workspace">
        <div id="tabs" class="tabs" role="tablist" aria-label="Open files"></div>
        <div id="editor" class="editor hidden" aria-label="Code editor"></div>
        <section id="welcome" class="welcome">
          <div class="welcome-mark">&lt;/&gt;</div><div class="welcome-title">Koren Code Editor</div>
          <p>A focused workspace for your code.</p>
          <div class="welcome-actions"><button class="primary-button" data-action="open">Open Folder</button><button data-action="open-file" id="welcome-open-file">Open File</button></div>
          <div class="welcome-hint">Open a folder <kbd>Ctrl+O</kbd> <span>·</span> Open a file <kbd>Ctrl+Shift+O</kbd> <span>·</span> Command Palette <kbd>Ctrl+Shift+P</kbd></div>
        </section>
      </section>
      <div class="bottom-resizer" id="bottom-resizer" role="separator" aria-label="Resize bottom panel" aria-orientation="horizontal" tabindex="0"></div>
      <section class="bottom-panel" id="bottom-panel">
        <div class="bottom-heading"><div class="tool-tabs" role="tablist" aria-label="Tool panels">
          <button class="selected" data-panel="output" role="tab" aria-selected="true">Output</button><button data-panel="problems" role="tab" aria-selected="false">Problems <span id="problem-count" class="count-badge">0</span></button><button data-panel="tasks" role="tab" aria-selected="false">Tasks</button><button data-panel="projects" role="tab" aria-selected="false">Projects</button><button data-panel="search" role="tab" aria-selected="false">Search</button><button data-panel="terminal" role="tab" aria-selected="false">Terminal</button>
        </div><span id="bottom-task-state" class="task-state"></span><span class="panel-actions"><button data-action="cancel-task" title="Cancel active task" aria-label="Cancel active task" disabled>■</button><button data-action="toggle-bottom" title="Collapse bottom panel" aria-label="Collapse bottom panel">⌄</button></span></div>
        <div id="output" class="panel-content output"><div class="panel-empty">Task output will appear here.</div></div>
        <div id="problems" class="panel-content problem-list hidden"><div class="problem-filters"><label>Severity <select id="problem-severity-filter"><option value="all">All</option><option value="error">Errors</option><option value="warning">Warnings</option><option value="information">Information</option><option value="hint">Hints</option></select></label><label>Group by <select id="problem-grouping"><option value="file">File</option><option value="provider">Provider</option><option value="category">Category</option><option value="severity">Severity</option></select></label></div><div id="problem-entries"><div class="panel-empty">No problems reported by installed language integrations.</div></div></div>
        <div id="tasks" class="panel-content task-list hidden"><div class="panel-empty">Discover Cargo projects to see available tasks.</div></div>
        <div id="projects" class="panel-content project-list hidden"><div class="project-analysis-controls"><button data-action="start-project-analysis">Analyze Open Documents</button><button data-action="cancel-project-analysis" disabled>Cancel</button><button data-action="load-project-findings" hidden>Load More Findings</button><span id="project-analysis-status">Uses a bounded snapshot of currently open documents.</span></div><div id="project-list"><div class="panel-empty">Project information will appear here.</div></div></div>
        <div id="search" class="panel-content developer-panel hidden"><form id="search-form" class="developer-controls"><input id="search-query" type="search" placeholder="Search workspace" aria-label="Search workspace"><input id="search-filter" type="text" placeholder="Files to include (glob)" aria-label="File name filter"><label><input id="search-case" type="checkbox"> Match case</label><label><input id="search-regex" type="checkbox"> Regex</label><button type="submit">Search</button><button type="button" data-action="cancel-search" disabled>Cancel</button></form><div id="search-status" class="subtle">Search starts only when requested. Results are limited to 5,000.</div><div id="search-results" class="developer-results"></div></div>
        <div id="terminal" class="panel-content terminal-panel hidden"><div class="terminal-toolbar"><button type="button" data-terminal-new>＋ New</button><button type="button" data-terminal-rename disabled>Rename</button><button type="button" data-terminal-restart disabled>Restart</button><button type="button" data-terminal-close disabled>Close</button><span data-terminal-status class="subtle">No terminal sessions.</span></div><div class="terminal-session-tabs" data-terminal-tabs role="tablist" aria-label="Terminal sessions"></div><div class="terminal-session-views" data-terminal-views></div><div class="terminal-shortcut-hint">Ctrl+Shift+C copy · Ctrl+Shift+V paste · Ctrl+L clear display</div></div>
        <div id="source-control" class="panel-content developer-panel hidden"><div class="developer-controls"><select id="git-repository" aria-label="Git repository"><option value="">No repository selected</option></select><button data-action="refresh-git">↻ Refresh</button></div><div id="git-branch" class="subtle"></div><div class="git-columns"><section><h3>Changes</h3><div id="git-unstaged" class="developer-results"></div></section><section><h3>Staged</h3><div id="git-staged" class="developer-results"></div></section></div><pre id="git-diff" class="git-diff" aria-label="Git diff preview"></pre><form id="git-commit-form" class="developer-controls"><input id="git-commit-message" type="text" placeholder="Commit message" aria-label="Commit message"><button type="submit">Commit staged</button></form></div>
        <div id="extension-panel-content"></div>
      </section>
    </section>
  </main>
  <footer class="status-bar">
    <div class="status-workspace"><span class="status-workspace-icon" aria-hidden="true">▱</span><span>Workspace:</span><strong id="status-workspace">No Workspace</strong></div>
    <span id="status-message" class="status-message">Ready</span><span id="status-document" class="status-revision"></span><span class="status-spacer"></span>
    <span id="status-language" class="status-item"></span><span id="status-encoding" class="status-item"></span><span id="status-line-ending" class="status-item"></span><span id="status-cursor" class="status-item"></span>
  </footer>
  <dialog id="approval-dialog" class="dialog"><form method="dialog"><header>Run task?</header><p class="subtle">Review the command before it starts. Approval is not a sandbox.</p><dl id="approval-details"></dl><div class="dialog-actions"><button value="deny" class="secondary">Deny</button><button value="approve" class="primary">Approve once</button></div></form></dialog>
  <dialog id="conflict-dialog" class="dialog conflict"><form method="dialog"><header>File changed outside Koren</header><p id="conflict-path"></p><details><summary>Inspect current disk contents</summary><pre id="conflict-preview"></pre></details><p class="subtle">Choose whether to reload the external version or overwrite it with your buffer. If the file changes again, Koren will ask again.</p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="reload" class="secondary">Reload disk version</button><button value="overwrite" class="primary">Overwrite file</button></div></form></dialog>
  <dialog id="confirm-dialog" class="dialog"><form method="dialog"><header id="confirm-title">Unsaved changes</header><p id="confirm-message"></p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="discard" class="primary">Discard changes</button></div></form></dialog>
  <dialog id="close-dialog" class="dialog"><form method="dialog"><header>Unsaved documents</header><p>Save your changes before closing Koren Code Editor?</p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="discard" class="secondary">Discard and close</button><button value="save" class="primary">Save all and close</button></div></form></dialog>
  <dialog id="switch-workspace-dialog" class="dialog"><form method="dialog"><header>Switch workspace?</header><p>Choose what to do with open documents before changing folders.</p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="discard" class="secondary">Discard and switch</button><button value="save" class="primary">Save and switch</button></div></form></dialog>
  <dialog id="command-dialog" class="dialog command-dialog"><form method="dialog"><header>Command Palette</header><input id="command-search" type="search" placeholder="Search available commands" autocomplete="off" aria-label="Search commands"><div id="command-results" role="listbox" aria-label="Available commands"></div><footer><kbd>↑</kbd><kbd>↓</kbd> navigate <kbd>Enter</kbd> run <kbd>Esc</kbd> close</footer></form></dialog>
  <dialog id="extensions-dialog" class="dialog extensions-dialog"><form method="dialog"><header>Extensions</header><p class="subtle">Local extensions from the configured Koren extensions folder. Code runs only after you enable and activate it.</p><div id="extension-list" class="extension-list"></div><div class="dialog-actions"><button value="close" class="primary">Close</button></div></form></dialog>
  <dialog id="language-server-approval-dialog" class="dialog"><form method="dialog"><header>Start language server?</header><p class="subtle">This starts an external program with your user account permissions. Approval is not a sandbox. Review the exact command and workspace.</p><dl id="language-server-approval-details"></dl><div class="dialog-actions"><button value="deny" class="secondary">Deny</button><button value="approve" class="primary">Approve and start</button></div></form></dialog>
  <dialog id="terminal-approval-dialog" class="dialog"><form><header>Start interactive shell?</header><p class="subtle">Approval grants this shell interactive access using your account permissions. Commands entered later are not individually reviewed, and this is not a filesystem sandbox.</p><dl data-terminal-approval-details></dl><div class="dialog-actions"><button type="submit" value="deny" class="secondary">Deny</button><button type="submit" value="approve" class="primary">Approve and start</button></div></form></dialog>
  <dialog id="settings-dialog" class="dialog settings-dialog"><form method="dialog"><header>Settings</header><label for="theme-setting">Color theme</label><select id="theme-setting"><option value="dark">OLED Black</option><option value="light">Light</option></select><div id="extension-settings" class="extension-settings"></div><section class="inspection-settings"><h3>Inspections</h3><div id="inspection-settings"></div></section><div class="dialog-actions"><button value="close" class="primary">Close</button></div></form></dialog>
  <dialog id="info-dialog" class="dialog"><form method="dialog"><header id="info-title"></header><div id="info-content" class="info-content"></div><div class="dialog-actions"><button value="close" class="primary">Close</button></div></form></dialog>
  <dialog id="workspace-input-dialog" class="dialog"><form id="workspace-input-form"><header id="workspace-input-title"></header><label id="workspace-input-label" for="workspace-input"></label><input id="workspace-input" type="text" autocomplete="off"><div class="dialog-actions"><button type="button" value="cancel" class="secondary">Cancel</button><button type="submit" class="primary">Continue</button></div></form></dialog>
  <dialog id="workspace-delete-dialog" class="dialog"><form method="dialog"><header>Delete workspace entry?</header><p id="workspace-delete-path"></p><p class="subtle">This permanently deletes the selected entry from disk. Open dirty documents are protected.</p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="delete" class="danger">Delete</button></div></form></dialog>
  <dialog id="project-wizard-dialog" class="dialog"><form id="project-wizard-form"><header id="project-wizard-title"></header><div id="project-wizard-fields" class="extension-panel-view"></div><p class="subtle">Submitting validates these fields only. Project creation must be performed by a host-authorized extension operation.</p><div class="dialog-actions"><button type="button" class="secondary" data-action="close-wizard">Cancel</button><button type="submit" class="primary">Validate form</button></div></form></dialog>
  <div id="status-toast" role="status" aria-live="polite"></div>`;

let terminalPanel: import('./app/terminal').TerminalPanel | null = null;
async function loadTerminalPanel(): Promise<void> {
  if (terminalPanel) return;
  const { TerminalPanel } = await import('./app/terminal');
  terminalPanel ??= new TerminalPanel(document.querySelector<HTMLElement>('#terminal')!, client);
}

const treeState = new ProjectTreeState();
const tabs = new TabState();
const layout: LayoutPreferences = readLayout(localStorage);
const treeRoot = document.querySelector<HTMLDivElement>('#tree')!;
const editorHost = document.querySelector<HTMLDivElement>('#editor')!;
const welcome = document.querySelector<HTMLElement>('#welcome')!;
const tabsHost = document.querySelector<HTMLDivElement>('#tabs')!;
const outputHost = document.querySelector<HTMLDivElement>('#output')!;
const taskHost = document.querySelector<HTMLDivElement>('#tasks')!;
const projectHost = document.querySelector<HTMLDivElement>('#project-list')!;
const runConfiguration = document.querySelector<HTMLSelectElement>('#run-configuration')!;
const toast = document.querySelector<HTMLDivElement>('#status-toast')!;
const workbench = document.querySelector<HTMLElement>('#workbench')!;
const rootLabel = document.querySelector<HTMLElement>('#status-workspace')!;
let workspaceRoot = '';
let selectedPath = '';
let runId: number | null = null;
const documentViews = new Map<number, { state: EditorState; pipeline: DocumentEditPipeline; lineEnding: 'LF' | 'CRLF'; languageConfig: Compartment }>();
const taskById = new Map<number, TaskSummary>();
const activeLanguageServers = new Map<string, number>();
const diagnosticsByDocument = new Map<number, Map<string, LanguageDiagnosticSet>>();
const inspectionFindingsByDocument = new Map<number, InspectionFinding[]>();
let importedSarifFindings: InspectionFinding[] = [];
const inspectionTimers = new Map<number, number>();
const inspectionGenerations = new Map<number, number>();
let editorContextMenus: ExtensionMenuSummary[] = [];
let projectExplorerMenus: ExtensionMenuSummary[] = [];
let extensionCatalogCache: ExtensionCatalog | null = null;
let openExtensionPanels = loadOpenExtensionPanels();
let activeDynamicPanel = localStorage.getItem('koren.activeExtensionPanel') ?? '';
let registeredCommands: string[] = [];
const commandLabels = new Map<string, string>();
const commandDescriptions = new Map<string, string>();
const commandMetadata = new Map<string, { description: string; shortcut: string | null; enabled: boolean }>();
let editor: EditorView | null = null;
let outputRefreshTimer = 0;
let toastTimer = 0;
let problemSeverityFilter: InspectionSeverityFilter = 'all';
let problemGrouping: InspectionGrouping = 'file';
let activeProjectInspection: number | null = null;
let latestProjectInspectionId: number | null = null;
let projectInspectionTimer = 0;
let projectInspectionOffset: number | null = null;
let projectSnapshotFindings: InspectionFinding[] = [];
let gitRepositories: GitRepositorySummary[] = [];
let gitStatus: GitStatusSummary | null = null;
let gitRepositoryPath = '';
let activeSearchJob: number | null = null;
let searchPollTimer = 0;
let searchMatches: SearchMatch[] = [];
let searchPollingJob: number | null = null;

function expect<T extends ResponseValue['kind']>(response: ResponseValue, kind: T): Extract<ResponseValue, { kind: T }> {
  if (response.kind !== kind) throw new Error(`Expected ${kind}, received ${response.kind}`);
  return response as Extract<ResponseValue, { kind: T }>;
}

function setStatus(message: string): void {
  document.querySelector('#status-message')!.textContent = message;
  toast.textContent = message;
  toast.classList.add('visible');
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => toast.classList.remove('visible'), 2200);
}
function showError(error: unknown): void { setStatus(error instanceof Error ? error.message : String(error)); }

function applyLayout(): void {
  workbench.style.setProperty('--explorer-width', `${layout.explorerWidth}px`);
  workbench.style.setProperty('--bottom-height', `${layout.bottomHeight}px`);
  document.querySelector('#explorer')!.classList.toggle('collapsed', layout.explorerCollapsed);
  workbench.classList.toggle('explorer-collapsed', layout.explorerCollapsed);
  document.querySelector('#bottom-panel')!.classList.toggle('collapsed', layout.bottomCollapsed);
}
function saveLayout(): void { persistLayout(localStorage, layout); }
function setExplorerCollapsed(collapsed: boolean): void {
  layout.explorerCollapsed = collapsed;
  document.querySelector('#explorer')!.classList.toggle('collapsed', collapsed);
  workbench.classList.toggle('explorer-collapsed', collapsed);
  saveLayout();
}
function setBottomCollapsed(collapsed: boolean): void {
  layout.bottomCollapsed = collapsed;
  document.querySelector('#bottom-panel')!.classList.toggle('collapsed', collapsed);
  document.querySelector('#bottom-resizer')!.classList.toggle('hidden', collapsed);
  saveLayout();
}

async function openWorkspace(path: string): Promise<void> {
  const value = expect(await client.call({ method: 'open_workspace', params: { path } }), 'workspace_opened').value;
  workspaceRoot = value.root;
  rootLabel.textContent = baseName(value.root);
  rootLabel.title = value.root;
  document.querySelector('#status-workspace')!.textContent = baseName(value.root);
  document.querySelector('#welcome-open-file')!.removeAttribute('disabled');
  document.querySelector<HTMLButtonElement>('#menu-open-file')!.disabled = false;
  treeState.setEntries('', expect(await client.call({ method: 'list_directory', params: { path: '' } }), 'directory_entries').value);
  renderTree();
  await refreshProjects();
  setStatus('Workspace opened');
}

function baseName(path: string): string { return path.replace(/[\\/]+$/, '').split(/[\\/]/).at(-1) || path; }

async function refreshDirectory(path: string): Promise<void> {
  treeState.setLoading(path, true); renderTree();
  try {
    const entries = expect(await client.call({ method: 'list_directory', params: { path } }), 'directory_entries').value;
    treeState.setEntries(path, entries);
  } catch (error) { treeState.setError(path, error instanceof Error ? error.message : String(error)); }
  renderTree();
}

async function refreshWorkspacePath(path: string): Promise<void> {
  if (!path) { await refreshDirectory(''); return; }
  const parts = path.split('/').filter(Boolean); let current = '';
  await refreshDirectory('');
  for (const part of parts) {
    current = current ? `${current}/${part}` : part;
    const node = treeState.child(current); node.expanded = true;
    await refreshDirectory(current);
  }
}

function askWorkspacePath(title: string, label: string, initial = ''): Promise<string | null> {
  const dialog = document.querySelector<HTMLDialogElement>('#workspace-input-dialog')!;
  const form = document.querySelector<HTMLFormElement>('#workspace-input-form')!;
  const input = document.querySelector<HTMLInputElement>('#workspace-input')!;
  document.querySelector('#workspace-input-title')!.textContent = title;
  document.querySelector('#workspace-input-label')!.textContent = label;
  input.value = initial;
  return new Promise((resolve) => {
    const cancelButton = form.querySelector<HTMLButtonElement>('[value="cancel"]')!;
    const cancel = () => finish(null);
    const submit = (event: SubmitEvent) => { event.preventDefault(); finish(input.value.trim() || null); };
    const finish = (value: string | null) => {
      form.removeEventListener('submit', submit); cancelButton.removeEventListener('click', cancel); dialog.removeEventListener('cancel', cancel);
      dialog.close(); resolve(value);
    };
    form.addEventListener('submit', submit); cancelButton.addEventListener('click', cancel); dialog.addEventListener('cancel', cancel);
    dialog.showModal(); input.focus(); input.select();
  });
}

function parentPath(path: string): string { return path.split('/').slice(0, -1).join('/'); }
async function createWorkspaceEntry(directory: string, isDirectory: boolean): Promise<void> {
  if (!workspaceRoot) { setStatus('Open a workspace first'); return; }
  const path = await askWorkspacePath(isDirectory ? 'New folder' : 'New file', 'Workspace-relative path', directory ? `${directory}/` : '');
  if (!path) return;
  try {
    const response = await client.call({ method: isDirectory ? 'create_workspace_directory' : 'create_workspace_file', params: { path } });
    if (response.kind !== 'workspace_entry') throw new Error('The workspace did not create the entry.');
    await refreshWorkspacePath(parentPath(path)); setStatus(`${isDirectory ? 'Folder' : 'File'} created`);
    if (!isDirectory) await openDocument(response.value.path);
  } catch (error) { showError(error); }
}

async function renameWorkspaceEntry(path: string): Promise<void> {
  const destination = await askWorkspacePath('Rename or move', 'New workspace-relative path', path);
  if (!destination || destination === path) return;
  try {
    const result = expect(await client.call({ method: 'rename_workspace_entry', params: { source: path, destination } }), 'workspace_entry_changed').value;
    for (const tab of tabs.tabs) if (tab.path === path || tab.path.startsWith(`${path}/`)) {
      tab.path = `${destination}${tab.path.slice(path.length)}`; tab.title = baseName(tab.path); tab.language = languageNameForPath(tab.path);
      void refreshDocumentLanguage(tab.id);
    }
    if (selectedPath === path || selectedPath.startsWith(`${path}/`)) selectedPath = `${destination}${selectedPath.slice(path.length)}`;
    await refreshWorkspacePath(parentPath(path)); if (parentPath(destination) !== parentPath(path)) await refreshWorkspacePath(parentPath(destination));
    renderTabs(); renderTree(); updateStatusDocument(); setStatus(`Renamed ${result.previous_path ?? path}`);
  } catch (error) { showError(error); }
}

async function deleteWorkspaceEntry(path: string, isDirectory: boolean): Promise<void> {
  const dialog = document.querySelector<HTMLDialogElement>('#workspace-delete-dialog')!;
  document.querySelector('#workspace-delete-path')!.textContent = path;
  const choice = await new Promise<boolean>((resolve) => {
    const form = dialog.querySelector('form')!;
    const cancel = () => finish(false);
    const submit = (event: SubmitEvent) => { event.preventDefault(); const value = (event.submitter as HTMLButtonElement | null)?.value === 'delete'; finish(value); };
    const finish = (value: boolean) => { form.removeEventListener('submit', submit); dialog.removeEventListener('cancel', cancel); dialog.close(); resolve(value); };
    form.addEventListener('submit', submit); dialog.addEventListener('cancel', cancel); dialog.showModal();
  });
  if (!choice) return;
  try {
    expect(await client.call({ method: 'delete_workspace_entry', params: { path, recursive: isDirectory, confirmed: true } }), 'workspace_entry_changed');
    await refreshWorkspacePath(parentPath(path)); renderTree(); setStatus('Workspace entry deleted');
  } catch (error) { showError(error); }
}

function showWorkspaceContextMenu(event: MouseEvent, path: string, isDirectory: boolean): void {
  event.preventDefault(); document.querySelector('#editor-context-menu')?.remove();
  const menu = document.createElement('div'); menu.id = 'editor-context-menu'; menu.className = 'editor-context-menu'; menu.setAttribute('role', 'menu');
  const options: Array<[string, () => void]> = [
    ['New File', () => void createWorkspaceEntry(isDirectory ? path : parentPath(path), false)],
    ['New Folder', () => void createWorkspaceEntry(isDirectory ? path : parentPath(path), true)],
    ...(path ? [['Rename / Move', () => void renameWorkspaceEntry(path)], ['Delete…', () => void deleteWorkspaceEntry(path, isDirectory)]] as Array<[string, () => void]> : []),
    ['Refresh', () => void refreshDirectory(path)],
  ];
  for (const [label, action] of options) { const button = document.createElement('button'); button.setAttribute('role', 'menuitem'); button.textContent = label; button.addEventListener('click', () => { menu.remove(); action(); }); menu.append(button); }
  menu.style.left = `${event.clientX}px`; menu.style.top = `${event.clientY}px`; document.body.append(menu);
}

function appendIcon(parent: HTMLElement, kind: string, expanded = false): void {
  const disclosure = document.createElement('span');
  disclosure.className = 'tree-chevron';
  disclosure.textContent = kind === 'directory' ? (expanded ? '▾' : '▸') : '';
  disclosure.setAttribute('aria-hidden', 'true');
  const icon = document.createElement('span');
  icon.className = `tree-icon icon-${kind}`;
  icon.setAttribute('aria-hidden', 'true');
  icon.textContent = kind === 'directory' ? '▱' : kind === 'symlink' ? '↗' : kind === 'file' ? '◇' : '·';
  parent.append(disclosure, icon);
}

function renderTree(): void {
  treeRoot.replaceChildren();
  if (!workspaceRoot) {
    const blank = document.createElement('div'); blank.className = 'tree-empty';
    blank.innerHTML = '<span class="tree-empty-icon" aria-hidden="true">▱</span><span>Open a folder to browse files.</span>';
    treeRoot.append(blank); return;
  }
  const heading = document.createElement('div'); heading.className = 'root-row'; heading.title = workspaceRoot;
  appendIcon(heading, 'directory', true);
  const headingText = document.createElement('span'); headingText.textContent = baseName(workspaceRoot); heading.append(headingText); treeRoot.append(heading);
  const rootNode = treeState.nodes.get('');
  if (rootNode?.loading) appendTreeMessage('Loading project files…');
  if (rootNode?.error) appendTreeMessage(rootNode.error, 'tree-error');
  if (!rootNode?.entries.length && !rootNode?.loading && !rootNode?.error) appendTreeMessage('This folder is empty.');
  for (const { entry, depth } of treeState.visible()) {
    const node = treeState.nodes.get(entry.path);
    const row = document.createElement('button');
    row.className = `tree-row ${entry.kind}${selectedPath === entry.path ? ' selected' : ''}`;
    row.style.paddingLeft = `${8 + depth * 15}px`;
    row.title = entry.path;
    row.setAttribute('role', 'treeitem'); row.setAttribute('aria-level', String(depth + 1));
    if (entry.kind === 'directory') row.setAttribute('aria-expanded', String(node?.expanded ?? false));
    if (selectedPath === entry.path) row.setAttribute('aria-current', 'true');
    appendIcon(row, entry.kind, node?.expanded);
    const label = document.createElement('span'); label.textContent = entry.name; row.append(label);
    row.addEventListener('click', () => {
      if (entry.kind === 'directory') {
        const expanded = treeState.toggle(entry.path);
        if (expanded && !node?.entries.length) void refreshDirectory(entry.path); else renderTree();
      } else if (entry.kind === 'file') {
        selectedPath = entry.path; void openDocument(entry.path);
      }
    });
    row.addEventListener('keydown', (event) => handleTreeKey(event, entry.path, entry.kind));
    treeRoot.append(row);
    if (node?.loading) appendTreeMessage('Loading…', 'tree-loading', depth + 1);
    if (node?.error) appendTreeMessage(node.error, 'tree-error', depth + 1);
  }
}

function appendTreeMessage(message: string, className = 'tree-message', depth = 0): void {
  const element = document.createElement('div'); element.className = className; element.textContent = message;
  element.style.paddingLeft = `${12 + depth * 15}px`; treeRoot.append(element);
}

function handleTreeKey(event: KeyboardEvent, path: string, kind: string): void {
  const rows = [...treeRoot.querySelectorAll<HTMLButtonElement>('.tree-row')];
  const index = rows.indexOf(event.currentTarget as HTMLButtonElement);
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault(); rows[Math.max(0, Math.min(rows.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)))]?.focus();
  } else if (event.key === 'ArrowRight' && kind === 'directory') {
    const node = treeState.nodes.get(path);
    if (node && !node.expanded) { treeState.toggle(path); void refreshDirectory(path); }
  } else if (event.key === 'ArrowLeft' && kind === 'directory') {
    const node = treeState.nodes.get(path);
    if (node?.expanded) { treeState.toggle(path); renderTree(); treeRoot.querySelector<HTMLButtonElement>(`[title="${CSS.escape(path)}"]`)?.focus(); }
  }
}

async function openDocument(path: string): Promise<void> {
  try {
    const opened = expect(await client.call({ method: 'open_document', params: { path } }), 'document_opened').value;
    const alreadyOpen = tabs.tabs.find((tab) => tab.id === opened.document_id);
    if (alreadyOpen) { selectTab(alreadyOpen.id); return; }
    const content = expect(await client.call({ method: 'read_document', params: { document_id: opened.document_id } }), 'document_contents').value;
    const tab: EditorTab = {
      id: opened.document_id, path, title: baseName(path), revision: content.revision,
      dirty: content.dirty, language: languageNameForPath(path), lineEnding: lineEndingFor(content.contents),
    };
    tabs.open(tab); selectedPath = path;
    const { state, pipeline, lineEnding, languageConfig } = await createDocumentState(tab, content.contents, content.revision);
    documentViews.set(tab.id, { state, pipeline, lineEnding, languageConfig });
    if (!editor) editor = new EditorView({ state, parent: editorHost }); else editor.setState(state);
    welcome.classList.add('hidden'); editorHost.classList.remove('hidden');
    renderTree(); renderTabs(); updateStatusDocument(); editor.focus();
    scheduleInspection(tab.id);
  } catch (error) { showError(error); }
}

async function createDocumentState(tab: EditorTab, contents: string, revision: number): Promise<{ state: EditorState; pipeline: DocumentEditPipeline; lineEnding: 'LF' | 'CRLF'; languageConfig: Compartment }> {
  const lineEnding = lineEndingFor(contents);
  const lineSeparator = lineEnding === 'CRLF' ? '\r\n' : '\n';
  const languageExtensions = await languageForPath(tab.path);
  const languageConfig = new Compartment();
  const serviceExtensions = languageServiceExtensions(client, {
    documentId: tab.id,
    revision: () => documentViews.get(tab.id)?.pipeline.currentRevision ?? revision,
    flush: () => documentViews.get(tab.id)?.pipeline.flush() ?? Promise.resolve(),
    navigate: navigateToDefinition,
  });
  const theme = EditorView.theme({
    '&': { height: '100%', color: 'var(--text)', backgroundColor: 'var(--bg)' },
    '.cm-content': { caretColor: 'var(--accent)' },
    '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--accent)' },
    '.cm-gutters': { backgroundColor: 'var(--panel)', color: 'var(--muted)', borderRight: '1px solid var(--border)' },
    '.cm-activeLineGutter, .cm-activeLine': { backgroundColor: 'var(--active-line)' },
    '.cm-selectionBackground, ::selection': { backgroundColor: 'var(--selection) !important' },
    '&.cm-focused .cm-selectionBackground': { backgroundColor: 'var(--selection-strong) !important' },
  });
  const state = EditorState.create({
    doc: contents,
    extensions: [basicSetup, linter(() => []), theme, EditorState.tabSize.of(4), EditorState.lineSeparator.of(lineSeparator), EditorView.lineWrapping, languageConfig.of(languageExtensions), ...serviceExtensions,
      EditorView.updateListener.of((update) => {
        const entry = documentViews.get(tab.id);
        if (!entry) return;
        entry.state = update.state;
        if (update.docChanged) {
          entry.pipeline.enqueue(editsFromViewUpdate(update));
          diagnosticsByDocument.delete(tab.id);
          inspectionFindingsByDocument.delete(tab.id);
          scheduleInspection(tab.id);
          queueMicrotask(() => { if (tabs.activeId === tab.id) renderLanguageDiagnostics(tab.id); renderProblems(); });
        }
        if (update.docChanged || update.selectionSet || update.focusChanged) updateStatusDocument();
      })],
  });
  const pipeline = new DocumentEditPipeline(revision, {
    edit: async (expectedRevision, edits) => expect(await client.call({ method: 'apply_document_edits', params: { document_id: tab.id, expected_revision: expectedRevision, edits } }), 'document_changed').value,
    read: async () => {
      const snapshot = expect(await client.call({ method: 'read_document', params: { document_id: tab.id } }), 'document_contents').value;
      return { contents: snapshot.contents, revision: snapshot.revision, dirty: snapshot.dirty };
    },
  }, () => documentViews.get(tab.id)?.state.doc.toString() ?? contents,
  (newRevision, dirty) => { tabs.update(tab.id, { revision: newRevision, dirty }); renderTabs(); updateStatusDocument(); }, showError);
  return { state, pipeline, lineEnding, languageConfig };
}

async function refreshDocumentLanguage(documentId: number): Promise<void> {
  const tab = tabs.tabs.find((item) => item.id === documentId); const view = documentViews.get(documentId);
  if (!tab || !view) return;
  const language = await languageForPath(tab.path);
  view.state = view.state.update({ effects: view.languageConfig.reconfigure(language) }).state;
  if (tabs.activeId === documentId && editor) editor.setState(view.state);
}

function renderTabs(): void {
  tabsHost.replaceChildren();
  for (const tab of tabs.tabs) {
    const item = document.createElement('div'); item.className = `tab${tab.id === tabs.activeId ? ' active' : ''}`;
    item.setAttribute('role', 'tab'); item.setAttribute('aria-selected', String(tab.id === tabs.activeId)); item.tabIndex = tab.id === tabs.activeId ? 0 : -1;
    const select = document.createElement('button'); select.className = 'tab-select'; select.textContent = tab.title;
    select.title = tab.path; select.setAttribute('aria-label', `${tab.title}${tab.dirty ? ', unsaved' : ''}`);
    if (tab.dirty) { const marker = document.createElement('span'); marker.className = 'dirty-marker'; marker.textContent = '●'; marker.setAttribute('aria-label', 'Unsaved changes'); select.prepend(marker); }
    select.addEventListener('click', () => selectTab(tab.id));
    select.addEventListener('keydown', (event) => {
      if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
      const current = tabs.tabs.findIndex((candidate) => candidate.id === tab.id);
      const next = tabs.tabs[(current + (event.key === 'ArrowRight' ? 1 : -1) + tabs.tabs.length) % tabs.tabs.length];
      if (next) { event.preventDefault(); selectTab(next.id); tabsHost.querySelector<HTMLButtonElement>(`[aria-label^="${CSS.escape(next.title)}"]`)?.focus(); }
    });
    const close = document.createElement('button'); close.className = 'tab-close'; close.textContent = '×';
    close.title = `Close ${tab.title}`; close.setAttribute('aria-label', `Close ${tab.title}`);
    close.addEventListener('click', () => void closeTab(tab));
    item.append(select, close); tabsHost.append(item);
  }
  if (!tabs.tabs.length) { editorHost.classList.add('hidden'); welcome.classList.remove('hidden'); }
}

function selectTab(id: number): void {
  if (!tabs.select(id)) return;
  const tab = tabs.active;
  if (!tab) return;
  selectedPath = tab.path;
  const entry = documentViews.get(id); if (entry && editor) editor.setState(entry.state);
  renderLanguageDiagnostics(id);
  scheduleInspection(id);
  renderTree(); renderTabs(); updateStatusDocument(); editor?.focus();
  void client.call({ method: 'read_document', params: { document_id: id } }).catch(showError);
}

function updateStatusDocument(): void {
  const tab = tabs.active;
  const revision = document.querySelector<HTMLElement>('#status-document')!;
  const details: Array<[string, string]> = [
    ['status-language', tab?.language ?? ''], ['status-cursor', ''],
    ['status-encoding', tab ? 'UTF-8' : ''], ['status-line-ending', tab?.lineEnding ?? ''],
  ];
  if (tab && editor) {
    const line = editor.state.doc.lineAt(editor.state.selection.main.head);
    details[1]![1] = formatCursorPosition(line.number, editor.state.selection.main.head - line.from + 1);
  }
  for (const [id, value] of details) document.querySelector<HTMLElement>(`#${id}`)!.textContent = value;
  revision.textContent = tab ? `Rev ${tab.revision}${tab.dirty ? ' · Modified' : ''}` : '';
  const hasDocument = !!tab && !!editor;
  document.querySelector<HTMLButtonElement>('#toolbar-save')!.disabled = !hasDocument;
  document.querySelector<HTMLButtonElement>('#toolbar-undo')!.disabled = !hasDocument || undoDepth(editor!.state) === 0;
  document.querySelector<HTMLButtonElement>('#toolbar-redo')!.disabled = !hasDocument || redoDepth(editor!.state) === 0;
}

function showInfo(title: string, content: string): void {
  document.querySelector('#info-title')!.textContent = title;
  document.querySelector('#info-content')!.textContent = content;
  document.querySelector<HTMLDialogElement>('#info-dialog')!.showModal();
}

function askDialog(dialog: HTMLDialogElement, title?: string, message?: string): Promise<string> {
  if (title) document.querySelector('#confirm-title')!.textContent = title;
  if (message) document.querySelector('#confirm-message')!.textContent = message;
  dialog.showModal();
  return new Promise((resolve) => dialog.addEventListener('close', () => resolve(dialog.returnValue), { once: true }));
}

async function closeTab(tab: EditorTab): Promise<void> {
  try {
    const action = tab.dirty ? await askDialog(document.querySelector<HTMLDialogElement>('#confirm-dialog')!, 'Unsaved changes', `Discard changes to ${tab.title}?`) : 'discard';
    if (action === 'cancel') return;
    await documentViews.get(tab.id)?.pipeline.flush();
    await client.call({ method: 'close_document', params: { document_id: tab.id, discard_unsaved_changes: action === 'discard' } });
    const wasActive = tabs.activeId === tab.id;
    tabs.close(tab.id); documentViews.delete(tab.id); diagnosticsByDocument.delete(tab.id); inspectionFindingsByDocument.delete(tab.id);
    const inspectionTimer = inspectionTimers.get(tab.id); if (inspectionTimer !== undefined) window.clearTimeout(inspectionTimer);
    inspectionTimers.delete(tab.id); inspectionGenerations.delete(tab.id); renderProblems();
    if (wasActive && tabs.activeId !== null) selectTab(tabs.activeId); else renderTabs();
    updateStatusDocument();
  } catch (error) { showError(error); }
}

async function saveActive(): Promise<void> {
  const tab = tabs.active; if (!tab) { setStatus('No open file to save'); return; }
  try {
    const entry = documentViews.get(tab.id)!;
    const result = await flushAndSave(entry.pipeline, async (expectedRevision) => expect(await client.call({ method: 'save_document', params: { document_id: tab.id, expected_revision: expectedRevision } }), 'document_saved').value);
    tabs.update(tab.id, { revision: result.revision, dirty: result.dirty }); renderTabs(); updateStatusDocument(); setStatus('Saved');
  } catch (error) {
    if (error instanceof KorenError && error.code === 'conflict') { await showConflict(tab); return; }
    showError(error);
  }
}

async function saveAll(): Promise<boolean> {
  for (const tab of tabs.tabs) {
    if (!tab.dirty) continue;
    try {
      const entry = documentViews.get(tab.id); if (!entry) return false;
      const result = await flushAndSave(entry.pipeline, async (revision) => expect(await client.call({ method: 'save_document', params: { document_id: tab.id, expected_revision: revision } }), 'document_saved').value);
      tabs.update(tab.id, { revision: result.revision, dirty: result.dirty });
    } catch (error) {
      if (error instanceof KorenError && error.code === 'conflict') await showConflict(tab); else showError(error);
      return false;
    }
  }
  renderTabs(); updateStatusDocument(); setStatus('All files saved');
  return tabs.tabs.every((tab) => !tab.dirty);
}

async function showConflict(tab: EditorTab): Promise<void> {
  document.querySelector('#conflict-path')!.textContent = tab.path;
  const pre = document.querySelector('#conflict-preview')!; pre.textContent = 'Loading current disk contents…';
  try {
    const preview = expect(await client.call({ method: 'read_document_conflict', params: { document_id: tab.id } }), 'document_conflict_contents').value;
    pre.textContent = preview.contents ?? '(The file was deleted.)';
  } catch (error) { pre.textContent = error instanceof Error ? error.message : String(error); }
  const dialog = document.querySelector<HTMLDialogElement>('#conflict-dialog')!; dialog.showModal();
  const action = conflictChoice(await new Promise<string>((resolve) => dialog.addEventListener('close', () => resolve(dialog.returnValue), { once: true })));
  if (!action) return;
  try {
    const decision = expect(await client.call({ method: 'resolve_document_conflict', params: { document_id: tab.id, expected_revision: tab.revision, decision: action } }), 'document_saved').value;
    tabs.update(tab.id, { revision: decision.revision, dirty: decision.dirty });
    if (action === 'reload') {
      const content = expect(await client.call({ method: 'read_document', params: { document_id: tab.id } }), 'document_contents').value;
      const { state, pipeline, lineEnding, languageConfig } = await createDocumentState(tab, content.contents, content.revision);
      tabs.update(tab.id, { lineEnding });
      documentViews.set(tab.id, { state, pipeline, lineEnding, languageConfig });
      if (tabs.activeId === tab.id && editor) editor.setState(state);
    }
    renderTabs(); updateStatusDocument();
  } catch (error) { showError(error); }
}

async function chooseWorkspace(): Promise<void> {
  try {
    const response = await fetch('/picker', { cache: 'no-store' });
    const { path } = await response.json() as { path: string | null };
    if (!path) return;
    if (!workspaceRoot) { await openWorkspace(path); return; }
    if (path === workspaceRoot) { setStatus('This workspace is already open'); return; }
    const dirty = tabs.tabs.some((tab) => tab.dirty);
    let discard = false;
    if (dirty) {
      const action = await askDialog(document.querySelector<HTMLDialogElement>('#switch-workspace-dialog')!);
      if (action === 'cancel' || !action) return;
      if (action === 'save' && !await saveAll()) return;
      discard = action === 'discard';
    }
    for (const tab of [...tabs.tabs]) {
      await client.call({ method: 'close_document', params: { document_id: tab.id, discard_unsaved_changes: discard } });
    }
    await client.call({ method: 'close_workspace' });
    for (const tab of [...tabs.tabs]) { tabs.close(tab.id); documentViews.delete(tab.id); }
    workspaceRoot = ''; selectedPath = ''; taskById.clear();
    rootLabel.textContent = 'No Workspace'; rootLabel.removeAttribute('title');
    document.querySelector('#status-workspace')!.textContent = '';
    document.querySelector<HTMLButtonElement>('#menu-open-file')!.disabled = true;
    document.querySelector<HTMLButtonElement>('#welcome-open-file')!.disabled = true;
    taskHost.innerHTML = '<div class="panel-empty">Discover Cargo projects to see available tasks.</div>';
    projectHost.innerHTML = '<div class="panel-empty">Project information will appear here.</div>';
    outputHost.innerHTML = '<div class="panel-empty">Task output will appear here.</div>';
    runId = null; updateRunConfigurations([]); setCancelEnabled(false);
    treeState.reset(); renderTree(); renderTabs(); updateStatusDocument();
    await openWorkspace(path);
  } catch (error) { showError(error); }
}

async function chooseFile(): Promise<void> {
  if (!workspaceRoot) { setStatus('Open a folder before choosing a file'); return; }
  try {
    const response = await fetch('/file-picker', { cache: 'no-store' });
    const { path } = await response.json() as { path: string | null };
    if (!path) return;
    const rootPath = workspaceRoot.replaceAll('\\', '/').replace(/\/$/, '');
    const filePath = path.replaceAll('\\', '/');
    if (!filePath.startsWith(`${rootPath}/`)) throw new Error('Choose a file inside the open workspace.');
    selectedPath = filePath.slice(rootPath.length + 1);
    await openDocument(selectedPath);
  } catch (error) { showError(error); }
}

async function refreshProjects(): Promise<void> {
  try {
    const list = expect(await client.call({ method: 'list_projects' }), 'projects').value;
    renderProjectList(list.map((project) => `${project.display_name} · ${project.provider_id}`));
  } catch (error) { showError(error); }
}

async function discoverCargo(): Promise<void> {
  if (!workspaceRoot) { setStatus('Open a workspace first'); return; }
  try {
    const response = expect(await client.call({ method: 'discover_cargo_projects', params: { max_directories: 6000, max_projects: 256, max_depth: 32 } }), 'cargo_projects');
    renderCargoProjects(response.value);
    const tasks = expect(await client.call({ method: 'list_tasks' }), 'tasks').value;
    taskById.clear(); for (const task of tasks) taskById.set(task.task_id, task);
    updateRunConfigurations(tasks);
    renderTasks(tasks); showPanel('tasks'); setStatus(`Found ${response.value.length} Cargo project${response.value.length === 1 ? '' : 's'}`);
    for (const button of document.querySelectorAll<HTMLButtonElement>('[data-action="run-cargo"]')) button.disabled = !tasks.some((task) => task.key.endsWith(`.${button.dataset.command}`));
  } catch (error) { showError(error); }
}

function renderCargoProjects(projects: CargoProjectSummary[]): void {
  projectHost.replaceChildren();
  for (const project of projects) {
    const row = document.createElement('div'); row.className = 'cargo-project';
    const icon = document.createElement('span'); icon.className = 'project-icon'; icon.textContent = '▱'; icon.setAttribute('aria-hidden', 'true');
    const info = document.createElement('div'); info.className = 'project-info';
    const name = document.createElement('strong'); name.textContent = project.project.display_name;
    const meta = document.createElement('span'); meta.textContent = `${project.is_workspace ? 'Workspace' : 'Package'} · ${project.members.length} members`;
    info.append(name, meta); row.append(icon, info); projectHost.append(row);
  }
  if (!projects.length) projectHost.innerHTML = '<div class="panel-empty">No Cargo projects found in this workspace.</div>';
}

function renderProjectList(projects: string[]): void {
  projectHost.replaceChildren();
  for (const display of projects) {
    const row = document.createElement('div'); row.className = 'cargo-project'; row.textContent = display; projectHost.append(row);
  }
  if (!projects.length) projectHost.innerHTML = '<div class="panel-empty">Project information will appear here.</div>';
}

function renderTasks(tasks: TaskSummary[]): void {
  taskHost.replaceChildren();
  for (const task of tasks) {
    if (task.category === 'custom' || task.key.endsWith('.metadata')) continue;
    const row = document.createElement('div'); row.className = 'task-row';
    const details = document.createElement('div'); details.className = 'task-info';
    const title = document.createElement('span'); title.className = 'task-title'; title.textContent = task.display_name;
    const summary = document.createElement('span'); summary.className = 'task-summary'; summary.textContent = `${task.category} · ${task.working_directory}`;
    details.append(title, summary);
    const run = document.createElement('button'); run.className = 'small-button'; run.textContent = 'Run'; run.addEventListener('click', () => void requestTask(task));
    row.append(details, run); taskHost.append(row);
  }
  if (!taskHost.childElementCount) taskHost.innerHTML = '<div class="panel-empty">Discover Cargo projects to load available tasks.</div>';
}

function updateRunConfigurations(tasks: TaskSummary[]): void {
  const configurations = runConfigurationTasks(tasks);
  runConfiguration.replaceChildren();
  if (!configurations.length) {
    const option = document.createElement('option'); option.value = ''; option.textContent = 'No run configuration';
    runConfiguration.append(option); runConfiguration.disabled = true;
  } else {
    for (const task of configurations) {
      const option = document.createElement('option'); option.value = String(task.task_id); option.textContent = task.display_name;
      runConfiguration.append(option);
    }
    runConfiguration.value = String(configurations[0]!.task_id); runConfiguration.disabled = false;
  }
  syncRunButton();
}

function syncRunButton(): void {
  const task = selectedRunTask([...taskById.values()], runConfiguration.value);
  document.querySelector<HTMLButtonElement>('#toolbar-run')!.disabled = !task || runId !== null;
}

async function runSelectedConfiguration(): Promise<void> {
  const task = selectedRunTask([...taskById.values()], runConfiguration.value);
  if (!task) { setStatus('Discover a runnable project first'); return; }
  await requestTask(task);
}

async function runCargoCommand(operation: string): Promise<void> {
  if (!taskById.size) { setStatus('Discover Cargo projects first'); return; }
  const task = [...taskById.values()].find((candidate) => candidate.key === `cargo.workspace.${operation}`)
    ?? [...taskById.values()].find((candidate) => candidate.key.endsWith(`.${operation}`));
  if (!task) { setStatus(`No Cargo ${operation} task is available`); return; }
  await requestTask(task);
}

async function requestTask(task: TaskSummary): Promise<void> {
  try {
    showPanel('output'); setBottomCollapsed(false);
    const response = await client.call({ method: 'request_task_execution', params: { task_id: task.task_id } });
    if (response.kind === 'approval_required') await showApproval(response.value);
    else if (response.kind === 'task_started') {
      runId = response.value.run_id; setCancelEnabled(true); setStatus(`${task.display_name} started`); await updateTaskOutput(runId);
    } else throw new Error(`Unexpected task response: ${response.kind}`);
  } catch (error) { showError(error); }
}

async function showApproval(approval: ExecutionApprovalSummary): Promise<void> {
  const details = document.querySelector<HTMLDListElement>('#approval-details')!; details.replaceChildren();
  const values: Array<[string, string]> = [
    ['Task', approval.display_name], ['Executable', approval.executable], ['Arguments', JSON.stringify(approval.arguments)],
    ['Working directory', approval.working_directory], ['Project', approval.project_id?.toString() ?? 'Workspace'],
    ['Actor', approval.requesting_actor], ['Environment keys', approval.environment_keys.join(', ') || 'None'],
  ];
  for (const [key, value] of values) { const dt = document.createElement('dt'); dt.textContent = key; const dd = document.createElement('dd'); dd.textContent = value; details.append(dt, dd); }
  const dialog = document.querySelector<HTMLDialogElement>('#approval-dialog')!; dialog.showModal();
  const action = approvalChoice(await new Promise<string>((resolve) => dialog.addEventListener('close', () => resolve(dialog.returnValue), { once: true })));
  if (!action) return;
  try {
    const response = await client.call(action === 'approve' ? { method: 'approve_task_execution', params: { request_id: approval.request_id } } : { method: 'deny_task_execution', params: { request_id: approval.request_id } });
    if (response.kind === 'task_approved') { runId = response.value.run_id; setCancelEnabled(true); setStatus('Task approved and started'); await updateTaskOutput(runId); }
    else if (response.kind === 'task_denied') setStatus('Task denied');
  } catch (error) { showError(error); }
}

async function updateTaskOutput(id: number): Promise<void> {
  try {
    const snapshot = expect(await client.call({ method: 'task_output', params: { run_id: id } }), 'task_snapshot').value;
    outputHost.textContent = visibleOutput(snapshot) || 'No task output.';
    document.querySelector('#bottom-task-state')!.textContent = taskStatus(snapshot);
    document.querySelector('#status-task')?.remove();
    if (snapshot.state !== 'running') {
      runId = null;
      setCancelEnabled(false);
      setStatus(`${taskById.get(snapshot.task_id)?.display_name ?? 'Task'} ${taskStatus(snapshot)}`);
    }
  } catch (error) { showError(error); }
}

function setCancelEnabled(enabled: boolean): void {
  const button = document.querySelector<HTMLButtonElement>('[data-action="cancel-task"]')!;
  button.disabled = !enabled;
  syncRunButton();
}
function queueOutputRefresh(id: number): void {
  if (outputRefreshTimer) window.clearTimeout(outputRefreshTimer);
  outputRefreshTimer = window.setTimeout(() => { outputRefreshTimer = 0; void updateTaskOutput(id); }, 100);
}
function handlePlatformEvent(event: PlatformEvent): void {
  if (event.kind.startsWith('terminal_')) { terminalPanel?.handleEvents([event]); return; }
  if (event.kind === 'git_status_changed' && gitStatus?.root === event.root) void refreshGitStatus();
  if (event.kind === 'git_operation_completed' && event.operation) setStatus(`Git ${event.operation}${event.commit ? ` · ${event.commit.slice(0, 8)}` : ''}`);
  if (event.kind.startsWith('workspace_entry_') && event.path) {
    const parent = parentPath(event.path); void refreshWorkspacePath(parent);
    if (event.operation === 'renamed' && event.previous_path) {
      for (const tab of tabs.tabs) if (tab.path === event.previous_path || tab.path.startsWith(`${event.previous_path}/`)) {
        tab.path = `${event.path}${tab.path.slice(event.previous_path.length)}`; tab.title = baseName(tab.path); tab.language = languageNameForPath(tab.path);
        void refreshDocumentLanguage(tab.id);
      }
      renderTabs(); updateStatusDocument();
    }
  }
  if (event.kind === 'document_path_changed' && event.document_id && event.path) {
    const tab = tabs.tabs.find((item) => item.id === event.document_id);
    if (tab) { tab.path = event.path; tab.title = baseName(event.path); tab.language = languageNameForPath(event.path); void refreshDocumentLanguage(tab.id); renderTabs(); updateStatusDocument(); }
    if (event.document_id) { diagnosticsByDocument.delete(event.document_id); inspectionFindingsByDocument.delete(event.document_id); renderProblems(); scheduleInspection(event.document_id); }
  }
  if (event.kind === 'document_closed' && event.document_id && tabs.tabs.some((tab) => tab.id === event.document_id)) {
    documentViews.delete(event.document_id); tabs.close(event.document_id); renderTabs();
    if (tabs.active) selectTab(tabs.active.id); else { editorHost.classList.add('hidden'); welcome.classList.remove('hidden'); editor = null; updateStatusDocument(); }
  }
  if (event.kind === 'task_output_available' && event.run_id) queueOutputRefresh(event.run_id);
  if (event.kind === 'task_finished' && event.run_id) void updateTaskOutput(event.run_id);
  if (event.kind === 'save_conflict_detected') setStatus('File changed outside Koren; review the save conflict.');
  if (event.kind === 'language_diagnostics' && event.document_id && event.diagnostics) {
    const providers = withDiagnosticPublication(diagnosticsByDocument.get(event.document_id), event.diagnostics);
    diagnosticsByDocument.set(event.document_id, providers);
    renderLanguageDiagnostics(event.document_id); renderProblems();
  }
  if (event.kind === 'language_server_error' && event.message) setStatus(event.message);
  if (event.kind === 'language_server_stopped' && event.server_id) {
    for (const [key, id] of activeLanguageServers) if (id === event.server_id) { activeLanguageServers.delete(key); clearDiagnosticProviders(key); }
    if (event.message) setStatus(event.message);
  }
  if (extensionCatalogCache && ['workspace_opened', 'workspace_closed', 'document_opened', 'document_closed'].includes(event.kind)) {
    void client.call({ method: 'list_extensions' }).then((response) => renderExtensions(expect(response, 'extensions').value)).catch(showError);
  }
}

function setTheme(theme: 'dark' | 'light'): void {
  document.documentElement.removeAttribute('data-extension-theme');
  for (const token of ['--bg', '--panel', '--raised', '--text', '--muted', '--border', '--accent', '--selected']) document.documentElement.style.removeProperty(token);
  document.documentElement.dataset.theme = theme;
  persistTheme(localStorage, theme);
  localStorage.removeItem('koren-code:theme-extension');
  document.querySelector<HTMLSelectElement>('#theme-setting')!.value = theme;
}
function applyExtensionTheme(theme: ExtensionThemeSummary): void {
  setTheme(theme.base === 'light' ? 'light' : 'dark');
  for (const [property, color] of Object.entries(safeThemeVariables(theme.tokens))) document.documentElement.style.setProperty(property, color);
  document.documentElement.dataset.extensionTheme = theme.id;
  localStorage.setItem('koren-code:theme-extension', theme.id);
  document.querySelector<HTMLSelectElement>('#theme-setting')!.value = `extension:${theme.id}`;
}
async function openSettings(): Promise<void> {
  try {
    const catalog = expect(await client.call({ method: 'list_extensions' }), 'extensions').value;
    renderExtensions(catalog);
    const configuration = expect(await client.call({ method: 'list_inspection_rules' }), 'inspection_configuration').value;
    renderInspectionSettings(configuration);
    document.querySelector<HTMLDialogElement>('#settings-dialog')!.showModal();
  }
  catch (error) { showError(error); }
}
function toggleTheme(): void { setTheme(nextTheme(document.documentElement.dataset.theme === 'light' ? 'light' : 'dark')); }

function showPanel(panel: 'output' | 'problems' | 'tasks' | 'projects' | string): void {
  if (panel === 'terminal') void loadTerminalPanel().catch(showError);
  for (const button of document.querySelectorAll<HTMLButtonElement>('[data-panel]')) {
    const selected = button.dataset.panel === panel;
    button.classList.toggle('selected', selected); button.setAttribute('aria-selected', String(selected));
  }
  for (const id of ['output', 'problems', 'tasks', 'projects', 'search', 'source-control', 'terminal']) document.querySelector(`#${id}`)!.classList.toggle('hidden', id !== panel);
  for (const host of document.querySelectorAll<HTMLElement>('[data-extension-panel]')) host.classList.toggle('hidden', host.dataset.extensionPanel !== panel);
  if (panel.startsWith('org.') || panel.includes('.')) { activeDynamicPanel = panel; localStorage.setItem('koren.activeExtensionPanel', panel); }
  setBottomCollapsed(false);
}

function scheduleInspection(documentId: number): void {
  const previous = inspectionTimers.get(documentId);
  if (previous !== undefined) window.clearTimeout(previous);
  const generation = (inspectionGenerations.get(documentId) ?? 0) + 1;
  inspectionGenerations.set(documentId, generation);
  inspectionTimers.set(documentId, window.setTimeout(() => {
    inspectionTimers.delete(documentId);
    void inspectDocument(documentId, generation);
  }, 300));
}

async function inspectDocument(documentId: number, generation: number): Promise<void> {
  const entry = documentViews.get(documentId);
  const tab = tabs.tabs.find((item) => item.id === documentId);
  if (!entry || !tab) return;
  try {
    await entry.pipeline.flush();
    const revision = entry.pipeline.currentRevision;
    const result = expect(await client.call({ method: 'inspect_document', params: {
      document_id: documentId, expected_revision: revision, language: languageIdForPath(tab.path),
    } }), 'inspection_findings').value;
    if (inspectionGenerations.get(documentId) !== generation || entry.pipeline.currentRevision !== result.document_revision) return;
    inspectionFindingsByDocument.set(documentId, result.findings.filter((finding) => finding.document_revision === result.document_revision));
    renderLanguageDiagnostics(documentId);
    renderProblems();
  } catch (error) {
    if (inspectionGenerations.get(documentId) !== generation) return;
    if (!(error instanceof KorenError) || error.code !== 'conflict') showError(error);
  }
}

function codeMirrorDiagnostics(documentId: number): CodeMirrorDiagnostic[] {
  const entry = documentViews.get(documentId);
  if (!entry) return [];
  const result: CodeMirrorDiagnostic[] = [];
  for (const { provider, diagnostic } of diagnosticsForRevision(diagnosticsByDocument.get(documentId), entry.pipeline.currentRevision)) {
    const from = editorPosition(entry.state.doc, diagnostic.range.start.line, diagnostic.range.start.character);
    const to = editorPosition(entry.state.doc, diagnostic.range.end.line, diagnostic.range.end.character);
    const severity = diagnostic.severity === 'information' || diagnostic.severity === null ? 'info' : diagnostic.severity;
    result.push({ from, to: Math.max(from, to), severity, message: diagnostic.message, source: diagnostic.source ?? provider });
  }
  for (const finding of inspectionFindingsByDocument.get(documentId) ?? []) {
    if (finding.document_revision !== entry.pipeline.currentRevision) continue;
    const from = editorPosition(entry.state.doc, finding.range.start.line, finding.range.start.character);
    const to = editorPosition(entry.state.doc, finding.range.end.line, finding.range.end.character);
    result.push({ from, to: Math.max(from, to), severity: finding.severity === 'information' ? 'info' : finding.severity, message: finding.message, source: finding.rule_id });
  }
  return result;
}

function editorPosition(doc: EditorState['doc'], lineNumber: number, column: number): number {
  const line = doc.line(Math.max(1, Math.min(doc.lines, lineNumber + 1)));
  return Math.max(line.from, Math.min(line.to, line.from + column));
}

function renderLanguageDiagnostics(documentId: number): void {
  if (tabs.activeId === documentId && editor) editor.dispatch(setDiagnostics(editor.state, codeMirrorDiagnostics(documentId)));
}

function renderProblems(): void {
  const host = document.querySelector<HTMLDivElement>('#problem-entries')!; host.replaceChildren();
  const rows = [...diagnosticsByDocument.entries()].flatMap(([documentId, providers]) => {
    const entry = documentViews.get(documentId);
    if (!entry) return [];
    return diagnosticsForRevision(providers, entry.pipeline.currentRevision)
      .map(({ diagnostic, provider }) => ({ documentId, diagnostic, provider }));
  }).filter(({ diagnostic }) => problemSeverityFilter === 'all' || (diagnostic.severity ?? 'information') === problemSeverityFilter);
  const inspectionRows = [...inspectionFindingsByDocument.entries()].flatMap(([documentId, findings]) => {
    const entry = documentViews.get(documentId);
    return entry ? filterInspectionFindings(findings.filter((finding) => finding.document_revision === entry.pipeline.currentRevision), problemSeverityFilter).map((finding) => ({ documentId, finding })) : [];
  });
  const historicalRows = filterInspectionFindings([...importedSarifFindings, ...projectSnapshotFindings], problemSeverityFilter);
  document.querySelector('#problem-count')!.textContent = String(rows.length + inspectionRows.length + historicalRows.length);
  const groups = new Set<string>();
  const groupHeading = (key: string): void => {
    if (problemGrouping === 'file' || groups.has(key)) return;
    groups.add(key);
    const heading = document.createElement('div'); heading.className = 'problem-group'; heading.textContent = key; host.append(heading);
  };
  for (const row of rows) {
    const rowHost = document.createElement('div'); rowHost.className = 'problem-entry';
    const button = document.createElement('button'); button.className = 'problem-row';
    const severity = document.createElement('span'); severity.className = `problem-severity ${row.diagnostic.severity ?? 'information'}`; severity.textContent = row.diagnostic.severity ?? 'info';
    const message = document.createElement('span'); message.className = 'problem-message'; message.textContent = row.diagnostic.message;
    const tab = tabs.tabs.find((item) => item.id === row.documentId);
    const location = document.createElement('span'); location.className = 'problem-location'; location.textContent = `${tab?.title ?? row.provider}:${row.diagnostic.range.start.line + 1}`;
    button.append(severity, message, location);
    const detail = extensionCatalogCache?.diagnostic_details.find((item: ExtensionDiagnosticDetailSummary) => diagnosticDetailMatches(item, row.diagnostic.source, row.diagnostic.code, row.provider));
    if (detail) {
      const details = document.createElement('details'); details.className = 'diagnostic-detail';
      const summary = document.createElement('summary'); summary.textContent = detail.title;
      const explanation = document.createElement('p'); explanation.textContent = detail.explanation; details.append(summary, explanation);
      for (const related of detail.related) { const note = document.createElement('p'); note.textContent = related; details.append(note); }
      if (detail.suggested_command) { const action = document.createElement('button'); action.textContent = 'Suggested action'; action.addEventListener('click', (event) => { event.stopPropagation(); void executeCommand(detail.suggested_command!); }); details.append(action); }
      rowHost.append(button, details);
    } else {
      rowHost.append(button);
    }
    button.addEventListener('click', () => {
      selectTab(row.documentId);
      if (editor) editor.dispatch({ selection: { anchor: editorPosition(editor.state.doc, row.diagnostic.range.start.line, row.diagnostic.range.start.character) }, scrollIntoView: true });
      showPanel('problems');
    });
    const key = problemGrouping === 'provider' ? row.provider : problemGrouping === 'category' ? row.diagnostic.source ?? row.provider : problemGrouping === 'severity' ? row.diagnostic.severity ?? 'information' : tab?.title ?? row.provider;
    groupHeading(key); host.append(rowHost);
  }
  for (const { documentId, finding } of inspectionRows) {
    const tab = tabs.tabs.find((item) => item.id === documentId);
    const key = inspectionFindingGroupKey(finding, problemGrouping, tab?.title ?? finding.path);
    groupHeading(key); renderInspectionFinding(host, documentId, finding);
  }
  for (const finding of historicalRows) {
    groupHeading(inspectionFindingGroupKey(finding, problemGrouping));
    renderImportedFinding(host, finding);
  }
  if (!rows.length && !inspectionRows.length && !historicalRows.length) host.innerHTML = '<div class="panel-empty">No problems reported by installed language integrations.</div>';
}

function renderInspectionFinding(host: HTMLElement, documentId: number, finding: InspectionFinding): void {
  const tab = tabs.tabs.find((item) => item.id === documentId);
  const row = document.createElement('div'); row.className = 'problem-entry';
  const button = document.createElement('button'); button.className = 'problem-row';
  const severity = document.createElement('span'); severity.className = `problem-severity ${finding.severity}`; severity.textContent = finding.severity;
  const message = document.createElement('span'); message.className = 'problem-message'; message.textContent = finding.message;
  const location = document.createElement('span'); location.className = 'problem-location'; location.textContent = `${tab?.title ?? finding.path}:${finding.range.start.line + 1}`;
  button.append(severity, message, location);
  button.addEventListener('click', () => {
    selectTab(documentId);
    if (editor) editor.dispatch({ selection: { anchor: editorPosition(editor.state.doc, finding.range.start.line, finding.range.start.character) }, scrollIntoView: true });
    showPanel('problems');
  });
  row.append(button);
  const details = document.createElement('details'); details.className = 'diagnostic-detail';
  const summary = document.createElement('summary'); summary.textContent = `${finding.rule_id} · ${finding.category ?? finding.provider_id} · ${finding.origin}`;
  details.append(summary);
  if (finding.explanation) { const explanation = document.createElement('p'); explanation.textContent = finding.explanation; details.append(explanation); }
  for (const related of finding.related) {
    const matchingTab = tabs.tabs.find((item) => item.path === related.path || item.path.endsWith(`/${related.path}`));
    if (matchingTab) {
      const link = document.createElement('button'); link.type = 'button';
      link.textContent = `${related.path}:${related.range.start.line + 1} — ${related.message}`;
      link.addEventListener('click', (event) => {
        event.stopPropagation(); selectTab(matchingTab.id);
        if (editor) editor.dispatch({ selection: { anchor: editorPosition(editor.state.doc, related.range.start.line, related.range.start.character) }, scrollIntoView: true });
      });
      details.append(link);
    } else {
      const note = document.createElement('p'); note.textContent = `${related.path}:${related.range.start.line + 1} — ${related.message}`; details.append(note);
    }
  }
  for (const action of finding.actions) {
    const fix = document.createElement('button'); fix.type = 'button'; fix.textContent = action.disabled_reason ? `${action.title} — ${action.disabled_reason}` : action.title;
    fix.disabled = Boolean(action.disabled_reason) || !action.edits.length;
    fix.addEventListener('click', (event) => { event.stopPropagation(); void applyInspectionAction(documentId, action); });
    details.append(fix);
  }
  row.append(details); host.append(row);
}

function renderImportedFinding(host: HTMLElement, finding: InspectionFinding): void {
  const row = document.createElement('div'); row.className = 'problem-entry';
  const button = document.createElement('button'); button.className = 'problem-row';
  const severity = document.createElement('span'); severity.className = `problem-severity ${finding.severity}`; severity.textContent = finding.severity;
  const message = document.createElement('span'); message.className = 'problem-message'; message.textContent = finding.message;
  const historicalSource = finding.origin === 'imported' ? 'Imported SARIF report' : 'Project snapshot';
  const location = document.createElement('span'); location.className = 'problem-location'; location.textContent = `${finding.path}:${finding.range.start.line + 1} · ${historicalSource}`;
  button.append(severity, message, location);
  const matchingTab = tabs.tabs.find((tab) => tab.path === finding.path || tab.path.endsWith(`/${finding.path}`));
  if (matchingTab) button.addEventListener('click', () => {
    selectTab(matchingTab.id);
    if (editor) editor.dispatch({ selection: { anchor: editorPosition(editor.state.doc, finding.range.start.line, finding.range.start.character) }, scrollIntoView: true });
  });
  const detail = document.createElement('details'); detail.className = 'diagnostic-detail';
  const summary = document.createElement('summary'); summary.textContent = `${finding.rule_id} · ${finding.category ?? finding.provider_id} · ${historicalSource}`;
  detail.append(summary);
  const source = document.createElement('p'); source.textContent = `${historicalSource}${finding.source_id ? ` ${finding.source_id}` : ''}${finding.source_revision ? ` from source revision ${finding.source_revision}` : ' with no source revision recorded'}. It is not a live diagnostic.`; detail.append(source);
  if (finding.explanation) { const explanation = document.createElement('p'); explanation.textContent = finding.explanation; detail.append(explanation); }
  for (const related of finding.related) {
    const relatedTab = tabs.tabs.find((tab) => tab.path === related.path || tab.path.endsWith(`/${related.path}`));
    if (relatedTab) {
      const link = document.createElement('button'); link.type = 'button'; link.textContent = `${related.path}:${related.range.start.line + 1} — ${related.message}`;
      link.addEventListener('click', () => {
        selectTab(relatedTab.id);
        if (editor) editor.dispatch({ selection: { anchor: editorPosition(editor.state.doc, related.range.start.line, related.range.start.character) }, scrollIntoView: true });
      }); detail.append(link);
    } else {
      const note = document.createElement('p'); note.textContent = `${related.path}:${related.range.start.line + 1} — ${related.message}`; detail.append(note);
    }
  }
  row.append(button, detail); host.append(row);
}

async function importSarif(): Promise<void> {
  try {
    const selection = await fetch('/file-picker', { cache: 'no-store' });
    const { path } = await selection.json() as { path: string | null };
    if (!path) return;
    const imported = expect(await client.call({ method: 'import_sarif', params: { path } }), 'imported_sarif').value;
    importedSarifFindings = imported.findings;
    renderProblems(); showPanel('problems');
    setStatus(`Imported ${imported.findings.length} historical findings${imported.warnings.length ? ` with ${imported.warnings.length} warnings` : ''}`);
  } catch (error) { showError(error); }
}

async function exportSarif(): Promise<void> {
  try {
    const report = expect(await client.call({ method: 'export_sarif' }), 'sarif_report').value;
    const url = URL.createObjectURL(new Blob([report.document], { type: 'application/sarif+json' }));
    const anchor = document.createElement('a'); anchor.href = url; anchor.download = 'koren-inspections.sarif'; anchor.click(); URL.revokeObjectURL(url);
    setStatus(`Exported SARIF 2.1.0${report.warnings.length ? ` with ${report.warnings.length} warnings` : ''}`);
  } catch (error) { showError(error); }
}

async function startProjectInspection(): Promise<void> {
  if (activeProjectInspection !== null) { setStatus('A project inspection is already active'); return; }
  try {
    const result = expect(await client.call({ method: 'start_project_inspection' }), 'project_inspection_started').value;
    activeProjectInspection = result.job_id;
    latestProjectInspectionId = result.job_id;
    projectInspectionOffset = 0;
    projectSnapshotFindings = [];
    const status = document.querySelector<HTMLElement>('#project-analysis-status')!;
    status.textContent = 'Starting explicit analysis of the currently open document snapshot…';
    document.querySelector<HTMLButtonElement>('[data-action="cancel-project-analysis"]')!.disabled = false;
    await pollProjectInspection(result.job_id);
  } catch (error) { showError(error); }
}

async function pollProjectInspection(jobId: number): Promise<void> {
  try {
    const summary: ProjectInspectionSummary = expect(await client.call({ method: 'project_inspection_status', params: { job_id: jobId } }), 'project_inspection_status').value;
    const status = document.querySelector<HTMLElement>('#project-analysis-status')!;
    status.textContent = `${summary.state}: ${summary.completed_documents}/${summary.total_documents} documents${summary.current_path ? ` · ${summary.current_path}` : ''}${summary.result_count ? ` · ${summary.result_count} findings` : ''}${summary.error ? ` · ${summary.error}` : ''}`;
    if (summary.state === 'running' || summary.state === 'cancelling') {
      projectInspectionTimer = window.setTimeout(() => void pollProjectInspection(jobId), 200);
      return;
    }
    activeProjectInspection = null;
    document.querySelector<HTMLButtonElement>('[data-action="cancel-project-analysis"]')!.disabled = true;
    if (summary.state === 'completed') {
      await loadProjectInspectionFindings(jobId, 0);
      showPanel('problems');
    }
  } catch (error) {
    activeProjectInspection = null;
    document.querySelector<HTMLButtonElement>('[data-action="cancel-project-analysis"]')!.disabled = true;
    showError(error);
  }
}

async function loadProjectInspectionFindings(jobId: number, offset: number): Promise<void> {
  try {
    const page = expect(await client.call({ method: 'project_inspection_findings', params: { job_id: jobId, offset, limit: 100 } }), 'project_inspection_findings').value;
    projectSnapshotFindings = offset === 0 ? page.findings : [...projectSnapshotFindings, ...page.findings];
    projectInspectionOffset = page.next_offset;
    const loadMore = document.querySelector<HTMLButtonElement>('[data-action="load-project-findings"]')!;
    loadMore.hidden = projectInspectionOffset === null;
    renderProblems();
  } catch (error) { showError(error); }
}

async function cancelProjectInspection(): Promise<void> {
  if (activeProjectInspection === null) return;
  try {
    await client.call({ method: 'cancel_project_inspection', params: { job_id: activeProjectInspection } });
    window.clearTimeout(projectInspectionTimer);
    await pollProjectInspection(activeProjectInspection);
  } catch (error) { showError(error); }
}

async function applyInspectionAction(documentId: number, action: InspectionFinding['actions'][number]): Promise<void> {
  const entry = documentViews.get(documentId);
  const tab = tabs.tabs.find((item) => item.id === documentId);
  if (!entry || !tab || !editor || tabs.activeId !== documentId) return;
  try {
    await entry.pipeline.flush();
    const revision = entry.pipeline.currentRevision;
    const edits = revisionBoundEdits(action, tab.path, revision);
    if (!edits) {
      setStatus('This quick fix is stale or targets another file. Run inspection again.');
      return;
    }
    const doc = editor.state.doc;
    const changes = edits.map((edit) => ({
      from: editorPosition(doc, edit.range.start.line, edit.range.start.character),
      to: editorPosition(doc, edit.range.end.line, edit.range.end.character),
      insert: edit.new_text,
    })).sort((left, right) => right.from - left.from);
    if (changes.some((change) => change.to < change.from || change.to > doc.length)) return;
    const before = doc.toString();
    const after = changes.reduce((text, change) => text.slice(0, change.from) + change.insert + text.slice(change.to), before);
    if (!window.confirm(`Preview: ${action.title}\n\nBefore:\n${before.slice(0, 800)}\n\nAfter:\n${after.slice(0, 800)}`)) return;
    if (entry.pipeline.currentRevision !== revision) return;
    editor.dispatch({ changes });
  } catch (error) { showError(error); }
}

function clearDiagnosticProviders(prefix: string): void {
  for (const [documentId, providers] of diagnosticsByDocument) {
    for (const provider of providers.keys()) if (provider === prefix || provider.startsWith(`${prefix}.`)) providers.delete(provider);
    if (!providers.size) diagnosticsByDocument.delete(documentId);
    renderLanguageDiagnostics(documentId);
  }
  for (const [documentId, findings] of inspectionFindingsByDocument) {
    const remaining = findings.filter((finding) => finding.provider_id !== prefix && !finding.provider_id.startsWith(`${prefix}.`));
    if (remaining.length) inspectionFindingsByDocument.set(documentId, remaining);
    else inspectionFindingsByDocument.delete(documentId);
    renderLanguageDiagnostics(documentId);
  }
  renderProblems();
}

async function openCommandPalette(): Promise<void> {
  try {
    registeredCommands = expect(await client.call({ method: 'list_commands' }), 'commands').value;
    const catalog = expect(await client.call({ method: 'list_extensions' }), 'extensions').value;
    updateCommandCatalog(catalog);
    extensionCatalogCache = catalog;
    renderCommandResults('');
    const dialog = document.querySelector<HTMLDialogElement>('#command-dialog')!;
    const input = document.querySelector<HTMLInputElement>('#command-search')!;
    input.value = ''; dialog.showModal(); input.focus();
  } catch (error) { showError(error); }
}

async function openExtensions(): Promise<void> {
  try {
    const catalog = expect(await client.call({ method: 'list_extensions' }), 'extensions').value;
    renderExtensions(catalog);
    document.querySelector<HTMLDialogElement>('#extensions-dialog')!.showModal();
  } catch (error) { showError(error); }
}

function renderExtensions(catalog: ExtensionCatalog): void {
  extensionCatalogCache = catalog;
  updateCommandCatalog(catalog);
  editorContextMenus = extensionMenusAt(catalog.menu_items, 'editor_context');
  projectExplorerMenus = extensionMenusAt(catalog.menu_items, 'project_explorer_context');
  for (const [location, hostId] of [['tools', 'extension-tools-items'], ['navigate', 'extension-navigate-items']] as const) {
    const menu = document.querySelector<HTMLDivElement>(`#${hostId}`)!; menu.replaceChildren();
    for (const item of extensionMenusAt(catalog.menu_items, location)) {
      const button = document.createElement('button'); button.setAttribute('role', 'menuitem'); button.textContent = item.label;
      button.disabled = !item.enabled;
      button.addEventListener('click', () => void executeCommand(item.command_id)); menu.append(button);
    }
    if (menu.childElementCount) menu.prepend(Object.assign(document.createElement('div'), { className: 'menu-separator' }));
  }
  renderToolWindows(catalog.tool_windows);
  renderExtensionSettings(catalog.settings, catalog.themes);
  renderProjectWizards(catalog.project_wizards);
  const host = document.querySelector<HTMLDivElement>('#extension-list')!;
  host.replaceChildren();
  for (const extension of catalog.extensions) {
    const row = document.createElement('article'); row.className = 'extension-row';
    const details = document.createElement('div'); details.className = 'extension-details';
    const title = document.createElement('strong'); title.textContent = extension.name;
    const meta = document.createElement('span'); meta.textContent = `${extension.id} · ${extension.version} · ${extension.state}`;
    const description = document.createElement('p'); description.textContent = extension.description;
    details.append(title, meta, description);
    const actions = document.createElement('div'); actions.className = 'extension-actions';
    const toggle = document.createElement('button'); toggle.className = 'small-button';
    toggle.textContent = extensionToggleLabel(extension);
    toggle.addEventListener('click', () => void changeExtension(extension, !extension.enabled));
    actions.append(toggle);
    if (extensionCanActivate(extension)) {
      const activate = document.createElement('button'); activate.className = 'small-button'; activate.textContent = 'Activate';
      activate.addEventListener('click', () => void activateExtension(extension.id)); actions.append(activate);
    }
    const serverConfigs = catalog.language_servers.filter((server) => server.extension_id === extension.id);
    for (const server of serverConfigs) {
      const key = `${server.extension_id}.${server.configuration_id}`;
      const control = document.createElement('button'); control.className = 'small-button';
      const activeId = activeLanguageServers.get(key);
      control.textContent = activeId ? `Stop ${server.configuration_id}` : `Start ${server.configuration_id}`;
      control.disabled = !languageServerEnabled(extension, server);
      control.addEventListener('click', () => activeId ? void stopLanguageServer(key, activeId) : void startLanguageServer(server));
      actions.append(control);
    }
    row.append(details, actions);
    if (extension.error) { const error = document.createElement('p'); error.className = 'extension-error'; error.textContent = extension.error; row.append(error); }
    host.append(row);
  }
  if (!catalog.extensions.length) host.innerHTML = '<div class="panel-empty">No local extensions installed.</div>';
}

function updateCommandCatalog(catalog: ExtensionCatalog): void {
  commandLabels.clear(); commandDescriptions.clear(); commandMetadata.clear();
  for (const command of [...catalog.built_in_commands, ...catalog.commands]) {
    commandLabels.set(command.id, command.title); commandDescriptions.set(command.id, command.description);
    commandMetadata.set(command.id, { description: command.description, shortcut: command.shortcut, enabled: command.enabled });
  }
  registeredCommands = [...catalog.built_in_commands, ...catalog.commands].map((command) => command.id);
}

function loadOpenExtensionPanels(): string[] {
  try { const value: unknown = JSON.parse(localStorage.getItem('koren.openExtensionPanels') ?? '[]'); return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : []; } catch { return []; }
}
function saveOpenExtensionPanels(): void { localStorage.setItem('koren.openExtensionPanels', JSON.stringify(openExtensionPanels)); }

function renderToolWindows(windows: ExtensionToolWindowSummary[]): void {
  const available = new Map(windows.map((panel) => [panel.id, panel]));
  openExtensionPanels = activePanelLayout(openExtensionPanels, [...available.keys()]);
  saveOpenExtensionPanels();
  if (!available.has(activeDynamicPanel)) { activeDynamicPanel = ''; localStorage.removeItem('koren.activeExtensionPanel'); }
  const menu = document.querySelector<HTMLDivElement>('#extension-panel-menu-items')!; menu.replaceChildren();
  const hosts = document.querySelector<HTMLDivElement>('#extension-panel-content')!; hosts.replaceChildren();
  const tabs = document.querySelector<HTMLDivElement>('.tool-tabs')!;
  for (const panel of windows) {
    const open = document.createElement('button'); open.setAttribute('role', 'menuitem'); open.textContent = panel.title;
    open.addEventListener('click', () => openExtensionPanel(panel.id)); menu.append(open);
    if (!openExtensionPanels.includes(panel.id)) continue;
    const tab = document.createElement('button'); tab.dataset.panel = panel.id; tab.setAttribute('role', 'tab'); tab.setAttribute('aria-selected', String(activeDynamicPanel === panel.id)); tab.classList.toggle('selected', activeDynamicPanel === panel.id);
    const title = document.createElement('span'); title.textContent = panel.title;
    const close = document.createElement('button'); close.type = 'button'; close.className = 'extension-panel-close'; close.textContent = '×'; close.setAttribute('aria-label', `Close ${panel.title}`);
    close.addEventListener('click', (event) => {
      event.stopPropagation(); openExtensionPanels = openExtensionPanels.filter((id) => id !== panel.id); saveOpenExtensionPanels();
      if (activeDynamicPanel === panel.id) { activeDynamicPanel = ''; localStorage.removeItem('koren.activeExtensionPanel'); showPanel('output'); }
      if (extensionCatalogCache) renderToolWindows(extensionCatalogCache.tool_windows);
    });
    tab.append(title);
    tab.addEventListener('click', () => showPanel(panel.id)); tabs.append(tab, close);
    const host = document.createElement('div'); host.className = 'panel-content extension-panel-view hidden'; host.dataset.extensionPanel = panel.id;
    renderExtensionDocument(host, panel.initial_view); hosts.append(host);
  }
  if (activeDynamicPanel && openExtensionPanels.includes(activeDynamicPanel)) showPanel(activeDynamicPanel);
}

function openExtensionPanel(panelId: string): void {
  if (!openExtensionPanels.includes(panelId)) openExtensionPanels.push(panelId);
  activeDynamicPanel = panelId; saveOpenExtensionPanels();
  if (extensionCatalogCache) renderToolWindows(extensionCatalogCache.tool_windows);
  showPanel(panelId);
}

function renderExtensionDocument(host: HTMLElement, view: ExtensionUiDocument): void {
  host.replaceChildren();
  for (const element of view.elements) {
    let node: HTMLElement;
    switch (element.kind) {
      case 'text': node = document.createElement('p'); node.textContent = element.text; break;
      case 'heading': node = document.createElement('h3'); node.textContent = element.text; break;
      case 'button': { const button = document.createElement('button'); button.textContent = element.label; button.addEventListener('click', () => void executeCommand(element.command_id)); node = button; break; }
      case 'list': { const list = document.createElement('ul'); for (const text of element.items) { const item = document.createElement('li'); item.textContent = text; list.append(item); } node = list; break; }
      case 'table': { const table = document.createElement('table'); for (const row of element.rows) { const tr = document.createElement('tr'); for (const text of row) { const cell = document.createElement('td'); cell.textContent = text; tr.append(cell); } table.append(tr); } node = table; break; }
      case 'input': { const label = document.createElement('label'); label.textContent = element.label; const input = document.createElement('input'); input.value = element.value; input.maxLength = element.max_length; input.required = element.required; label.append(input); node = label; break; }
      case 'select': { const label = document.createElement('label'); label.textContent = element.label; const select = document.createElement('select'); for (const value of element.options) { const option = document.createElement('option'); option.value = value; option.textContent = value; select.append(option); } select.value = element.value; label.append(select); node = label; break; }
      case 'checkbox': { const label = document.createElement('label'); const input = document.createElement('input'); input.type = 'checkbox'; input.checked = element.checked; label.append(input, document.createTextNode(element.label)); node = label; break; }
      case 'status': node = document.createElement('p'); node.className = `extension-status ${element.tone}`; node.textContent = element.label; break;
    }
    node.dataset.componentId = element.id; host.append(node);
  }
  if (!view.elements.length) { const empty = document.createElement('div'); empty.className = 'panel-empty'; empty.textContent = 'This extension panel has no content yet.'; host.append(empty); }
}

function renderExtensionSettings(settings: ExtensionSettingSummary[], themes: ExtensionThemeSummary[]): void {
  const host = document.querySelector<HTMLDivElement>('#extension-settings')!; host.replaceChildren();
  const themeSelect = document.querySelector<HTMLSelectElement>('#theme-setting')!;
  const selected = themeSelect.value;
  for (const option of [...themeSelect.options].filter((item) => item.value.startsWith('extension:'))) option.remove();
  for (const theme of themes) { const option = document.createElement('option'); option.value = `extension:${theme.id}`; option.textContent = theme.title; themeSelect.append(option); }
  if ([...themeSelect.options].some((option) => option.value === selected)) themeSelect.value = selected;
  const persistedTheme = localStorage.getItem('koren-code:theme-extension');
  const selectedTheme = themes.find((theme) => theme.id === persistedTheme);
  if (selectedTheme) { themeSelect.value = `extension:${selectedTheme.id}`; applyExtensionTheme(selectedTheme); }
  else if (persistedTheme) setTheme(readTheme(localStorage));
  if (!settings.length) return;
  const heading = document.createElement('h3'); heading.textContent = 'Extension settings'; host.append(heading);
  for (const setting of settings) {
    const label = document.createElement('label'); label.textContent = setting.title;
    let control: HTMLInputElement | HTMLSelectElement;
    if (setting.kind.type === 'choice') { const select = document.createElement('select'); for (const value of setting.kind.options) { const option = document.createElement('option'); option.value = value; option.textContent = value; select.append(option); } select.value = String(setting.value); control = select; }
    else { const input = document.createElement('input'); input.type = setting.kind.type === 'boolean' ? 'checkbox' : setting.kind.type === 'integer' ? 'number' : 'text'; if (input.type === 'checkbox') input.checked = Boolean(setting.value); else { input.value = String(setting.value); if (setting.kind.type === 'integer') { input.min = String(setting.kind.min); input.max = String(setting.kind.max); } if (setting.kind.type === 'text') input.maxLength = setting.kind.max_length; } control = input; }
    control.dataset.settingId = setting.id;
    control.addEventListener('change', () => { const value: ExtensionSettingValue = setting.kind.type === 'boolean' ? (control as HTMLInputElement).checked : setting.kind.type === 'integer' ? Number(control.value) : control.value; void updateExtensionSetting(setting.id, value); });
    label.append(control); if (setting.description) { const help = document.createElement('small'); help.textContent = setting.description; label.append(help); } host.append(label);
  }
}

async function updateExtensionSetting(settingId: string, value: ExtensionSettingValue): Promise<void> {
  try {
    const setting = extensionCatalogCache?.settings.find((item) => item.id === settingId);
    if (!setting || !validExtensionSettingValue(setting.kind, value)) throw new Error('The setting value is outside the extension’s declared schema.');
    await client.call({ method: 'set_extension_setting', params: { setting_id: settingId, value } }); const catalog = expect(await client.call({ method: 'list_extensions' }), 'extensions').value; renderExtensions(catalog); setStatus(`Updated ${settingId}`);
  }
  catch (error) { showError(error); }
}

function renderInspectionSettings(configuration: InspectionConfigurationSummary): void {
  const host = document.querySelector<HTMLDivElement>('#inspection-settings')!; host.replaceChildren();
  const profileLabel = document.createElement('label'); profileLabel.textContent = 'Profile';
  const profile = document.createElement('select');
  for (const name of configuration.profiles) { const option = document.createElement('option'); option.value = name; option.textContent = name; profile.append(option); }
  profile.value = configuration.selected_profile;
  const createProfile = document.createElement('button'); createProfile.type = 'button'; createProfile.textContent = 'New Profile';
  createProfile.addEventListener('click', () => {
    const name = window.prompt('Name the new inspection profile');
    if (name?.trim()) void createInspectionProfile(name, workspaceOnly.checked);
  });
  const workspaceOnly = document.createElement('input'); workspaceOnly.type = 'checkbox';
  workspaceOnly.checked = configuration.workspace_specific;
  const workspaceLabel = document.createElement('label'); workspaceLabel.append(workspaceOnly, document.createTextNode('Apply changes to this workspace only'));
  profile.addEventListener('change', () => void updateInspectionProfile(profile.value, workspaceOnly.checked));
  profileLabel.append(profile); host.append(profileLabel, createProfile, workspaceLabel);
  if (!configuration.rules.length) {
    const note = document.createElement('p'); note.className = 'subtle'; note.textContent = 'Enable and activate an inspection extension to configure its rules.'; host.append(note); return;
  }
  for (const rule of configuration.rules) {
    const row = document.createElement('div'); row.className = 'inspection-rule';
    const enabled = document.createElement('input'); enabled.type = 'checkbox'; enabled.checked = rule.enabled;
    enabled.setAttribute('aria-label', `Enable ${rule.title}`);
    const label = document.createElement('div'); label.className = 'inspection-rule-description';
    const title = document.createElement('strong'); title.textContent = rule.title;
    const detail = document.createElement('small'); detail.textContent = `${rule.id} · ${rule.category}${rule.workspace_override ? ' · workspace override' : ''}`;
    label.append(title, detail);
    const severity = document.createElement('select'); severity.setAttribute('aria-label', `${rule.title} severity`);
    for (const value of ['error', 'warning', 'information', 'hint'] as const) { const option = document.createElement('option'); option.value = value; option.textContent = value; severity.append(option); }
    severity.value = rule.severity;
    const save = (): void => void updateInspectionRule(rule.id, enabled.checked, severity.value, workspaceOnly.checked, false);
    enabled.addEventListener('change', save); severity.addEventListener('change', save);
    const reset = document.createElement('button'); reset.type = 'button'; reset.className = 'secondary'; reset.textContent = 'Reset';
    reset.title = 'Restore this rule to provider defaults';
    reset.addEventListener('click', () => void updateInspectionRule(rule.id, true, null, workspaceOnly.checked, true));
    row.append(enabled, label, severity, reset); host.append(row);
  }
}

async function updateInspectionProfile(profileName: string, workspaceSpecific: boolean): Promise<void> {
  try {
    const value = expect(await client.call({ method: 'select_inspection_profile', params: { profile_name: profileName, workspace_specific: workspaceSpecific } }), 'inspection_configuration').value;
    renderInspectionSettings(value); refreshInspectionsForOpenDocuments();
  } catch (error) { showError(error); }
}

async function createInspectionProfile(profileName: string, workspaceSpecific: boolean): Promise<void> {
  try {
    const value = expect(await client.call({ method: 'create_inspection_profile', params: { profile_name: profileName, workspace_specific: workspaceSpecific } }), 'inspection_configuration').value;
    renderInspectionSettings(value); refreshInspectionsForOpenDocuments();
  } catch (error) { showError(error); }
}

async function updateInspectionRule(ruleId: string, enabled: boolean, severity: string | null, workspaceSpecific: boolean, reset: boolean): Promise<void> {
  try {
    const value = expect(await client.call({ method: 'set_inspection_rule', params: { rule_id: ruleId, enabled, severity, workspace_specific: workspaceSpecific, reset } }), 'inspection_configuration').value;
    renderInspectionSettings(value); refreshInspectionsForOpenDocuments();
  } catch (error) { showError(error); }
}

function refreshInspectionsForOpenDocuments(): void {
  for (const tab of tabs.tabs) scheduleInspection(tab.id);
}

function renderProjectWizards(wizards: ExtensionProjectWizardSummary[]): void {
  const menu = document.querySelector<HTMLDivElement>('#extension-wizard-menu-items')!; menu.replaceChildren();
  for (const wizard of wizards) { const button = document.createElement('button'); button.setAttribute('role', 'menuitem'); button.textContent = `New ${wizard.title}…`; button.addEventListener('click', () => showProjectWizard(wizard)); menu.append(button); }
}

function showProjectWizard(wizard: ExtensionProjectWizardSummary): void {
  document.querySelector('#project-wizard-title')!.textContent = wizard.title;
  const fields = document.querySelector<HTMLDivElement>('#project-wizard-fields')!;
  renderExtensionDocument(fields, wizard.form);
  const form = document.querySelector<HTMLFormElement>('#project-wizard-form')!;
  form.onsubmit = (event) => {
    event.preventDefault();
    const invalidField = [...fields.querySelectorAll<HTMLInputElement | HTMLSelectElement>('input,select')].find((control) => !control.checkValidity());
    if (invalidField) { invalidField.reportValidity(); return; }
    document.querySelector<HTMLDialogElement>('#project-wizard-dialog')!.close();
    setStatus(`Validated ${wizard.title} fields. No project files were created.`);
  };
  document.querySelector<HTMLDialogElement>('#project-wizard-dialog')!.showModal();
}

async function startLanguageServer(server: LanguageServerSummary): Promise<void> {
  try {
    const result = await client.call({ method: 'start_language_server', params: { extension_id: server.extension_id, configuration_id: server.configuration_id } });
    if (result.kind === 'language_server_started') {
      activeLanguageServers.set(`${server.extension_id}.${server.configuration_id}`, result.value.server_id);
      renderExtensions(expect(await client.call({ method: 'list_extensions' }), 'extensions').value);
      setStatus(`Started language server ${server.configuration_id}`); return;
    }
    if (result.kind !== 'language_server_approval_required') throw new Error(`Unexpected response: ${result.kind}`);
    const decision = await askLanguageServerApproval(result.value);
    if (decision === 'approve') {
      const started = expect(await client.call({ method: 'approve_language_server', params: { approval_id: result.value.approval_id } }), 'language_server_started').value;
      activeLanguageServers.set(`${started.extension_id}.${started.configuration_id}`, started.server_id);
      renderExtensions(expect(await client.call({ method: 'list_extensions' }), 'extensions').value);
      setStatus(`Started language server ${started.configuration_id}`);
    } else {
      await client.call({ method: 'deny_language_server', params: { approval_id: result.value.approval_id } });
      setStatus('Language-server start denied');
    }
  } catch (error) { showError(error); }
}

function askLanguageServerApproval(approval: LanguageServerApprovalSummary): Promise<string> {
  const details = document.querySelector<HTMLDListElement>('#language-server-approval-details')!; details.replaceChildren();
  for (const [label, value] of [
    ['Extension', approval.extension_id], ['Configuration', approval.configuration_id],
    ['Executable', approval.command], ['Arguments', approval.arguments.join(' ') || '(none)'],
    ['Working directory', approval.working_directory], ['Environment names', approval.environment_keys.join(', ') || '(none)'],
  ]) {
    const term = document.createElement('dt'); term.textContent = label;
    const description = document.createElement('dd'); description.textContent = value; details.append(term, description);
  }
  const dialog = document.querySelector<HTMLDialogElement>('#language-server-approval-dialog')!; dialog.showModal();
  return new Promise((resolve) => dialog.addEventListener('close', () => resolve(dialog.returnValue), { once: true }));
}

async function stopLanguageServer(key: string, id: number): Promise<void> {
  try {
    await client.call({ method: 'stop_language_server', params: { server_id: id } });
    activeLanguageServers.delete(key);
    clearDiagnosticProviders(key);
    renderExtensions(expect(await client.call({ method: 'list_extensions' }), 'extensions').value);
    setStatus('Language server stopped');
  } catch (error) { showError(error); }
}

async function changeExtension(extension: ExtensionSummary, enabled: boolean): Promise<void> {
  try {
    await client.call({ method: 'set_extension_enabled', params: { extension_id: extension.id, enabled } });
    if (!enabled) { clearDiagnosticProviders(extension.id); refreshInspectionsForOpenDocuments(); }
    const catalog = expect(await client.call({ method: 'list_extensions' }), 'extensions').value;
    renderExtensions(catalog); setStatus(`${extension.name} ${enabled ? 'enabled' : 'disabled'}`);
  } catch (error) { showError(error); }
}

async function activateExtension(id: string): Promise<void> {
  try {
    await client.call({ method: 'activate_extension', params: { extension_id: id } });
    const catalog = expect(await client.call({ method: 'list_extensions' }), 'extensions').value;
    renderExtensions(catalog); setStatus(`Activated ${id}`);
  } catch (error) {
    showError(error);
    try { renderExtensions(expect(await client.call({ method: 'list_extensions' }), 'extensions').value); } catch { /* preserve activation error */ }
  }
}
let commandSelection = 0;
function renderCommandResults(query: string): void {
  const results = filterCommands(registeredCommands, query, commandLabels, commandDescriptions);
  const enabled = results.map((command) => commandMetadata.get(command)?.enabled !== false);
  if (commandSelection < 0 || commandSelection >= results.length || !enabled[commandSelection]) commandSelection = nextCommandSelection(-1, results.length, 1, enabled);
  const host = document.querySelector<HTMLDivElement>('#command-results')!; host.replaceChildren();
  for (const [index, command] of results.entries()) {
    const button = document.createElement('button'); button.setAttribute('role', 'option');
    button.setAttribute('aria-selected', String(index === commandSelection)); button.disabled = commandMetadata.get(command)?.enabled === false;
    const label = document.createElement('span'); label.textContent = commandLabels.has(command) ? `${commandLabels.get(command)} · ${command}` : command; button.append(label);
    const metadata = commandMetadata.get(command);
    if (metadata?.description) button.title = metadata.description;
    if (metadata?.shortcut) { const shortcut = document.createElement('kbd'); shortcut.textContent = metadata.shortcut; button.append(shortcut); }
    button.addEventListener('mouseenter', () => {
      if (!enabled[index]) return;
      commandSelection = index;
      for (const option of host.querySelectorAll<HTMLButtonElement>('[role="option"]')) {
        option.setAttribute('aria-selected', String(Number(option.dataset.commandIndex) === commandSelection));
      }
    });
    button.dataset.commandIndex = String(index);
    button.addEventListener('click', () => void executeCommand(command)); host.append(button);
  }
  if (!results.length) { const empty = document.createElement('div'); empty.className = 'command-empty'; empty.textContent = 'No matching commands.'; host.append(empty); }
}
async function executeCommand(command: string): Promise<void> {
  document.querySelector<HTMLDialogElement>('#command-dialog')!.close();
  try {
    const active = tabs.active;
    if (active) {
      await documentViews.get(active.id)?.pipeline.flush();
      await client.call({ method: 'read_document', params: { document_id: active.id } });
    }
    const result = expect(await client.call({ method: 'execute_command', params: { command_id: command } }), 'command_output').value;
    if (result === 'editor.new_file') { void createWorkspaceEntry(parentPath(selectedPath), false); return; }
    if (result === 'editor.search') { showPanel('search'); document.querySelector<HTMLInputElement>('#search-query')!.focus(); return; }
    if (result === 'editor.source_control') { showPanel('source-control'); void refreshGitRepositories(); return; }
    renderExtensions(expect(await client.call({ method: 'list_extensions' }), 'extensions').value);
    setStatus(`${command}: ${result}`);
  } catch (error) { showError(error); }
}

function handleAction(action: string, target: HTMLElement): void {
  switch (action) {
    case 'open': void chooseWorkspace(); break;
    case 'open-file': void chooseFile(); break;
    case 'new-file': void createWorkspaceEntry(parentPath(selectedPath), false); break;
    case 'source-control': showPanel('source-control'); void refreshGitRepositories(); break;
    case 'refresh-git': void refreshGitRepositories(); break;
    case 'cancel-search': cancelWorkspaceSearch(); break;
    case 'save': void saveActive(); break;
    case 'save-all': void saveAll(); break;
    case 'close-active': if (tabs.active) void closeTab(tabs.active); else setStatus('No open tab to close'); break;
    case 'undo': if (!editor || !undo(editor)) setStatus('Nothing to undo'); break;
    case 'redo': if (!editor || !redo(editor)) setStatus('Nothing to redo'); break;
    case 'select-all': if (editor) selectAll(editor); break;
    case 'toggle-theme': toggleTheme(); break;
    case 'toggle-explorer': setExplorerCollapsed(!layout.explorerCollapsed); break;
    case 'toggle-bottom': setBottomCollapsed(!layout.bottomCollapsed); break;
    case 'show-panel': {
      const panel = target.dataset.panelTarget ?? 'output'; showPanel(panel);
      if (panel === 'search') document.querySelector<HTMLInputElement>('#search-query')!.focus();
      break;
    }
    case 'run-selected': void runSelectedConfiguration(); break;
    case 'settings': {
      void openSettings(); break;
    }
    case 'close-wizard': document.querySelector<HTMLDialogElement>('#project-wizard-dialog')!.close(); break;
    case 'focus-explorer': setExplorerCollapsed(false); treeRoot.querySelector<HTMLButtonElement>('.tree-row')?.focus(); break;
    case 'focus-editor': editor?.focus(); break;
    case 'next-tab': cycleTab(1); break;
    case 'previous-tab': cycleTab(-1); break;
    case 'discover': void discoverCargo(); break;
    case 'run-cargo': void runCargoCommand(target.dataset.command ?? ''); break;
    case 'cancel-task': if (runId !== null) void client.call({ method: 'cancel_task', params: { run_id: runId } }).then(() => setStatus('Cancellation requested')).catch(showError); break;
    case 'refresh': if (workspaceRoot) void refreshDirectory(''); else setStatus('Open a workspace first'); break;
    case 'command-palette': void openCommandPalette(); break;
    case 'extensions': void openExtensions(); break;
    case 'import-sarif': void importSarif(); break;
    case 'export-sarif': void exportSarif(); break;
    case 'copy-path': void copyCurrentPath(); break;
    case 'shortcuts': showInfo('Keyboard Shortcuts', 'Ctrl+O  Open Folder\nCtrl+Shift+O  Open File\nCtrl+N  New File\nCtrl+Shift+F  Search Workspace\nCtrl+S  Save\nCtrl+W  Close Active Tab\nCtrl+Shift+P  Command Palette\nCtrl+B  Toggle Project Explorer\nCtrl+J  Toggle Bottom Panel\nAlt+T  Toggle Theme\nCtrl+Tab / Ctrl+Shift+Tab  Change Tab\nF6  Focus Editor\nF5  Cancel Active Task'); break;
    case 'about': showInfo('Koren Code Editor', 'A lightweight, universal code editor powered by Koren IDE Platform.\n\nVersion 0.1.0'); break;
  }
}

function cycleTab(direction: -1 | 1): void {
  if (tabs.tabs.length < 2 || tabs.activeId === null) return;
  const current = tabs.tabs.findIndex((tab) => tab.id === tabs.activeId);
  selectTab(tabs.tabs[(current + direction + tabs.tabs.length) % tabs.tabs.length]!.id);
}

async function copyCurrentPath(): Promise<void> {
  const tab = tabs.active;
  if (!tab || !workspaceRoot) { setStatus('No open file path to copy'); return; }
  try {
    await navigator.clipboard.writeText(`${workspaceRoot.replace(/[\\/]+$/, '')}/${tab.path}`);
    setStatus('File path copied');
  } catch { setStatus('Clipboard access is unavailable'); }
}

async function refreshGitRepositories(): Promise<void> {
  if (!workspaceRoot) { setStatus('Open a workspace first'); return; }
  try {
    gitRepositories = expect(await client.call({ method: 'discover_git_repositories' }), 'git_repositories').value;
    const select = document.querySelector<HTMLSelectElement>('#git-repository')!;
    const workspace = workspaceRoot.replaceAll('\\', '/').replace(/\/$/, '');
    const choices = gitRepositories.map((repo) => ({ repo, path: repo.root.replaceAll('\\', '/').replace(/\/$/, '').slice(workspace.length).replace(/^\//, '') }));
    select.replaceChildren(new Option('Select repository', ''));
    for (const { repo, path } of choices) select.add(new Option(`${path ? `${path}/` : ''}${repo.branch ?? '(detached)'}`, path));
    if (!choices.some(({ path }) => path === gitRepositoryPath)) gitRepositoryPath = choices[0]?.path ?? '';
    select.value = gitRepositoryPath;
    if (gitRepositoryPath) await refreshGitStatus();
    else { gitStatus = null; renderGitStatus(); }
  } catch (error) { showError(error); }
}

async function refreshGitStatus(): Promise<void> {
  if (!gitRepositoryPath) { renderGitStatus(); return; }
  try {
    gitStatus = expect(await client.call({ method: 'git_status', params: { path: gitRepositoryPath } }), 'git_status').value;
    renderGitStatus();
  } catch (error) { gitStatus = null; renderGitStatus(); showError(error); }
}

function renderGitStatus(): void {
  document.querySelector('#git-branch')!.textContent = gitStatus ? `${gitStatus.root} · ${gitStatus.branch ?? 'detached HEAD'}` : 'No local Git repository selected.';
  const renderFiles = (hostId: string, staged: boolean) => {
    const host = document.querySelector<HTMLDivElement>(`#${hostId}`)!; host.replaceChildren();
    const partition = partitionGitFiles(gitStatus?.files ?? []);
    const files = staged ? partition.staged : partition.unstaged;
    if (!files.length) { const empty = document.createElement('div'); empty.className = 'panel-empty'; empty.textContent = staged ? 'No staged changes.' : 'No unstaged changes.'; host.append(empty); return; }
    for (const file of files) {
      const row = document.createElement('div'); row.className = `git-file ${staged ? 'staged' : 'unstaged'}`;
      const path = document.createElement('button'); path.className = 'git-file-path'; path.textContent = `${file.deleted ? 'D' : file.untracked ? '?' : file.staged ? 'M' : 'M'}  ${file.path}`; path.title = file.path;
      path.addEventListener('click', () => void loadGitDiff(file.path, staged)); row.append(path);
      const action = document.createElement('button'); action.className = 'git-file-action'; action.textContent = staged ? 'Unstage' : 'Stage'; action.disabled = file.conflicted;
      action.addEventListener('click', () => void mutateGitPath(staged ? 'git_unstage' : 'git_stage', file.path)); row.append(action); host.append(row);
    }
  };
  renderFiles('git-unstaged', false); renderFiles('git-staged', true);
}

async function loadGitDiff(path: string, staged: boolean): Promise<void> {
  try {
    const diff = expect(await client.call({ method: 'git_diff', params: { repository: gitRepositoryPath, path, staged } }), 'git_diff').value;
    const host = document.querySelector<HTMLPreElement>('#git-diff')!; host.replaceChildren();
    const lines = diff.text.split('\n'); const visible = lines.slice(0, 5000);
    if (!diff.text) host.textContent = '(No text diff available.)';
    else visible.forEach((line, index) => {
      const row = document.createElement('span');
      row.className = line.startsWith('+') && !line.startsWith('+++') ? 'diff-add' : line.startsWith('-') && !line.startsWith('---') ? 'diff-delete' : line.startsWith('@@') ? 'diff-hunk' : line.startsWith('diff ') || line.startsWith('index ') ? 'diff-header' : '';
      row.textContent = line; host.append(row);
      if (index < visible.length - 1) host.append(document.createTextNode('\n'));
    });
    if (lines.length > 5000) { const note = document.createElement('span'); note.className = 'diff-limit'; note.textContent = `\n… ${lines.length - 5000} further lines hidden`; host.append(note); }
    host.dataset.path = path; host.dataset.staged = String(staged);
    host.classList.toggle('has-diff', diff.text.length > 0);
  } catch (error) { showError(error); }
}

async function mutateGitPath(method: 'git_stage' | 'git_unstage', path: string): Promise<void> {
  try {
    expect(await client.call({ method, params: { repository: gitRepositoryPath, path } }), 'git_operation');
    await refreshGitStatus();
    const diff = document.querySelector<HTMLPreElement>('#git-diff')!;
    if (diff.dataset.path === path) await loadGitDiff(path, diff.dataset.staged === 'true');
  } catch (error) { showError(error); }
}

async function createGitCommit(message: string): Promise<void> {
  if (!gitRepositoryPath) { setStatus('Select a Git repository first'); return; }
  try {
    const result = expect(await client.call({ method: 'git_commit', params: { repository: gitRepositoryPath, message } }), 'git_operation').value;
    setStatus(`Commit created ${result.commit?.slice(0, 8) ?? ''}`.trim()); await refreshGitStatus();
  } catch (error) { showError(error); }
}

async function startWorkspaceSearch(): Promise<void> {
  if (!workspaceRoot) { setStatus('Open a workspace first'); return; }
  if (activeSearchJob !== null) {
    await cancelWorkspaceSearch();
    if (activeSearchJob !== null) { setStatus('Waiting for the previous search to stop'); return; }
  }
  const query = document.querySelector<HTMLInputElement>('#search-query')!.value;
  const filter = document.querySelector<HTMLInputElement>('#search-filter')!.value.trim();
  const results = document.querySelector<HTMLDivElement>('#search-results')!; results.replaceChildren(); searchMatches = [];
  document.querySelector('#search-status')!.textContent = 'Searching…';
  const cancel = document.querySelector<HTMLButtonElement>('[data-action="cancel-search"]')!; cancel.disabled = false;
  try {
    const started = expect(await client.call({ method: 'start_workspace_search', params: {
      query, case_sensitive: document.querySelector<HTMLInputElement>('#search-case')!.checked,
      regular_expression: document.querySelector<HTMLInputElement>('#search-regex')!.checked,
      file_filter: filter || null,
    } }), 'search_started').value;
    activeSearchJob = started.job_id; void pollWorkspaceSearch(started.job_id);
  } catch (error) { activeSearchJob = null; cancel.disabled = true; showError(error); }
}

async function pollWorkspaceSearch(jobId: number): Promise<void> {
  if (activeSearchJob !== jobId || searchPollingJob === jobId) return;
  searchPollingJob = jobId;
  try {
    const batch = expect(await client.call({ method: 'poll_workspace_search', params: { job_id: jobId } }), 'search_batch').value;
    searchMatches.push(...batch.matches); renderSearchResults();
    if (batch.error) setStatus(batch.error);
    if (batch.complete) {
      activeSearchJob = null; document.querySelector<HTMLButtonElement>('[data-action="cancel-search"]')!.disabled = true;
      const summary = batch.summary;
      document.querySelector('#search-status')!.textContent = summary
        ? searchSummaryText(summary)
        : 'Search complete.';
      return;
    }
    const progress = batch.progress;
    if (progress) document.querySelector('#search-status')!.textContent = `${progress.matches_found} matches · ${progress.files_scanned} files${progress.current_path ? ` · ${progress.current_path}` : ''}`;
    searchPollTimer = window.setTimeout(() => void pollWorkspaceSearch(jobId), 75);
  } catch (error) { activeSearchJob = null; document.querySelector<HTMLButtonElement>('[data-action="cancel-search"]')!.disabled = true; showError(error); }
  finally { searchPollingJob = null; }
}

async function cancelWorkspaceSearch(): Promise<void> {
  if (activeSearchJob === null) return;
  const jobId = activeSearchJob; window.clearTimeout(searchPollTimer);
  try { await client.call({ method: 'cancel_workspace_search', params: { job_id: jobId } }); setStatus('Search cancellation requested'); }
  catch (error) { showError(error); }
  for (let attempt = 0; attempt < 100 && activeSearchJob === jobId; attempt++) {
    while (searchPollingJob === jobId) await new Promise((resolve) => window.setTimeout(resolve, 10));
    if (activeSearchJob !== jobId) break;
    try {
      const batch = expect(await client.call({ method: 'poll_workspace_search', params: { job_id: jobId } }), 'search_batch').value;
      searchMatches.push(...batch.matches); renderSearchResults();
      if (batch.complete) {
        activeSearchJob = null; document.querySelector<HTMLButtonElement>('[data-action="cancel-search"]')!.disabled = true;
        if (batch.summary) document.querySelector('#search-status')!.textContent = searchSummaryText(batch.summary);
        break;
      }
    } catch (error) { activeSearchJob = null; showError(error); break; }
    await new Promise((resolve) => window.setTimeout(resolve, 40));
  }
  if (activeSearchJob === jobId) searchPollTimer = window.setTimeout(() => void pollWorkspaceSearch(jobId), 50);
}

function renderSearchResults(): void {
  const host = document.querySelector<HTMLDivElement>('#search-results')!; host.replaceChildren();
  const visible = searchMatches.slice(0, 1000);
  const groups = groupSearchMatches(visible);
  for (const [path, matches] of groups) {
    const heading = document.createElement('div'); heading.className = 'problem-group'; heading.textContent = `${path} · ${matches.length}`; host.append(heading);
    for (const match of matches) {
      const button = document.createElement('button'); button.className = 'search-result'; button.title = `${path}:${match.line}`;
      const location = document.createElement('span'); location.className = 'search-result-path'; location.textContent = `:${match.line}:${match.column_utf16 + 1}`;
      const preview = document.createElement('span'); preview.className = 'search-result-preview'; preview.textContent = match.preview;
      button.append(location, preview); button.addEventListener('click', () => void navigateToSearchMatch(match)); host.append(button);
    }
  }
  if (searchMatches.length > visible.length) { const note = document.createElement('div'); note.className = 'panel-empty'; note.textContent = `Showing ${visible.length} of ${searchMatches.length} matches.`; host.append(note); }
}

async function navigateToSearchMatch(match: SearchMatch): Promise<void> {
  await openDocument(match.path);
  if (!editor) return;
  const line = editor.state.doc.line(Math.max(1, Math.min(editor.state.doc.lines, match.line)));
  const position = Math.max(line.from, Math.min(line.to, line.from + match.column_utf16));
  editor.dispatch({ selection: { anchor: position, head: Math.min(line.to, position + match.match_length_utf16) }, scrollIntoView: true }); editor.focus();
}

async function navigateToDefinition(uri: string, range: TextRange): Promise<void> {
  if (!workspaceRoot) return;
  try {
    const target = new URL(uri);
    if (target.protocol !== 'file:') return;
    let filePath = decodeURIComponent(target.pathname);
    if (/^\/[A-Za-z]:\//.test(filePath)) filePath = filePath.slice(1);
    const root = workspaceRoot.replaceAll('\\', '/').replace(/\/$/, '');
    const candidate = filePath.replaceAll('\\', '/');
    if (!candidate.startsWith(`${root}/`)) { setStatus('Definition is outside the open workspace'); return; }
    const relative = candidate.slice(root.length + 1);
    await openDocument(relative);
    const active = tabs.active;
    if (!active || !editor) return;
    const line = editor.state.doc.line(Math.max(1, Math.min(editor.state.doc.lines, range.start.line + 1)));
    const position = Math.max(line.from, Math.min(line.to, line.from + range.start.character));
    editor.dispatch({ selection: { anchor: position }, scrollIntoView: true }); editor.focus();
  } catch (error) { showError(error); }
}

document.addEventListener('contextmenu', (event) => {
  const target = event.target as HTMLElement;
  if (target.closest('#tree')) {
    const row = target.closest<HTMLButtonElement>('.tree-row');
    const path = row?.title ?? '';
    showWorkspaceContextMenu(event, path, !row || row.classList.contains('directory'));
    const items = projectExplorerMenus;
    if (items.length) {
      const menu = document.querySelector<HTMLDivElement>('#editor-context-menu')!;
      for (const item of items) { const button = document.createElement('button'); button.setAttribute('role', 'menuitem'); button.textContent = item.label; button.disabled = !item.enabled; button.addEventListener('click', () => { menu.remove(); void executeCommand(item.command_id); }); menu.append(button); }
    }
    return;
  }
  const items = target.closest('#editor') ? editorContextMenus : [];
  if (!items.length) return;
  event.preventDefault();
  document.querySelector('#editor-context-menu')?.remove();
  const menu = document.createElement('div'); menu.id = 'editor-context-menu'; menu.className = 'editor-context-menu'; menu.setAttribute('role', 'menu');
  for (const item of items) {
    const button = document.createElement('button'); button.setAttribute('role', 'menuitem'); button.textContent = item.label;
    button.disabled = !item.enabled;
    button.addEventListener('click', () => { menu.remove(); void executeCommand(item.command_id); }); menu.append(button);
  }
  menu.style.left = `${event.clientX}px`; menu.style.top = `${event.clientY}px`; document.body.append(menu);
});

document.addEventListener('click', (event) => {
  if (!(event.target as HTMLElement).closest('#editor-context-menu')) document.querySelector('#editor-context-menu')?.remove();
  const target = (event.target as HTMLElement).closest<HTMLElement>('[data-action]');
  if (target) handleAction(target.dataset.action ?? '', target);
  if (!(event.target as HTMLElement).closest('.menu-group')) {
    for (const menu of document.querySelectorAll<HTMLDetailsElement>('.menu-group[open]')) menu.open = false;
  } else if (target?.matches('[role="menuitem"]')) {
    for (const menu of document.querySelectorAll<HTMLDetailsElement>('.menu-group[open]')) menu.open = false;
  }
});

document.addEventListener('keydown', (event) => {
  const commandDialog = document.querySelector<HTMLDialogElement>('#command-dialog')!;
  if (commandDialog.open) {
    const input = document.querySelector<HTMLInputElement>('#command-search')!;
    const results = filterCommands(registeredCommands, input.value, commandLabels, commandDescriptions);
    const enabled = results.map((command) => commandMetadata.get(command)?.enabled !== false);
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault(); commandSelection = nextCommandSelection(commandSelection, results.length, event.key === 'ArrowDown' ? 1 : -1, enabled); renderCommandResults(input.value);
    } else if (event.key === 'Enter' && results[commandSelection] && enabled[commandSelection]) {
      event.preventDefault(); void executeCommand(results[commandSelection]!);
    }
    return;
  }
  if (!event.isComposing) {
    const shortcut = [...commandMetadata.entries()].find(([_, metadata]) => metadata.enabled && metadata.shortcut && commandShortcutMatches(event, metadata.shortcut));
    if (shortcut) { event.preventDefault(); void executeCommand(shortcut[0]); return; }
  }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'o') { event.preventDefault(); event.shiftKey ? void chooseFile() : void chooseWorkspace(); }
  else if ((event.ctrlKey || event.metaKey) && !event.shiftKey && event.key.toLowerCase() === 'n') { event.preventDefault(); void createWorkspaceEntry(parentPath(selectedPath), false); }
  else if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === 'f') { event.preventDefault(); showPanel('search'); document.querySelector<HTMLInputElement>('#search-query')!.focus(); }
  else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') { event.preventDefault(); void saveActive(); }
  else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'w' && tabs.active) { event.preventDefault(); void closeTab(tabs.active); }
  else if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === 'p') { event.preventDefault(); void openCommandPalette(); }
  else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'b') { event.preventDefault(); setExplorerCollapsed(!layout.explorerCollapsed); }
  else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'j') { event.preventDefault(); setBottomCollapsed(!layout.bottomCollapsed); }
  else if ((event.ctrlKey || event.metaKey) && event.key === '`') { event.preventDefault(); if (layout.bottomCollapsed || !document.querySelector<HTMLButtonElement>('[data-panel="terminal"]')!.classList.contains('selected')) showPanel('terminal'); else setBottomCollapsed(true); }
  else if ((event.ctrlKey || event.metaKey) && event.key === 'Tab') { event.preventDefault(); cycleTab(event.shiftKey ? -1 : 1); }
  else if (event.altKey && event.key.toLowerCase() === 't') { event.preventDefault(); toggleTheme(); }
  else if (event.key === 'F6') { event.preventDefault(); editor?.focus(); }
  else if (event.key === 'F5' && runId !== null) { event.preventDefault(); void client.call({ method: 'cancel_task', params: { run_id: runId } }).then(() => setStatus('Cancellation requested')).catch(showError); }
  else if (event.key === 'Escape') for (const menu of document.querySelectorAll<HTMLDetailsElement>('.menu-group[open]')) { menu.open = false; menu.querySelector('summary')?.focus(); }
});

document.querySelector<HTMLInputElement>('#command-search')!.addEventListener('input', (event) => {
  commandSelection = 0; renderCommandResults((event.target as HTMLInputElement).value);
});

document.querySelectorAll<HTMLButtonElement>('[data-panel]').forEach((button) => button.addEventListener('click', () => showPanel(button.dataset.panel ?? 'output')));
document.querySelector<HTMLSelectElement>('#git-repository')!.addEventListener('change', (event) => {
  gitRepositoryPath = (event.currentTarget as HTMLSelectElement).value; void refreshGitStatus();
});
document.querySelector<HTMLButtonElement>('[data-action="refresh-git"]')!.addEventListener('click', () => void refreshGitRepositories());
document.querySelector<HTMLFormElement>('#git-commit-form')!.addEventListener('submit', (event) => {
  event.preventDefault(); const input = document.querySelector<HTMLInputElement>('#git-commit-message')!;
  const message = normalizedCommitMessage(input.value);
  if (!message) { setStatus('Enter a commit message'); input.focus(); return; }
  void createGitCommit(message).then(() => { input.value = ''; });
});
document.querySelector<HTMLFormElement>('#search-form')!.addEventListener('submit', (event) => { event.preventDefault(); void startWorkspaceSearch(); });
document.querySelector<HTMLButtonElement>('[data-action="start-project-analysis"]')!.addEventListener('click', () => void startProjectInspection());
document.querySelector<HTMLButtonElement>('[data-action="cancel-project-analysis"]')!.addEventListener('click', () => void cancelProjectInspection());
document.querySelector<HTMLButtonElement>('[data-action="load-project-findings"]')!.addEventListener('click', () => {
  if (latestProjectInspectionId !== null && projectInspectionOffset !== null) void loadProjectInspectionFindings(latestProjectInspectionId, projectInspectionOffset);
});
document.querySelector<HTMLSelectElement>('#problem-severity-filter')!.addEventListener('change', (event) => {
  problemSeverityFilter = (event.currentTarget as HTMLSelectElement).value as InspectionSeverityFilter; renderProblems();
});
document.querySelector<HTMLSelectElement>('#problem-grouping')!.addEventListener('change', (event) => {
  problemGrouping = (event.currentTarget as HTMLSelectElement).value as InspectionGrouping; renderProblems();
});
document.querySelector<HTMLSelectElement>('#theme-setting')!.addEventListener('change', (event) => {
  const value = (event.currentTarget as HTMLSelectElement).value;
  if (value.startsWith('extension:')) {
    const theme = extensionCatalogCache?.themes.find((item) => item.id === value.slice('extension:'.length));
    if (theme) applyExtensionTheme(theme);
  } else setTheme(value === 'light' ? 'light' : 'dark');
});
runConfiguration.addEventListener('change', syncRunButton);

document.querySelector<HTMLElement>('#explorer-resizer')!.addEventListener('pointerdown', (event) => {
  const move = (moveEvent: PointerEvent) => {
    layout.explorerWidth = Math.max(148, Math.min(moveEvent.clientX, window.innerWidth * 0.42));
    workbench.style.setProperty('--explorer-width', `${layout.explorerWidth}px`);
  };
  const stop = () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', stop); saveLayout(); };
  window.addEventListener('pointermove', move); window.addEventListener('pointerup', stop, { once: true }); event.preventDefault();
});
document.querySelector<HTMLElement>('#explorer-resizer')!.addEventListener('keydown', (event) => {
  if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
  layout.explorerWidth = Math.max(148, layout.explorerWidth + (event.key === 'ArrowRight' ? 12 : -12));
  workbench.style.setProperty('--explorer-width', `${layout.explorerWidth}px`); saveLayout(); event.preventDefault();
});
document.querySelector<HTMLElement>('#bottom-resizer')!.addEventListener('pointerdown', (event) => {
  setBottomCollapsed(false);
  const move = (moveEvent: PointerEvent) => {
    layout.bottomHeight = Math.max(96, Math.min(window.innerHeight - moveEvent.clientY - 24, window.innerHeight * 0.5));
    document.querySelector<HTMLElement>('#bottom-panel')!.style.height = `${layout.bottomHeight}px`;
  };
  const stop = () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', stop); saveLayout(); };
  window.addEventListener('pointermove', move); window.addEventListener('pointerup', stop, { once: true }); event.preventDefault();
});
document.querySelector<HTMLElement>('#bottom-resizer')!.addEventListener('keydown', (event) => {
  if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return;
  layout.bottomHeight = Math.max(96, Math.min(480, layout.bottomHeight + (event.key === 'ArrowUp' ? 12 : -12)));
  document.querySelector<HTMLElement>('#bottom-panel')!.style.height = `${layout.bottomHeight}px`; saveLayout(); event.preventDefault();
});

window.addEventListener('koren-close-request', () => {
  void (async () => {
    const dialog = document.querySelector<HTMLDialogElement>('#close-dialog')!;
    const dirty = tabs.tabs.some((tab) => tab.dirty);
    if (!dirty) { window.ipc?.postMessage(JSON.stringify({ type: 'close_decision', allow: true })); return; }
    dialog.showModal();
    const action = await new Promise<string>((resolve) => dialog.addEventListener('close', () => resolve(dialog.returnValue), { once: true }));
    if (action === 'save') window.ipc?.postMessage(JSON.stringify({ type: 'close_decision', allow: await saveAll() }));
    else window.ipc?.postMessage(JSON.stringify({ type: 'close_decision', allow: action === 'discard' }));
  })();
});

document.documentElement.dataset.theme = readTheme(localStorage);
document.querySelector<HTMLSelectElement>('#theme-setting')!.value = document.documentElement.dataset.theme === 'light' ? 'light' : 'dark';
applyLayout();
if (layout.bottomCollapsed) document.querySelector('#bottom-resizer')!.classList.add('hidden');
renderTree(); renderTabs(); updateStatusDocument();
void (async () => { for await (const events of client.subscribeEvents()) events.forEach(handlePlatformEvent); })();

export { client, renderTree, renderTabs, showPanel };
