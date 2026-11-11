import { basicSetup, EditorView } from 'codemirror';
import { EditorState } from '@codemirror/state';
import { KorenClient, KorenError, createDesktopTransport } from './services/client';
import type { CargoProjectSummary, ExecutionApprovalSummary, PlatformEvent, ResponseValue, TaskSummary } from './services/protocol';
import { ProjectTreeState, TabState, type EditorTab } from './app/state';
import { DocumentEditPipeline } from './editor/sync';
import { languageForPath } from './editor/languages';
import { flushAndSave } from './editor/save';
import { approvalChoice, conflictChoice } from './app/dialogs';
import { nextTheme, persistTheme, readTheme } from './app/theme';
import { taskStatus, visibleOutput } from './app/taskPresentation';
import { editsFromViewUpdate } from './editor/transactions';
import './styles/main.css';

const client = new KorenClient(createDesktopTransport());
const root = document.querySelector<HTMLDivElement>('#app')!;
root.innerHTML = `
  <header class="menu"><div class="brand">K</div><nav aria-label="Main menu"><button data-action="open">File</button><button data-action="save">Save</button><button data-action="theme">Theme</button><button data-action="discover">Build</button></nav><div class="menu-spacer"></div><span id="workspace-label">No workspace</span><button class="icon-button" data-action="theme" aria-label="Toggle theme">◐</button></header>
  <main class="workbench">
    <aside class="explorer" id="explorer"><div class="panel-heading"><span>PROJECT</span><div><button data-action="refresh" title="Refresh">↻</button><button data-action="collapse" title="Collapse explorer">‹</button></div></div><div id="tree" class="tree"></div><div class="explorer-footer"><button data-action="discover">Discover Cargo</button></div></aside>
    <div id="explorer-resizer" class="explorer-resizer" role="separator" aria-label="Resize project explorer" aria-orientation="vertical" tabindex="0"></div>
    <section class="main-column"><div id="tabs" class="tabs" role="tablist"></div><div id="editor" class="editor" aria-label="Code editor"></div><div id="empty-editor" class="empty-editor">Open a file to start editing.</div><section class="bottom-panel" id="bottom-panel"><div class="bottom-heading"><div class="tool-tabs"><button class="selected" data-panel="output">OUTPUT</button><button data-panel="tasks">TASKS</button><button data-panel="problems">PROJECTS</button></div><div><button data-action="cancel-task" title="Cancel active task" disabled>■</button><button data-action="toggle-bottom" title="Hide panel">⌄</button></div></div><div id="output" class="output panel-content"></div><div id="tasks" class="task-list panel-content hidden"></div><div id="problems" class="problem-list panel-content hidden"></div></section><footer class="status"><span id="status-message">Ready</span><span id="status-document"></span><span class="status-spacer"></span><span id="status-workspace"></span><span id="status-theme">Dark</span></footer></section>
  </main>
  <dialog id="approval-dialog" class="dialog"><form method="dialog"><header>Run task?</header><p class="subtle">This command can execute project-controlled code.</p><dl id="approval-details"></dl><div class="dialog-actions"><button value="deny" class="secondary">Deny</button><button value="approve" class="primary">Approve once</button></div></form></dialog>
  <dialog id="conflict-dialog" class="dialog conflict"><form method="dialog"><header>File changed outside Koren</header><p id="conflict-path"></p><details><summary>Inspect current disk contents</summary><pre id="conflict-preview"></pre></details><p class="subtle">Choose whether to reload the external version or overwrite it with your buffer. If the file changes again, Koren will ask again.</p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="reload" class="secondary">Reload disk version</button><button value="overwrite" class="primary">Overwrite file</button></div></form></dialog>
  <dialog id="confirm-dialog" class="dialog"><form method="dialog"><header id="confirm-title">Unsaved changes</header><p id="confirm-message"></p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="discard" class="primary">Discard changes</button></div></form></dialog>
  <dialog id="close-dialog" class="dialog"><form method="dialog"><header>Unsaved documents</header><p>Save your changes before closing Koren Code Editor?</p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="discard" class="secondary">Discard and close</button><button value="save" class="primary">Save all and close</button></div></form></dialog>
  <div id="status-toast" role="status" aria-live="polite"></div>`;

const treeState = new ProjectTreeState();
const tabs = new TabState();
const treeRoot = document.querySelector<HTMLDivElement>('#tree')!;
const editorHost = document.querySelector<HTMLDivElement>('#editor')!;
const emptyEditor = document.querySelector<HTMLDivElement>('#empty-editor')!;
const tabsHost = document.querySelector<HTMLDivElement>('#tabs')!;
const outputHost = document.querySelector<HTMLDivElement>('#output')!;
const taskHost = document.querySelector<HTMLDivElement>('#tasks')!;
const toast = document.querySelector<HTMLDivElement>('#status-toast')!;
const rootLabel = document.querySelector<HTMLElement>('#workspace-label')!;
let workspaceRoot = '';
let runId: number | null = null;
const documentViews = new Map<number, { state: EditorState; pipeline: DocumentEditPipeline }>();
const taskById = new Map<number, TaskSummary>();
const activeRuns = new Set<number>();
let editor: EditorView | null = null;
let outputRefreshTimer = 0;

function setStatus(message: string): void {
  document.querySelector('#status-message')!.textContent = message;
  toast.textContent = message;
  toast.classList.add('visible');
  window.setTimeout(() => toast.classList.remove('visible'), 2600);
}
function showError(error: unknown): void { setStatus(error instanceof Error ? error.message : String(error)); }
function expect<T extends ResponseValue['kind']>(response: ResponseValue, kind: T): Extract<ResponseValue, { kind: T }> {
  if (response.kind !== kind) throw new Error(`Expected ${kind}, received ${response.kind}`);
  return response as Extract<ResponseValue, { kind: T }>;
}

async function openWorkspace(path: string): Promise<void> {
  const value = expect(await client.call({ method: 'open_workspace', params: { path } }), 'workspace_opened').value;
  workspaceRoot = value.root;
  rootLabel.textContent = value.root;
  document.querySelector('#status-workspace')!.textContent = value.root;
  treeState.setEntries('', expect(await client.call({ method: 'list_directory', params: { path: '' } }), 'directory_entries').value);
  renderTree();
  await refreshProjects();
  setStatus('Workspace opened');
}

async function refreshDirectory(path: string): Promise<void> {
  treeState.setLoading(path, true); renderTree();
  try {
    const entries = expect(await client.call({ method: 'list_directory', params: { path } }), 'directory_entries').value;
    treeState.setEntries(path, entries);
  } catch (error) { treeState.setError(path, String(error)); }
  renderTree();
}

function renderTree(): void {
  treeRoot.replaceChildren();
  if (!workspaceRoot) {
    const blank = document.createElement('div'); blank.className = 'tree-empty'; blank.textContent = 'Open a folder to browse files.'; treeRoot.append(blank); return;
  }
  const heading = document.createElement('div'); heading.className = 'root-row'; heading.textContent = workspaceRoot.split('/').at(-1) || workspaceRoot; treeRoot.append(heading);
  if (treeState.nodes.get('')?.loading) { const label = document.createElement('div'); label.className = 'tree-message'; label.textContent = 'Loading…'; treeRoot.append(label); }
  if (treeState.nodes.get('')?.error) { const label = document.createElement('div'); label.className = 'tree-error'; label.textContent = treeState.nodes.get('')!.error!; treeRoot.append(label); }
  if (!treeState.nodes.get('')?.entries.length && !treeState.nodes.get('')?.loading) { const label = document.createElement('div'); label.className = 'tree-message'; label.textContent = 'This folder is empty.'; treeRoot.append(label); }
  for (const { entry, depth } of treeState.visible()) {
    const row = document.createElement('button'); row.className = `tree-row ${entry.kind}`; row.style.paddingLeft = `${10 + depth * 14}px`; row.title = entry.path;
    const node = treeState.nodes.get(entry.path);
    row.textContent = `${entry.kind === 'directory' ? node?.expanded ? '▾' : '▸' : entry.kind === 'file' ? '·' : '↗'}  ${entry.name}`;
    row.addEventListener('click', () => {
      if (entry.kind === 'directory') { const open = treeState.toggle(entry.path); if (open && !node?.entries.length) void refreshDirectory(entry.path); else renderTree(); }
      else if (entry.kind === 'file') void openDocument(entry.path);
    });
    treeRoot.append(row);
    if (node?.loading) { const loading = document.createElement('div'); loading.className = 'tree-message'; loading.style.paddingLeft = `${20 + (depth + 1) * 14}px`; loading.textContent = 'Loading…'; treeRoot.append(loading); }
    if (node?.error) { const err = document.createElement('div'); err.className = 'tree-error'; err.style.paddingLeft = `${20 + (depth + 1) * 14}px`; err.textContent = node.error; treeRoot.append(err); }
  }
}

async function openDocument(path: string): Promise<void> {
  try {
    const opened = expect(await client.call({ method: 'open_document', params: { path } }), 'document_opened').value;
    const alreadyOpen = tabs.tabs.find((tab) => tab.id === opened.document_id);
    if (alreadyOpen) { selectTab(alreadyOpen.id); return; }
    const content = expect(await client.call({ method: 'read_document', params: { document_id: opened.document_id } }), 'document_contents').value;
    const tab: EditorTab = { id: opened.document_id, path, title: path.split('/').at(-1) || path, revision: content.revision, dirty: content.dirty };
    tabs.open(tab);
    const { state, pipeline } = createDocumentState(tab, content.contents, content.revision);
    documentViews.set(tab.id, { state, pipeline });
    if (!editor) editor = new EditorView({ state, parent: editorHost }); else editor.setState(state);
    emptyEditor.classList.add('hidden'); editorHost.classList.remove('hidden');
    renderTabs(); updateStatusDocument();
    editor.focus();
  } catch (error) { showError(error); }
}

function createDocumentState(tab: EditorTab, contents: string, revision: number): { state: EditorState; pipeline: DocumentEditPipeline } {
  const lineSeparator = contents.includes('\r\n') ? '\r\n' : '\n';
  const state = EditorState.create({ doc: contents, extensions: [basicSetup, EditorState.lineSeparator.of(lineSeparator), ...languageForPath(tab.path), EditorView.updateListener.of((update) => {
    const entry = documentViews.get(tab.id);
    if (!entry) return;
    entry.state = update.state;
    if (update.docChanged) entry.pipeline.enqueue(editsFromViewUpdate(update));
  })] });
  const pipeline = new DocumentEditPipeline(revision, {
    edit: async (expectedRevision, edits) => expect(await client.call({ method: 'apply_document_edits', params: { document_id: tab.id, expected_revision: expectedRevision, edits } }), 'document_changed').value,
    read: async () => { const snapshot = expect(await client.call({ method: 'read_document', params: { document_id: tab.id } }), 'document_contents').value; return { contents: snapshot.contents, revision: snapshot.revision, dirty: snapshot.dirty }; },
  }, () => documentViews.get(tab.id)?.state.doc.toString() ?? contents,
  (newRevision, dirty) => { tabs.update(tab.id, { revision: newRevision, dirty }); renderTabs(); updateStatusDocument(); }, showError);
  return { state, pipeline };
}

function renderTabs(): void {
  tabsHost.replaceChildren();
  for (const tab of tabs.tabs) {
    const button = document.createElement('button'); button.className = `tab ${tab.id === tabs.activeId ? 'active' : ''}`; button.setAttribute('role', 'tab'); button.setAttribute('aria-selected', String(tab.id === tabs.activeId));
    const label = document.createElement('span'); label.textContent = `${tab.dirty ? '● ' : ''}${tab.title}`; button.append(label);
    const close = document.createElement('span'); close.className = 'tab-close'; close.textContent = '×'; close.title = 'Close'; close.addEventListener('click', (event) => { event.stopPropagation(); void closeTab(tab); }); button.append(close);
    button.addEventListener('click', () => selectTab(tab.id)); tabsHost.append(button);
  }
  if (!tabs.tabs.length) { editorHost.classList.add('hidden'); emptyEditor.classList.remove('hidden'); }
}
function selectTab(id: number): void {
  if (!tabs.select(id)) return;
  const entry = documentViews.get(id); if (entry && editor) editor.setState(entry.state);
  renderTabs(); updateStatusDocument(); editor?.focus();
}
function updateStatusDocument(): void {
  const tab = tabs.active;
  document.querySelector('#status-document')!.textContent = tab ? `Rev ${tab.revision}${tab.dirty ? ' • Modified' : ''}` : '';
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
  } catch (error) { showError(error); }
}

async function saveActive(): Promise<void> {
  const tab = tabs.active; if (!tab) return;
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
      const entry = documentViews.get(tab.id);
      if (!entry) return false;
      const result = await flushAndSave(entry.pipeline, async (revision) => expect(await client.call({ method: 'save_document', params: { document_id: tab.id, expected_revision: revision } }), 'document_saved').value);
      tabs.update(tab.id, { revision: result.revision, dirty: result.dirty });
    } catch (error) {
      if (error instanceof KorenError && error.code === 'conflict') await showConflict(tab);
      else showError(error);
      return false;
    }
  }
  renderTabs(); updateStatusDocument();
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
      const { state, pipeline } = createDocumentState(tab, content.contents, content.revision);
      documentViews.set(tab.id, { state, pipeline });
      if (tabs.activeId === tab.id && editor) editor.setState(state);
    }
    renderTabs(); updateStatusDocument();
  } catch (error) { showError(error); }
}

async function refreshProjects(): Promise<void> {
  try {
    const list = expect(await client.call({ method: 'list_projects' }), 'projects').value;
    const host = document.querySelector('#problems')!; host.textContent = list.map((project) => `${project.display_name} · ${project.provider_id}`).join('\n') || 'No projects discovered.';
  } catch (error) { showError(error); }
}
async function discoverCargo(): Promise<void> {
  if (!workspaceRoot) { setStatus('Open a workspace first'); return; }
  try {
    const response = expect(await client.call({ method: 'discover_cargo_projects', params: { max_directories: 6000, max_projects: 256, max_depth: 32 } }), 'cargo_projects');
    renderCargoProjects(response.value);
    const tasks = expect(await client.call({ method: 'list_tasks' }), 'tasks').value;
    taskById.clear(); for (const task of tasks) taskById.set(task.task_id, task);
    renderTasks(tasks); setStatus(`Found ${response.value.length} Cargo project${response.value.length === 1 ? '' : 's'}`);
  } catch (error) { showError(error); }
}
function renderCargoProjects(projects: CargoProjectSummary[]): void {
  const panel = document.querySelector('#problems')!; panel.replaceChildren();
  for (const project of projects) {
    const row = document.createElement('div'); row.className = 'cargo-project';
    const name = document.createElement('strong'); name.textContent = project.project.display_name; row.append(name);
    const meta = document.createElement('span'); meta.textContent = `${project.is_workspace ? 'Workspace' : 'Package'} · ${project.members.length} members`; row.append(meta); panel.append(row);
  }
  if (!projects.length) panel.textContent = 'No Cargo projects found.';
}
function renderTasks(tasks: TaskSummary[]): void {
  taskHost.replaceChildren();
  for (const task of tasks) {
    if (task.category === 'custom') continue;
    const row = document.createElement('div'); row.className = 'task-row';
    const title = document.createElement('span'); title.textContent = task.display_name; row.append(title);
    const run = document.createElement('button'); run.textContent = 'Run'; run.addEventListener('click', () => void requestTask(task)); row.append(run); taskHost.append(row);
  }
  if (!taskHost.childElementCount) taskHost.textContent = 'Discover Cargo projects to load available tasks.';
}
async function requestTask(task: TaskSummary): Promise<void> {
  try {
    const response = await client.call({ method: 'request_task_execution', params: { task_id: task.task_id } });
    if (response.kind === 'approval_required') await showApproval(response.value);
    else if (response.kind === 'task_started') { runId = response.value.run_id; activeRuns.add(runId); setCancelEnabled(true); setStatus('Task started'); await updateTaskOutput(runId); }
    else throw new Error(`Unexpected task response: ${response.kind}`);
  } catch (error) { showError(error); }
}
async function showApproval(approval: ExecutionApprovalSummary): Promise<void> {
  const details = document.querySelector<HTMLDListElement>('#approval-details')!; details.replaceChildren();
  const values: Array<[string, string]> = [['Task', approval.display_name], ['Executable', approval.executable], ['Arguments', JSON.stringify(approval.arguments)], ['Working directory', approval.working_directory], ['Project', approval.project_id?.toString() ?? 'Workspace'], ['Actor', approval.requesting_actor], ['Environment keys', approval.environment_keys.join(', ') || 'None']];
  for (const [key, value] of values) { const dt = document.createElement('dt'); dt.textContent = key; const dd = document.createElement('dd'); dd.textContent = value; details.append(dt, dd); }
  const dialog = document.querySelector<HTMLDialogElement>('#approval-dialog')!; dialog.showModal();
  const action = approvalChoice(await new Promise<string>((resolve) => dialog.addEventListener('close', () => resolve(dialog.returnValue), { once: true })));
  if (!action) return;
  try {
    const response = await client.call(action === 'approve' ? { method: 'approve_task_execution', params: { request_id: approval.request_id } } : { method: 'deny_task_execution', params: { request_id: approval.request_id } });
    if (response.kind === 'task_approved') { runId = response.value.run_id; activeRuns.add(runId); setCancelEnabled(true); setStatus('Task approved and started'); await updateTaskOutput(runId); }
    else if (response.kind === 'task_denied') setStatus('Task denied');
  } catch (error) { showError(error); }
}
async function updateTaskOutput(id: number): Promise<void> {
  try {
    const result = expect(await client.call({ method: 'task_output', params: { run_id: id } }), 'task_snapshot').value;
    outputHost.textContent = visibleOutput(result);
    document.querySelector('.bottom-heading')!.setAttribute('data-task-state', taskStatus(result));
    if (result.state !== 'running') { activeRuns.delete(id); if (runId === id) setCancelEnabled(false); }
  } catch (error) { showError(error); }
}
function setCancelEnabled(enabled: boolean): void { const button = document.querySelector<HTMLButtonElement>('[data-action="cancel-task"]')!; button.disabled = !enabled; }
function queueOutputRefresh(id: number): void {
  if (outputRefreshTimer) window.clearTimeout(outputRefreshTimer);
  outputRefreshTimer = window.setTimeout(() => { outputRefreshTimer = 0; void updateTaskOutput(id); }, 100);
}
function handlePlatformEvent(event: PlatformEvent): void {
  if (event.kind === 'task_output_available' && event.run_id) queueOutputRefresh(event.run_id);
  if (event.kind === 'task_finished' && event.run_id) { void updateTaskOutput(event.run_id); setStatus('Task completed'); }
  if (event.kind === 'save_conflict_detected') setStatus('File changed outside Koren; review the save conflict.');
}

async function chooseWorkspace(): Promise<void> {
  try {
    const response = await fetch('/picker', { cache: 'no-store' });
    const { path } = await response.json() as { path: string | null };
    if (path) await openWorkspace(path);
  } catch (error) { showError(error); }
}

window.addEventListener('koren-close-request', () => {
  void (async () => {
    const dialog = document.querySelector<HTMLDialogElement>('#close-dialog')!;
    const dirty = tabs.tabs.some((tab) => tab.dirty);
    if (!dirty) { window.ipc?.postMessage(JSON.stringify({ type: 'close_decision', allow: true })); return; }
    dialog.showModal();
    const action = await new Promise<string>((resolve) => dialog.addEventListener('close', () => resolve(dialog.returnValue), { once: true }));
    if (action === 'save') {
      const saved = await saveAll();
      window.ipc?.postMessage(JSON.stringify({ type: 'close_decision', allow: saved }));
    } else if (action === 'discard') window.ipc?.postMessage(JSON.stringify({ type: 'close_decision', allow: true }));
    else window.ipc?.postMessage(JSON.stringify({ type: 'close_decision', allow: false }));
  })();
});

document.addEventListener('click', (event) => {
  const target = (event.target as HTMLElement).closest<HTMLElement>('[data-action]'); if (!target) return;
  switch (target.dataset.action) {
    case 'open': void chooseWorkspace(); break;
    case 'save': void saveActive(); break;
    case 'theme': toggleTheme(); break;
    case 'discover': void discoverCargo(); document.querySelector('[data-panel="tasks"]')?.dispatchEvent(new Event('click')); break;
    case 'refresh': if (workspaceRoot) void refreshDirectory(''); break;
    case 'collapse': document.querySelector('#explorer')!.classList.toggle('collapsed'); workbench.classList.toggle('explorer-collapsed'); break;
    case 'toggle-bottom': document.querySelector('#bottom-panel')!.classList.toggle('collapsed'); break;
    case 'cancel-task': if (runId !== null) void client.call({ method: 'cancel_task', params: { run_id: runId } }).then(() => setStatus('Cancellation requested')).catch(showError); break;
  }
});

const workbench = document.querySelector<HTMLElement>('.workbench')!;
document.querySelector<HTMLElement>('#explorer-resizer')!.addEventListener('pointerdown', (event) => {
  const move = (moveEvent: PointerEvent) => { workbench.style.setProperty('--explorer-width', `${Math.max(140, Math.min(moveEvent.clientX, window.innerWidth * 0.45))}px`); };
  const stop = () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', stop); };
  window.addEventListener('pointermove', move); window.addEventListener('pointerup', stop, { once: true });
  event.preventDefault();
});
document.querySelector<HTMLElement>('#explorer-resizer')!.addEventListener('keydown', (event) => {
  if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
  const current = Number.parseFloat(getComputedStyle(workbench).gridTemplateColumns.split(' ')[0] ?? '225');
  workbench.style.setProperty('--explorer-width', `${Math.max(140, current + (event.key === 'ArrowRight' ? 12 : -12))}px`);
  event.preventDefault();
});
document.querySelectorAll<HTMLButtonElement>('[data-panel]').forEach((button) => button.addEventListener('click', () => {
  document.querySelectorAll('[data-panel]').forEach((item) => item.classList.remove('selected'));
  button.classList.add('selected');
  for (const id of ['output', 'tasks', 'problems']) document.querySelector(`#${id}`)!.classList.toggle('hidden', id !== button.dataset.panel);
}));
document.addEventListener('keydown', (event) => {
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') { event.preventDefault(); void saveActive(); }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'w' && tabs.active) { event.preventDefault(); void closeTab(tabs.active); }
  if (event.key === 'F5' && runId !== null) { event.preventDefault(); const id = runId; void client.call({ method: 'cancel_task', params: { run_id: id } }).then(() => setStatus('Cancellation requested')).catch(showError); }
});

function toggleTheme(): void {
  const html = document.documentElement;
  const theme = nextTheme(html.dataset.theme === 'light' ? 'light' : 'dark');
  html.dataset.theme = theme;
  persistTheme(localStorage, theme);
  document.querySelector('#status-theme')!.textContent = theme === 'dark' ? 'Dark' : 'Light';
}
document.documentElement.dataset.theme = readTheme(localStorage);
document.querySelector('#status-theme')!.textContent = document.documentElement.dataset.theme === 'light' ? 'Light' : 'Dark';
renderTree(); renderTabs();
void (async () => { for await (const events of client.subscribeEvents()) events.forEach(handlePlatformEvent); })();

export { client, renderTree, renderTabs };
