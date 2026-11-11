import { redo, selectAll, undo } from '@codemirror/commands';
import { EditorState } from '@codemirror/state';
import { basicSetup, EditorView } from 'codemirror';
import { KorenClient, KorenError, createDesktopTransport } from './services/client';
import type { CargoProjectSummary, ExecutionApprovalSummary, PlatformEvent, ResponseValue, TaskSummary } from './services/protocol';
import { approvalChoice, conflictChoice } from './app/dialogs';
import { filterCommands, nextCommandSelection } from './app/commands';
import { readLayout, persistLayout, type LayoutPreferences } from './app/layout';
import { formatCursorPosition, lineEndingFor } from './app/status';
import { nextTheme, persistTheme, readTheme } from './app/theme';
import { taskStatus, visibleOutput } from './app/taskPresentation';
import { ProjectTreeState, TabState, type EditorTab } from './app/state';
import { DocumentEditPipeline } from './editor/sync';
import { editsFromViewUpdate } from './editor/transactions';
import { languageForPath, languageNameForPath } from './editor/languages';
import { flushAndSave } from './editor/save';
import './styles/main.css';

const client = new KorenClient(createDesktopTransport());
const app = document.querySelector<HTMLDivElement>('#app')!;
app.innerHTML = `
  <header class="menu-bar">
    <div class="brand" aria-label="Koren Code Editor"><span class="brand-mark">&lt;/&gt;</span><span class="brand-name">Koren Code Editor</span></div>
    <nav class="menus" aria-label="Main menu">
      <details class="menu-group"><summary>File</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="open">Open Folder… <kbd>Ctrl+O</kbd></button>
        <button role="menuitem" data-action="open-file" id="menu-open-file" disabled>Open File… <kbd>Ctrl+Shift+O</kbd></button>
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
        <button role="menuitem" data-action="show-panel" data-panel-target="projects">Projects</button>
      </div></details>
      <details class="menu-group"><summary>Navigate</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="focus-explorer">Focus Project Explorer <kbd>Ctrl+B</kbd></button>
        <button role="menuitem" data-action="focus-editor">Focus Editor <kbd>F6</kbd></button>
        <div class="menu-separator"></div><button role="menuitem" data-action="previous-tab">Previous Tab <kbd>Ctrl+Shift+Tab</kbd></button>
        <button role="menuitem" data-action="next-tab">Next Tab <kbd>Ctrl+Tab</kbd></button>
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
        <button role="menuitem" data-action="copy-path">Copy Current File Path</button>
        <button role="menuitem" data-action="refresh">Refresh Project Explorer</button>
      </div></details>
      <details class="menu-group"><summary>Help</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="shortcuts">Keyboard Shortcuts</button>
        <button role="menuitem" data-action="about">About Koren Code Editor</button>
      </div></details>
    </nav>
    <span class="menu-spacer"></span><span id="workspace-label" class="workspace-label">No Workspace</span>
    <button class="icon-button" data-action="toggle-theme" aria-label="Toggle theme" title="Toggle theme (Alt+T)">◐</button>
  </header>
  <main class="workbench" id="workbench">
    <aside class="explorer" id="explorer">
      <div class="panel-heading"><span>PROJECT</span><span class="panel-actions"><button data-action="refresh" title="Refresh explorer" aria-label="Refresh explorer">↻</button><button data-action="toggle-explorer" title="Collapse explorer" aria-label="Collapse explorer">‹</button></span></div>
      <div id="tree" class="tree" role="tree" aria-label="Project files"></div>
      <div class="explorer-footer"><button data-action="discover">Discover Cargo</button></div>
    </aside>
    <div id="explorer-resizer" class="explorer-resizer" role="separator" aria-label="Resize project explorer" aria-orientation="vertical" tabindex="0"></div>
    <section class="main-column">
      <div id="tabs" class="tabs" role="tablist" aria-label="Open files"></div>
      <div id="editor" class="editor hidden" aria-label="Code editor"></div>
      <section id="welcome" class="welcome">
        <div class="welcome-mark">&lt;/&gt;</div><div class="welcome-title">Koren Code Editor</div>
        <p>A focused workspace for your code.</p>
        <div class="welcome-actions"><button class="primary-button" data-action="open">Open Folder</button><button data-action="open-file" id="welcome-open-file">Open File</button></div>
        <div class="welcome-hint">Open a folder <kbd>Ctrl+O</kbd> <span>·</span> Open a file <kbd>Ctrl+Shift+O</kbd> <span>·</span> Command Palette <kbd>Ctrl+Shift+P</kbd></div>
      </section>
      <div class="bottom-resizer" id="bottom-resizer" role="separator" aria-label="Resize bottom panel" aria-orientation="horizontal" tabindex="0"></div>
      <section class="bottom-panel" id="bottom-panel">
        <div class="bottom-heading"><div class="tool-tabs" role="tablist" aria-label="Tool panels">
          <button class="selected" data-panel="output" role="tab" aria-selected="true">Output</button><button data-panel="tasks" role="tab" aria-selected="false">Tasks</button><button data-panel="projects" role="tab" aria-selected="false">Projects</button>
        </div><span id="bottom-task-state" class="task-state"></span><span class="panel-actions"><button data-action="cancel-task" title="Cancel active task" aria-label="Cancel active task" disabled>■</button><button data-action="toggle-bottom" title="Collapse bottom panel" aria-label="Collapse bottom panel">⌄</button></span></div>
        <div id="output" class="panel-content output"><div class="panel-empty">Task output will appear here.</div></div>
        <div id="tasks" class="panel-content task-list hidden"><div class="panel-empty">Discover Cargo projects to see available tasks.</div></div>
        <div id="projects" class="panel-content problem-list hidden"><div class="panel-empty">Project information will appear here.</div></div>
      </section>
      <footer class="status-bar">
        <span id="status-message" class="status-message">Ready</span><span id="status-document" class="status-revision"></span><span class="status-spacer"></span>
        <span id="status-workspace" class="status-item"></span><span id="status-language" class="status-item"></span><span id="status-cursor" class="status-item"></span>
        <span id="status-indent" class="status-item"></span><span id="status-encoding" class="status-item"></span><span id="status-line-ending" class="status-item"></span><span id="status-theme" class="status-item">Dark</span>
      </footer>
    </section>
  </main>
  <dialog id="approval-dialog" class="dialog"><form method="dialog"><header>Run task?</header><p class="subtle">Review the command before it starts. Approval is not a sandbox.</p><dl id="approval-details"></dl><div class="dialog-actions"><button value="deny" class="secondary">Deny</button><button value="approve" class="primary">Approve once</button></div></form></dialog>
  <dialog id="conflict-dialog" class="dialog conflict"><form method="dialog"><header>File changed outside Koren</header><p id="conflict-path"></p><details><summary>Inspect current disk contents</summary><pre id="conflict-preview"></pre></details><p class="subtle">Choose whether to reload the external version or overwrite it with your buffer. If the file changes again, Koren will ask again.</p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="reload" class="secondary">Reload disk version</button><button value="overwrite" class="primary">Overwrite file</button></div></form></dialog>
  <dialog id="confirm-dialog" class="dialog"><form method="dialog"><header id="confirm-title">Unsaved changes</header><p id="confirm-message"></p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="discard" class="primary">Discard changes</button></div></form></dialog>
  <dialog id="close-dialog" class="dialog"><form method="dialog"><header>Unsaved documents</header><p>Save your changes before closing Koren Code Editor?</p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="discard" class="secondary">Discard and close</button><button value="save" class="primary">Save all and close</button></div></form></dialog>
  <dialog id="switch-workspace-dialog" class="dialog"><form method="dialog"><header>Switch workspace?</header><p>Choose what to do with open documents before changing folders.</p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="discard" class="secondary">Discard and switch</button><button value="save" class="primary">Save and switch</button></div></form></dialog>
  <dialog id="command-dialog" class="dialog command-dialog"><form method="dialog"><header>Command Palette</header><input id="command-search" type="search" placeholder="Search available commands" autocomplete="off" aria-label="Search commands"><div id="command-results" role="listbox" aria-label="Available commands"></div><footer><kbd>↑</kbd><kbd>↓</kbd> navigate <kbd>Enter</kbd> run <kbd>Esc</kbd> close</footer></form></dialog>
  <dialog id="info-dialog" class="dialog"><form method="dialog"><header id="info-title"></header><div id="info-content" class="info-content"></div><div class="dialog-actions"><button value="close" class="primary">Close</button></div></form></dialog>
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
const toast = document.querySelector<HTMLDivElement>('#status-toast')!;
const workbench = document.querySelector<HTMLElement>('#workbench')!;
const rootLabel = document.querySelector<HTMLElement>('#workspace-label')!;
let workspaceRoot = '';
let selectedPath = '';
let runId: number | null = null;
const documentViews = new Map<number, { state: EditorState; pipeline: DocumentEditPipeline; lineEnding: 'LF' | 'CRLF' }>();
const taskById = new Map<number, TaskSummary>();
let registeredCommands: string[] = [];
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
  const icon = document.createElement('span');
  icon.className = `tree-icon icon-${kind}`;
  icon.setAttribute('aria-hidden', 'true');
  icon.textContent = kind === 'directory' ? (expanded ? '▾' : '▸') : kind === 'symlink' ? '↗' : kind === 'rust' ? 'ƒ' : '·';
  parent.append(icon);
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
    appendIcon(row, entry.kind === 'file' && entry.name.toLowerCase().endsWith('.rs') ? 'rust' : entry.kind, node?.expanded);
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
    extensions: [basicSetup, theme, EditorState.tabSize.of(4), EditorState.lineSeparator.of(lineSeparator), EditorView.lineWrapping, ...languageExtensions,
      EditorView.updateListener.of((update) => {
        const entry = documentViews.get(tab.id);
        if (!entry) return;
        entry.state = update.state;
        if (update.docChanged) entry.pipeline.enqueue(editsFromViewUpdate(update));
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
  renderTree(); renderTabs(); updateStatusDocument(); editor?.focus();
}

function updateStatusDocument(): void {
  const tab = tabs.active;
  const revision = document.querySelector<HTMLElement>('#status-document')!;
  const details: Array<[string, string]> = [
    ['status-language', tab?.language ?? ''], ['status-cursor', ''], ['status-indent', tab ? 'Spaces: 4' : ''],
    ['status-encoding', tab ? 'UTF-8' : ''], ['status-line-ending', tab?.lineEnding ?? ''],
  ];
  if (tab && editor) {
    const line = editor.state.doc.lineAt(editor.state.selection.main.head);
    details[1]![1] = formatCursorPosition(line.number, editor.state.selection.main.head - line.from + 1);
  }
  for (const [id, value] of details) document.querySelector<HTMLElement>(`#${id}`)!.textContent = value;
  revision.textContent = tab ? `Rev ${tab.revision}${tab.dirty ? ' · Modified' : ''}` : '';
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
    tabs.close(tab.id); documentViews.delete(tab.id);
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
    runId = null; setCancelEnabled(false);
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
      setCancelEnabled(false);
      setStatus(`${taskById.get(snapshot.task_id)?.display_name ?? 'Task'} ${taskStatus(snapshot)}`);
    }
  } catch (error) { showError(error); }
}

function setCancelEnabled(enabled: boolean): void {
  const button = document.querySelector<HTMLButtonElement>('[data-action="cancel-task"]')!;
  button.disabled = !enabled;
}
function queueOutputRefresh(id: number): void {
  if (outputRefreshTimer) window.clearTimeout(outputRefreshTimer);
  outputRefreshTimer = window.setTimeout(() => { outputRefreshTimer = 0; void updateTaskOutput(id); }, 100);
}
function handlePlatformEvent(event: PlatformEvent): void {
  if (event.kind === 'task_output_available' && event.run_id) queueOutputRefresh(event.run_id);
  if (event.kind === 'task_finished' && event.run_id) void updateTaskOutput(event.run_id);
  if (event.kind === 'save_conflict_detected') setStatus('File changed outside Koren; review the save conflict.');
}

function setTheme(theme: 'dark' | 'light'): void {
  document.documentElement.dataset.theme = theme;
  persistTheme(localStorage, theme);
  document.querySelector('#status-theme')!.textContent = theme === 'dark' ? 'Dark' : 'Light';
}
function toggleTheme(): void { setTheme(nextTheme(document.documentElement.dataset.theme === 'light' ? 'light' : 'dark')); }

function showPanel(panel: 'output' | 'tasks' | 'projects'): void {
  for (const button of document.querySelectorAll<HTMLButtonElement>('[data-panel]')) {
    const selected = button.dataset.panel === panel;
    button.classList.toggle('selected', selected); button.setAttribute('aria-selected', String(selected));
  }
  for (const id of ['output', 'tasks', 'projects']) document.querySelector(`#${id}`)!.classList.toggle('hidden', id !== panel);
  setBottomCollapsed(false);
}

async function openCommandPalette(): Promise<void> {
  try {
    registeredCommands = expect(await client.call({ method: 'list_commands' }), 'commands').value;
    renderCommandResults('');
    const dialog = document.querySelector<HTMLDialogElement>('#command-dialog')!;
    const input = document.querySelector<HTMLInputElement>('#command-search')!;
    input.value = ''; dialog.showModal(); input.focus();
  } catch (error) { showError(error); }
}
let commandSelection = 0;
function renderCommandResults(query: string): void {
  const results = filterCommands(registeredCommands, query);
  commandSelection = results.length ? Math.min(commandSelection, results.length - 1) : -1;
  const host = document.querySelector<HTMLDivElement>('#command-results')!; host.replaceChildren();
  for (const [index, command] of results.entries()) {
    const button = document.createElement('button'); button.setAttribute('role', 'option');
    button.setAttribute('aria-selected', String(index === commandSelection)); button.textContent = command;
    button.addEventListener('mouseenter', () => {
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
    const result = expect(await client.call({ method: 'execute_command', params: { command_id: command } }), 'command_output').value;
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
    case 'show-panel': showPanel(target.dataset.panelTarget as 'output' | 'tasks' | 'projects'); break;
    case 'focus-explorer': setExplorerCollapsed(false); treeRoot.querySelector<HTMLButtonElement>('.tree-row')?.focus(); break;
    case 'focus-editor': editor?.focus(); break;
    case 'next-tab': cycleTab(1); break;
    case 'previous-tab': cycleTab(-1); break;
    case 'discover': void discoverCargo(); break;
    case 'run-cargo': void runCargoCommand(target.dataset.command ?? ''); break;
    case 'cancel-task': if (runId !== null) void client.call({ method: 'cancel_task', params: { run_id: runId } }).then(() => setStatus('Cancellation requested')).catch(showError); break;
    case 'refresh': if (workspaceRoot) void refreshDirectory(''); else setStatus('Open a workspace first'); break;
    case 'command-palette': void openCommandPalette(); break;
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

document.addEventListener('click', (event) => {
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
    const results = filterCommands(registeredCommands, input.value);
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault(); commandSelection = nextCommandSelection(commandSelection, results.length, event.key === 'ArrowDown' ? 1 : -1); renderCommandResults(input.value);
    } else if (event.key === 'Enter' && results[commandSelection]) {
      event.preventDefault(); void executeCommand(results[commandSelection]!);
    }
    return;
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

document.querySelectorAll<HTMLButtonElement>('[data-panel]').forEach((button) => button.addEventListener('click', () => showPanel(button.dataset.panel as 'output' | 'tasks' | 'projects')));

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
document.querySelector('#status-theme')!.textContent = document.documentElement.dataset.theme === 'light' ? 'Light' : 'Dark';
applyLayout();
if (layout.bottomCollapsed) document.querySelector('#bottom-resizer')!.classList.add('hidden');
renderTree(); renderTabs(); updateStatusDocument();
void (async () => { for await (const events of client.subscribeEvents()) events.forEach(handlePlatformEvent); })();

export { client, renderTree, renderTabs, showPanel };
