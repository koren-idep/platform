import { redo, redoDepth, selectAll, undo, undoDepth } from '@codemirror/commands';
import { EditorState } from '@codemirror/state';
import { basicSetup, EditorView } from 'codemirror';
import { linter, setDiagnostics, type Diagnostic as CodeMirrorDiagnostic } from '@codemirror/lint';
import { KorenClient, KorenError, createDesktopTransport } from './services/client';
import type { CargoProjectSummary, ExecutionApprovalSummary, ExtensionCatalog, ExtensionDiagnosticDetailSummary, ExtensionMenuSummary, ExtensionProjectWizardSummary, ExtensionSettingSummary, ExtensionSettingValue, ExtensionSummary, ExtensionThemeSummary, ExtensionToolWindowSummary, ExtensionUiDocument, LanguageDiagnosticSet, LanguageServerApprovalSummary, LanguageServerSummary, PlatformEvent, ResponseValue, TaskSummary, TextRange } from './services/protocol';
import { approvalChoice, conflictChoice } from './app/dialogs';
import { commandShortcutMatches, filterCommands, nextCommandSelection } from './app/commands';
import { activePanelLayout, diagnosticDetailMatches, extensionMenusAt, safeThemeVariables, validExtensionSettingValue } from './app/extensions';
import { readLayout, persistLayout, type LayoutPreferences } from './app/layout';
import { formatCursorPosition, lineEndingFor } from './app/status';
import { runConfigurationTasks, selectedRunTask } from './app/toolbar';
import { nextTheme, persistTheme, readTheme } from './app/theme';
import { taskStatus, visibleOutput } from './app/taskPresentation';
import { extensionCanActivate, extensionToggleLabel, languageServerEnabled } from './app/extensions';
import { diagnosticsForRevision, withDiagnosticPublication } from './app/diagnostics';
import { ProjectTreeState, TabState, type EditorTab } from './app/state';
import { DocumentEditPipeline } from './editor/sync';
import { editsFromViewUpdate } from './editor/transactions';
import { languageForPath, languageNameForPath } from './editor/languages';
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
        <button role="menuitem" disabled title="New file creation is not available yet">New File</button>
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
      <button class="toolbar-button" disabled title="New file creation is not available yet" aria-label="New File">＋</button>
      <button class="toolbar-button" data-action="open" title="Open Folder (Ctrl+O)" aria-label="Open Folder">▱</button>
      <button class="toolbar-button" data-action="save" id="toolbar-save" disabled title="Save (Ctrl+S)" aria-label="Save">▣</button>
    </div>
    <span class="toolbar-divider" aria-hidden="true"></span>
    <div class="toolbar-group">
      <button class="toolbar-button" data-action="undo" id="toolbar-undo" disabled title="Undo (Ctrl+Z)" aria-label="Undo">↶</button>
      <button class="toolbar-button" data-action="redo" id="toolbar-redo" disabled title="Redo (Ctrl+Y)" aria-label="Redo">↷</button>
    </div>
    <span class="toolbar-divider" aria-hidden="true"></span>
    <button class="toolbar-button" disabled title="Source control integration is not available" aria-label="Source Control">⑂</button>
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
          <button class="selected" data-panel="output" role="tab" aria-selected="true">Output</button><button data-panel="problems" role="tab" aria-selected="false">Problems <span id="problem-count" class="count-badge">0</span></button><button data-panel="tasks" role="tab" aria-selected="false">Tasks</button><button data-panel="projects" role="tab" aria-selected="false">Projects</button>
        </div><span id="bottom-task-state" class="task-state"></span><span class="panel-actions"><button data-action="cancel-task" title="Cancel active task" aria-label="Cancel active task" disabled>■</button><button data-action="toggle-bottom" title="Collapse bottom panel" aria-label="Collapse bottom panel">⌄</button></span></div>
        <div id="output" class="panel-content output"><div class="panel-empty">Task output will appear here.</div></div>
        <div id="problems" class="panel-content problem-list hidden"><div class="panel-empty">No problems reported by installed language integrations.</div></div>
        <div id="tasks" class="panel-content task-list hidden"><div class="panel-empty">Discover Cargo projects to see available tasks.</div></div>
        <div id="projects" class="panel-content project-list hidden"><div class="panel-empty">Project information will appear here.</div></div>
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
  <dialog id="settings-dialog" class="dialog settings-dialog"><form method="dialog"><header>Settings</header><label for="theme-setting">Color theme</label><select id="theme-setting"><option value="dark">OLED Black</option><option value="light">Light</option></select><div id="extension-settings" class="extension-settings"></div><div class="dialog-actions"><button value="close" class="primary">Close</button></div></form></dialog>
  <dialog id="info-dialog" class="dialog"><form method="dialog"><header id="info-title"></header><div id="info-content" class="info-content"></div><div class="dialog-actions"><button value="close" class="primary">Close</button></div></form></dialog>
  <dialog id="project-wizard-dialog" class="dialog"><form id="project-wizard-form"><header id="project-wizard-title"></header><div id="project-wizard-fields" class="extension-panel-view"></div><p class="subtle">Submitting validates these fields only. Project creation must be performed by a host-authorized extension operation.</p><div class="dialog-actions"><button type="button" class="secondary" data-action="close-wizard">Cancel</button><button type="submit" class="primary">Validate form</button></div></form></dialog>
  <div id="status-toast" role="status" aria-live="polite"></div>`;

const treeState = new ProjectTreeState();
const tabs = new TabState();
const layout: LayoutPreferences = readLayout(localStorage);
const treeRoot = document.querySelector<HTMLDivElement>('#tree')!;
const editorHost = document.querySelector<HTMLDivElement>('#editor')!;
const welcome = document.querySelector<HTMLElement>('#welcome')!;
const tabsHost = document.querySelector<HTMLDivElement>('#tabs')!;
const outputHost = document.querySelector<HTMLDivElement>('#output')!;
const taskHost = document.querySelector<HTMLDivElement>('#tasks')!;
const projectHost = document.querySelector<HTMLDivElement>('#projects')!;
const runConfiguration = document.querySelector<HTMLSelectElement>('#run-configuration')!;
const toast = document.querySelector<HTMLDivElement>('#status-toast')!;
const workbench = document.querySelector<HTMLElement>('#workbench')!;
const rootLabel = document.querySelector<HTMLElement>('#status-workspace')!;
let workspaceRoot = '';
let selectedPath = '';
let runId: number | null = null;
const documentViews = new Map<number, { state: EditorState; pipeline: DocumentEditPipeline; lineEnding: 'LF' | 'CRLF' }>();
const taskById = new Map<number, TaskSummary>();
const activeLanguageServers = new Map<string, number>();
const diagnosticsByDocument = new Map<number, Map<string, LanguageDiagnosticSet>>();
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
    const { state, pipeline, lineEnding } = await createDocumentState(tab, content.contents, content.revision);
    documentViews.set(tab.id, { state, pipeline, lineEnding });
    if (!editor) editor = new EditorView({ state, parent: editorHost }); else editor.setState(state);
    welcome.classList.add('hidden'); editorHost.classList.remove('hidden');
    renderTree(); renderTabs(); updateStatusDocument(); editor.focus();
  } catch (error) { showError(error); }
}

async function createDocumentState(tab: EditorTab, contents: string, revision: number): Promise<{ state: EditorState; pipeline: DocumentEditPipeline; lineEnding: 'LF' | 'CRLF' }> {
  const lineEnding = lineEndingFor(contents);
  const lineSeparator = lineEnding === 'CRLF' ? '\r\n' : '\n';
  const languageExtensions = await languageForPath(tab.path);
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
    extensions: [basicSetup, linter(() => []), theme, EditorState.tabSize.of(4), EditorState.lineSeparator.of(lineSeparator), EditorView.lineWrapping, ...languageExtensions, ...serviceExtensions,
      EditorView.updateListener.of((update) => {
        const entry = documentViews.get(tab.id);
        if (!entry) return;
        entry.state = update.state;
        if (update.docChanged) {
          entry.pipeline.enqueue(editsFromViewUpdate(update));
          diagnosticsByDocument.delete(tab.id);
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
  return { state, pipeline, lineEnding };
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
    tabs.close(tab.id); documentViews.delete(tab.id); diagnosticsByDocument.delete(tab.id); renderProblems();
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
      const { state, pipeline, lineEnding } = await createDocumentState(tab, content.contents, content.revision);
      tabs.update(tab.id, { lineEnding });
      documentViews.set(tab.id, { state, pipeline, lineEnding });
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
  try { const catalog = expect(await client.call({ method: 'list_extensions' }), 'extensions').value; renderExtensions(catalog); document.querySelector<HTMLDialogElement>('#settings-dialog')!.showModal(); }
  catch (error) { showError(error); }
}
function toggleTheme(): void { setTheme(nextTheme(document.documentElement.dataset.theme === 'light' ? 'light' : 'dark')); }

function showPanel(panel: 'output' | 'problems' | 'tasks' | 'projects' | string): void {
  for (const button of document.querySelectorAll<HTMLButtonElement>('[data-panel]')) {
    const selected = button.dataset.panel === panel;
    button.classList.toggle('selected', selected); button.setAttribute('aria-selected', String(selected));
  }
  for (const id of ['output', 'problems', 'tasks', 'projects']) document.querySelector(`#${id}`)!.classList.toggle('hidden', id !== panel);
  for (const host of document.querySelectorAll<HTMLElement>('[data-extension-panel]')) host.classList.toggle('hidden', host.dataset.extensionPanel !== panel);
  if (panel.startsWith('org.') || panel.includes('.')) { activeDynamicPanel = panel; localStorage.setItem('koren.activeExtensionPanel', panel); }
  setBottomCollapsed(false);
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
  const host = document.querySelector<HTMLDivElement>('#problems')!; host.replaceChildren();
  const rows = [...diagnosticsByDocument.entries()].flatMap(([documentId, providers]) => {
    const entry = documentViews.get(documentId);
    if (!entry) return [];
    return diagnosticsForRevision(providers, entry.pipeline.currentRevision)
      .map(({ diagnostic, provider }) => ({ documentId, diagnostic, provider }));
  });
  document.querySelector('#problem-count')!.textContent = String(rows.length);
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
    host.append(rowHost);
  }
  if (!rows.length) host.innerHTML = '<div class="panel-empty">No problems reported by installed language integrations.</div>';
}

function clearDiagnosticProviders(prefix: string): void {
  for (const [documentId, providers] of diagnosticsByDocument) {
    for (const provider of providers.keys()) if (provider === prefix || provider.startsWith(`${prefix}.`)) providers.delete(provider);
    if (!providers.size) diagnosticsByDocument.delete(documentId);
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
    if (!enabled) clearDiagnosticProviders(extension.id);
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
    renderExtensions(expect(await client.call({ method: 'list_extensions' }), 'extensions').value);
    setStatus(`${command}: ${result}`);
  } catch (error) { showError(error); }
}

function handleAction(action: string, target: HTMLElement): void {
  switch (action) {
    case 'open': void chooseWorkspace(); break;
    case 'open-file': void chooseFile(); break;
    case 'save': void saveActive(); break;
    case 'save-all': void saveAll(); break;
    case 'close-active': if (tabs.active) void closeTab(tabs.active); else setStatus('No open tab to close'); break;
    case 'undo': if (!editor || !undo(editor)) setStatus('Nothing to undo'); break;
    case 'redo': if (!editor || !redo(editor)) setStatus('Nothing to redo'); break;
    case 'select-all': if (editor) selectAll(editor); break;
    case 'toggle-theme': toggleTheme(); break;
    case 'toggle-explorer': setExplorerCollapsed(!layout.explorerCollapsed); break;
    case 'toggle-bottom': setBottomCollapsed(!layout.bottomCollapsed); break;
    case 'show-panel': showPanel(target.dataset.panelTarget as 'output' | 'problems' | 'tasks' | 'projects'); break;
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
    case 'copy-path': void copyCurrentPath(); break;
    case 'shortcuts': showInfo('Keyboard Shortcuts', 'Ctrl+O  Open Folder\nCtrl+Shift+O  Open File\nCtrl+S  Save\nCtrl+W  Close Active Tab\nCtrl+Shift+P  Command Palette\nCtrl+B  Toggle Project Explorer\nCtrl+J  Toggle Bottom Panel\nAlt+T  Toggle Theme\nCtrl+Tab / Ctrl+Shift+Tab  Change Tab\nF6  Focus Editor\nF5  Cancel Active Task'); break;
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
  const items = target.closest('#editor') ? editorContextMenus : target.closest('#tree') ? projectExplorerMenus : [];
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
  else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') { event.preventDefault(); void saveActive(); }
  else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'w' && tabs.active) { event.preventDefault(); void closeTab(tabs.active); }
  else if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === 'p') { event.preventDefault(); void openCommandPalette(); }
  else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'b') { event.preventDefault(); setExplorerCollapsed(!layout.explorerCollapsed); }
  else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'j') { event.preventDefault(); setBottomCollapsed(!layout.bottomCollapsed); }
  else if ((event.ctrlKey || event.metaKey) && event.key === 'Tab') { event.preventDefault(); cycleTab(event.shiftKey ? -1 : 1); }
  else if (event.altKey && event.key.toLowerCase() === 't') { event.preventDefault(); toggleTheme(); }
  else if (event.key === 'F6') { event.preventDefault(); editor?.focus(); }
  else if (event.key === 'F5' && runId !== null) { event.preventDefault(); void client.call({ method: 'cancel_task', params: { run_id: runId } }).then(() => setStatus('Cancellation requested')).catch(showError); }
  else if (event.key === 'Escape') for (const menu of document.querySelectorAll<HTMLDetailsElement>('.menu-group[open]')) { menu.open = false; menu.querySelector('summary')?.focus(); }
});

document.querySelector<HTMLInputElement>('#command-search')!.addEventListener('input', (event) => {
  commandSelection = 0; renderCommandResults((event.target as HTMLInputElement).value);
});

document.querySelectorAll<HTMLButtonElement>('[data-panel]').forEach((button) => button.addEventListener('click', () => showPanel(button.dataset.panel as 'output' | 'problems' | 'tasks' | 'projects')));
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
