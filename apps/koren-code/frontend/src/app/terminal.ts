// biome-ignore lint/correctness/noUnresolvedImports: xterm exports use package ambient module declarations.
import { Terminal } from '@xterm/xterm';
// biome-ignore lint/correctness/noUnresolvedImports: the fit addon follows xterm's ambient module pattern.
import { FitAddon } from '@xterm/addon-fit';
import { KorenError, type KorenClient } from '../services/client';
import type { PlatformEvent, TerminalApprovalSummary, TerminalSessionSummary } from '../services/protocol';
import { decodeTerminalBytes, encodeTerminalBytes, terminalSequenceOrder } from './terminalTransport';
import '@xterm/xterm/css/xterm.css';

interface TerminalTab {
  summary: TerminalSessionSummary;
  title: string;
  terminal: Terminal;
  fit: FitAddon;
  host: HTMLDivElement;
  resizeObserver: ResizeObserver;
  lastSize: string;
  lastSequence: number;
  pendingInput: Uint8Array[];
  pendingInputBytes: number;
  inputTimer: number | null;
  sendingInput: boolean;
}

const MAX_TERMINAL_INPUT_BYTES = 64 * 1024;
const MAX_PENDING_INPUT_BYTES = 1024 * 1024;

export class TerminalPanel {
  private readonly tabs = new Map<number, TerminalTab>();
  private activeId: number | null = null;
  private pendingApproval: TerminalApprovalSummary | null = null;
  private readonly tabsHost: HTMLElement;
  private readonly viewsHost: HTMLElement;
  private readonly status: HTMLElement;
  private readonly approvalDialog: HTMLDialogElement;

  constructor(private readonly root: HTMLElement, private readonly client: KorenClient) {
    this.tabsHost = root.querySelector<HTMLElement>('[data-terminal-tabs]')!;
    this.viewsHost = root.querySelector<HTMLElement>('[data-terminal-views]')!;
    this.status = root.querySelector<HTMLElement>('[data-terminal-status]')!;
    this.approvalDialog = document.querySelector<HTMLDialogElement>('#terminal-approval-dialog')!;
    this.bindControls();
  }

  async open(): Promise<void> {
    try {
      const response = await this.client.call({ method: 'request_terminal_start' });
      if (response.kind !== 'terminal_approval_required') throw new Error('The terminal approval request was not returned.');
      this.showApproval(response.value);
    } catch (error) { this.setStatus(error instanceof Error ? error.message : String(error)); }
  }

  handleEvents(events: PlatformEvent[]): void {
    for (const event of events) {
      if (!event.terminal_id) continue;
      const tab = this.tabs.get(event.terminal_id);
      if (!tab) continue;
      if (event.kind === 'terminal_output' && event.data_base64) {
        if (event.sequence === undefined) continue;
        const order = terminalSequenceOrder(tab.lastSequence, event.sequence);
        if (order === 'duplicate') continue;
        if (order === 'gap') this.setStatus(`${tab.title}: terminal output sequence gap detected.`);
        tab.lastSequence = event.sequence;
        try { tab.terminal.write(decodeTerminalBytes(event.data_base64)); }
        catch { this.setStatus('Terminal output could not be decoded.'); }
      } else if (event.kind === 'terminal_state_changed') {
        tab.summary.state = event.terminal_state ?? tab.summary.state;
        tab.summary.exit_code = event.exit_code ?? null;
        tab.summary.process_group_active = event.terminal_process_group_active ?? tab.summary.process_group_active;
        tab.summary.error = event.message ?? null;
        if (tab.summary.state !== 'running') this.discardQueuedInput(tab, 'The shell exited before all queued input was sent.');
        this.renderTabs();
        if (tab.summary.state !== 'running') this.setStatus(`${tab.title}: ${tab.summary.state}${tab.summary.exit_code === null ? '' : ` (exit ${tab.summary.exit_code})`}`);
      } else if (event.kind === 'terminal_closed') {
        this.removeTab(event.terminal_id);
      }
    }
  }

  private bindControls(): void {
    this.root.querySelector<HTMLButtonElement>('[data-terminal-new]')!.addEventListener('click', () => void this.open());
    this.root.querySelector<HTMLButtonElement>('[data-terminal-rename]')!.addEventListener('click', () => this.renameActive());
    this.root.querySelector<HTMLButtonElement>('[data-terminal-close]')!.addEventListener('click', () => void this.closeActive());
    this.root.querySelector<HTMLButtonElement>('[data-terminal-restart]')!.addEventListener('click', () => void this.open());
    this.approvalDialog.querySelector<HTMLFormElement>('form')!.addEventListener('submit', (event) => {
      event.preventDefault();
      const choice = (event.submitter as HTMLButtonElement | null)?.value;
      const pending = this.pendingApproval;
      this.pendingApproval = null;
      this.approvalDialog.close();
      if (!pending) return;
      void (choice === 'approve' ? this.approve(pending) : this.deny(pending));
    });
    this.approvalDialog.addEventListener('cancel', (event) => {
      event.preventDefault();
      const pending = this.pendingApproval;
      this.pendingApproval = null;
      this.approvalDialog.close();
      if (pending) void this.deny(pending);
    });
  }

  private showApproval(approval: TerminalApprovalSummary): void {
    this.pendingApproval = approval;
    const details = this.approvalDialog.querySelector<HTMLElement>('[data-terminal-approval-details]')!;
    details.replaceChildren();
    const values: Array<[string, string]> = [
      ['Executable', approval.executable], ['Arguments', approval.arguments.join(' ') || '(none)'],
      ['Working directory', approval.working_directory], ['Environment names', approval.environment_keys.join(', ') || '(none)'],
      ['Requested by', approval.actor], ['Workspace', approval.workspace ?? '(none)'],
    ];
    if (approval.fallback_executable) values.push(['Fallback', approval.fallback_executable]);
    for (const [label, value] of values) {
      const term = document.createElement('dt'); term.textContent = label;
      const description = document.createElement('dd'); description.textContent = value;
      details.append(term, description);
    }
    this.approvalDialog.showModal();
  }

  private async approve(approval: TerminalApprovalSummary): Promise<void> {
    try {
      const response = await this.client.call({ method: 'approve_terminal_start', params: { request_id: approval.request_id } });
      if (response.kind !== 'terminal_approved') throw new Error('The shell did not start.');
      this.addSession(response.value.session);
    } catch (error) { this.setStatus(error instanceof Error ? error.message : String(error)); }
  }

  private async deny(approval: TerminalApprovalSummary): Promise<void> {
    try { await this.client.call({ method: 'deny_terminal_start', params: { request_id: approval.request_id } }); }
    catch (error) { this.setStatus(error instanceof Error ? error.message : String(error)); }
  }

  private addSession(summary: TerminalSessionSummary): void {
    if (this.tabs.has(summary.session_id)) return;
    const host = document.createElement('div');
    host.className = 'terminal-view';
    host.dataset.terminalSession = String(summary.session_id);
    const terminal = new Terminal({
      allowProposedApi: false,
      cursorBlink: true,
      convertEol: false,
      scrollback: 10_000,
      theme: {
        background: getComputedStyle(document.documentElement).getPropertyValue('--bg').trim() || '#000000',
        foreground: getComputedStyle(document.documentElement).getPropertyValue('--text').trim() || '#e8e8e8',
        cursor: getComputedStyle(document.documentElement).getPropertyValue('--accent').trim() || '#45a9ff',
        selectionBackground: 'rgba(69, 169, 255, 0.32)',
      },
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(host);
    const tab: TerminalTab = {
      summary, title: 'Terminal', terminal, fit, host, lastSize: '', lastSequence: 0,
      pendingInput: [], pendingInputBytes: 0, inputTimer: null, sendingInput: false,
      resizeObserver: new ResizeObserver(() => this.fit(tab)),
    };
    terminal.onData((text) => this.queueInput(tab, new TextEncoder().encode(text)));
    host.addEventListener('keydown', (event) => this.terminalShortcut(event, tab), true);
    this.viewsHost.append(host);
    this.tabs.set(summary.session_id, tab);
    tab.resizeObserver.observe(host);
    this.activeId = summary.session_id;
    this.renderTabs();
    this.activate(summary.session_id);
    this.setStatus(`${summary.profile_id} shell started in ${summary.working_directory}`);
  }

  private terminalShortcut(event: KeyboardEvent, tab: TerminalTab): void {
    const modifier = event.ctrlKey || event.metaKey;
    if (modifier && event.shiftKey && event.key.toLowerCase() === 'c') {
      if (tab.terminal.hasSelection()) { event.preventDefault(); void navigator.clipboard.writeText(tab.terminal.getSelection()).catch(() => this.setStatus('Clipboard access is unavailable.')); }
    } else if (modifier && event.shiftKey && event.key.toLowerCase() === 'v') {
      event.preventDefault(); void navigator.clipboard.readText().then((text) => tab.terminal.paste(text)).catch(() => this.setStatus('Clipboard access is unavailable.'));
    } else if (modifier && event.key.toLowerCase() === 'l') {
      event.preventDefault(); tab.terminal.clear();
    }
  }

  private queueInput(tab: TerminalTab, bytes: Uint8Array): void {
    if (tab.pendingInputBytes + bytes.length > MAX_PENDING_INPUT_BYTES) {
      this.setStatus(`${tab.title}: input queue reached 1 MiB; wait for the shell before pasting more.`);
      return;
    }
    tab.pendingInput.push(bytes);
    tab.pendingInputBytes += bytes.length;
    if (tab.inputTimer === null) tab.inputTimer = window.setTimeout(() => {
      tab.inputTimer = null;
      void this.flushInput(tab);
    }, 12);
  }

  private discardQueuedInput(tab: TerminalTab, reason: string): void {
    if (tab.pendingInputBytes > 0) this.setStatus(`${tab.title}: ${reason}`);
    tab.pendingInput = [];
    tab.pendingInputBytes = 0;
    if (tab.inputTimer !== null) window.clearTimeout(tab.inputTimer);
    tab.inputTimer = null;
  }

  private async flushInput(tab: TerminalTab): Promise<void> {
    if (tab.sendingInput || !tab.pendingInput.length || tab.summary.state !== 'running') return;
    let remaining = MAX_TERMINAL_INPUT_BYTES;
    const parts: Uint8Array[] = [];
    let length = 0;
    while (remaining > 0 && tab.pendingInput.length) {
      const next = tab.pendingInput.shift()!;
      const part = next.slice(0, remaining);
      parts.push(part);
      length += part.length;
      remaining -= part.length;
      if (next.length > part.length) tab.pendingInput.unshift(next.slice(part.length));
    }
    const chunk = new Uint8Array(length);
    let offset = 0;
    for (const part of parts) { chunk.set(part, offset); offset += part.length; }
    tab.sendingInput = true;
    try {
      await this.client.call({ method: 'terminal_write', params: { session_id: tab.summary.session_id, input_base64: encodeTerminalBytes(chunk) } });
      tab.pendingInputBytes -= chunk.length;
      tab.sendingInput = false;
      if (tab.pendingInput.length) void this.flushInput(tab);
    } catch (error) {
      tab.pendingInput.unshift(chunk);
      tab.sendingInput = false;
      if (error instanceof KorenError && error.code === 'busy') {
        if (tab.inputTimer === null) tab.inputTimer = window.setTimeout(() => {
          tab.inputTimer = null;
          void this.flushInput(tab);
        }, 24);
      } else {
        this.setStatus(error instanceof Error ? error.message : String(error));
      }
    }
  }

  private fit(tab: TerminalTab): void {
    if (tab.host.classList.contains('hidden') || tab.host.clientWidth === 0 || tab.host.clientHeight === 0) return;
    try {
      tab.fit.fit();
      const { rows, cols } = tab.terminal;
      const key = `${rows}x${cols}`;
      if (key !== tab.lastSize && tab.summary.state === 'running') {
        tab.lastSize = key;
        void this.client.call({ method: 'terminal_resize', params: { session_id: tab.summary.session_id, rows, columns: cols } })
          .catch((error: unknown) => this.setStatus(error instanceof Error ? error.message : String(error)));
      }
    } catch { /* The panel can briefly report a zero-sized viewport during layout. */ }
  }

  private renderTabs(): void {
    this.tabsHost.replaceChildren();
    for (const tab of this.tabs.values()) {
      const button = document.createElement('button');
      button.type = 'button'; button.className = 'terminal-session-tab';
      button.classList.toggle('selected', tab.summary.session_id === this.activeId);
      button.textContent = `${tab.title}${tab.summary.state === 'running' ? '' : ` · ${tab.summary.state}`}`;
      button.setAttribute('role', 'tab'); button.setAttribute('aria-selected', String(tab.summary.session_id === this.activeId));
      button.addEventListener('click', () => this.activate(tab.summary.session_id));
      this.tabsHost.append(button);
    }
    this.root.querySelector<HTMLButtonElement>('[data-terminal-rename]')!.disabled = this.activeId === null;
    this.root.querySelector<HTMLButtonElement>('[data-terminal-close]')!.disabled = this.activeId === null;
    this.root.querySelector<HTMLButtonElement>('[data-terminal-restart]')!.disabled = this.activeId === null || this.tabs.get(this.activeId)?.summary.state === 'running';
  }

  private activate(id: number): void {
    const tab = this.tabs.get(id); if (!tab) return;
    this.activeId = id;
    for (const view of this.viewsHost.querySelectorAll<HTMLElement>('[data-terminal-session]')) view.classList.toggle('hidden', view.dataset.terminalSession !== String(id));
    this.renderTabs();
    requestAnimationFrame(() => { this.fit(tab); tab.terminal.focus(); });
  }

  private renameActive(): void {
    const tab = this.activeId === null ? undefined : this.tabs.get(this.activeId);
    if (!tab) return;
    const name = window.prompt('Terminal tab name', tab.title)?.trim();
    if (name) { tab.title = name.slice(0, 48); this.renderTabs(); }
  }

  private async closeActive(): Promise<void> {
    if (this.activeId === null) return;
    const tab = this.tabs.get(this.activeId); if (!tab) return;
    const live = tab.summary.process_group_active || tab.summary.state === 'running' || tab.summary.state === 'starting';
    const terminate = live && window.confirm('This terminal is running a process. Close it and terminate the owned process group?');
    if (live && !terminate) return;
    try {
      await this.client.call({ method: 'close_terminal', params: { session_id: this.activeId, terminate_active: terminate } });
      this.removeTab(this.activeId);
    } catch (error) { this.setStatus(error instanceof Error ? error.message : String(error)); }
  }

  private removeTab(id: number): void {
    const tab = this.tabs.get(id); if (!tab) return;
    tab.resizeObserver.disconnect(); this.discardQueuedInput(tab, 'Queued input was not sent before the terminal closed.'); tab.terminal.dispose(); tab.host.remove(); this.tabs.delete(id);
    if (tab.inputTimer !== null) window.clearTimeout(tab.inputTimer);
    if (this.activeId === id) this.activeId = this.tabs.keys().next().value ?? null;
    if (this.activeId !== null) this.activate(this.activeId);
    this.renderTabs();
  }

  private setStatus(message: string): void { this.status.textContent = message; }
}
