import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { KorenClient } from '../services/client';
import type { TerminalApprovalSummary, TerminalSessionSummary } from '../services/protocol';
import { decodeTerminalBytes, encodeTerminalBytes } from './terminalTransport';

const terminalMocks = vi.hoisted(() => ({ instances: [] as Array<Record<string, unknown>> }));

vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    rows = 24;
    cols = 80;
    input: ((text: string) => void) | null = null;
    output: Uint8Array[] = [];
    focused = false;
    cleared = false;
    opened = false;
    onData(callback: (text: string) => void) { this.input = callback; }
    loadAddon() {}
    open() { this.opened = true; }
    write(bytes: Uint8Array) { this.output.push(bytes); }
    focus() { this.focused = true; }
    hasSelection() { return true; }
    getSelection() { return 'selected'; }
    paste(text: string) { this.input?.(text); }
    clear() { this.cleared = true; }
    dispose() { this.opened = false; }
    constructor() { terminalMocks.instances.push(this as unknown as Record<string, unknown>); }
  },
}));

vi.mock('@xterm/addon-fit', () => ({ FitAddon: class { fit() {} } }));

const approval: TerminalApprovalSummary = {
  request_id: 11, profile_id: 'default', profile_name: 'Terminal', executable: '/bin/sh', arguments: [],
  working_directory: '/tmp/workspace', environment_keys: [], fallback_executable: '/bin/sh',
  actor: 'interactive_user', workspace: '/tmp/workspace',
};

function session(session_id: number): TerminalSessionSummary {
  return { session_id, profile_id: 'default', working_directory: '/tmp/workspace', state: 'running', exit_code: null, process_group_active: true, error: null, rows: 24, columns: 80 };
}

describe('integrated terminal panel', () => {
  beforeEach(() => {
    terminalMocks.instances.length = 0;
    document.body.innerHTML = `
      <section id="terminal">
        <button data-terminal-new></button><button data-terminal-rename></button>
        <button data-terminal-restart></button><button data-terminal-close></button>
        <span data-terminal-status></span><div data-terminal-tabs></div><div data-terminal-views></div>
      </section>
      <dialog id="terminal-approval-dialog"><form><dl data-terminal-approval-details></dl>
        <button type="submit" value="deny"></button><button type="submit" value="approve"></button>
      </form></dialog>`;
    HTMLDialogElement.prototype.showModal = function showModal() { this.open = true; };
    HTMLDialogElement.prototype.close = function close() { this.open = false; };
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    vi.spyOn(window, 'prompt').mockReturnValue('build shell');
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    Object.defineProperties(HTMLElement.prototype, {
      clientWidth: { configurable: true, get: () => 800 },
      clientHeight: { configurable: true, get: () => 220 },
    });
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: vi.fn(async () => {}), readText: vi.fn(async () => 'paste') } });
  });

  it('retains output received before the approved session response', async () => {
    let finish!: (value: unknown) => void;
    const pending = new Promise((resolve) => { finish = resolve; });
    const client = { call: vi.fn(async (request: { method: string }) => {
      if (request.method === 'request_terminal_start') return { kind: 'terminal_approval_required', value: approval };
      if (request.method === 'approve_terminal_start') return pending;
      return { kind: 'terminal_action', value: { session_id: 1 } };
    }) } as unknown as KorenClient;
    const { TerminalPanel } = await import('./terminal');
    const panel = new TerminalPanel(document.querySelector<HTMLElement>('#terminal')!, client);
    await panel.open();
    const form = document.querySelector<HTMLFormElement>('#terminal-approval-dialog form')!;
    form.dispatchEvent(new SubmitEvent('submit', { bubbles: true, cancelable: true, submitter: form.querySelector<HTMLButtonElement>('[value="approve"]') }));
    const bytes = new TextEncoder().encode('early prompt');
    panel.handleEvents([{ kind: 'terminal_output', terminal_id: 1, sequence: 1, data_base64: encodeTerminalBytes(bytes) }]);
    finish({ kind: 'terminal_approved', value: { request_id: 11, session: session(1) } });
    await vi.waitFor(() => expect(document.querySelectorAll('.terminal-session-tab')).toHaveLength(1));
    const terminal = terminalMocks.instances[0] as { output: Uint8Array[] };
    expect(terminal.output.map((chunk) => Array.from(chunk))).toEqual([Array.from(bytes)]);
    panel.handleEvents([{ kind: 'terminal_output', terminal_id: 1, sequence: 2, data_base64: encodeTerminalBytes(bytes) }]);
    expect(document.querySelector('[data-terminal-status]')?.textContent).not.toContain('sequence gap');
  });

  it('approves sessions, batches input, renders ordered bytes, switches, renames, and closes live tabs', async () => {
    const calls: Array<{ method: string; params?: Record<string, unknown> }> = [];
    let nextSessionId = 1;
    const client = {
      call: vi.fn(async (request: { method: string; params?: Record<string, unknown> }) => {
        calls.push(request);
        if (request.method === 'request_terminal_start') return { kind: 'terminal_approval_required', value: { ...approval, request_id: calls.filter((item) => item.method === 'request_terminal_start').length } };
        if (request.method === 'approve_terminal_start') return { kind: 'terminal_approved', value: { request_id: request.params?.request_id, session: session(nextSessionId++) } };
        return { kind: 'terminal_action', value: { session_id: request.params?.session_id ?? 1 } };
      }),
    } as unknown as KorenClient;
    const { TerminalPanel } = await import('./terminal');
    const panel = new TerminalPanel(document.querySelector<HTMLElement>('#terminal')!, client);

    document.querySelector<HTMLButtonElement>('[data-terminal-new]')!.click();
    await vi.waitFor(() => expect(document.querySelector<HTMLDialogElement>('#terminal-approval-dialog')!.open).toBe(true));
    expect(document.querySelector('[data-terminal-approval-details]')!.textContent).toContain('/bin/sh');
    const form = document.querySelector<HTMLFormElement>('#terminal-approval-dialog form')!;
    const submit = (value: string) => {
      const event = new SubmitEvent('submit', { bubbles: true, cancelable: true, submitter: form.querySelector<HTMLButtonElement>(`[value="${value}"]`) });
      form.dispatchEvent(event);
    };
    submit('approve');
    await vi.waitFor(() => expect(document.querySelectorAll('.terminal-session-tab')).toHaveLength(1));
    await vi.waitFor(() => expect(calls.some((item) => item.method === 'terminal_resize')).toBe(true));
    const first = terminalMocks.instances[0] as { input: (text: string) => void; output: Uint8Array[]; cleared: boolean };
    first.input('λ'); first.input('\n');
    await vi.waitFor(() => expect(calls.some((item) => item.method === 'terminal_write')).toBe(true));
    const input = calls.find((item) => item.method === 'terminal_write')!;
    expect(input.params?.input_base64).toBe(encodeTerminalBytes(new TextEncoder().encode('λ\n')));

    const output = Uint8Array.from([0, 0x1b, 0xff, 10]);
    panel.handleEvents([{ kind: 'terminal_output', terminal_id: 1, sequence: 1, data_base64: encodeTerminalBytes(output) }]);
    panel.handleEvents([{ kind: 'terminal_output', terminal_id: 1, sequence: 1, data_base64: encodeTerminalBytes(Uint8Array.from([1])) }]);
    expect(first.output).toHaveLength(1);
    expect(decodeTerminalBytes(encodeTerminalBytes(first.output[0]!))).toEqual(output);
    const firstHost = document.querySelector<HTMLElement>('[data-terminal-session="1"]')!;
    firstHost.dispatchEvent(new KeyboardEvent('keydown', { key: 'c', ctrlKey: true, shiftKey: true, bubbles: true }));
    await vi.waitFor(() => expect(navigator.clipboard.writeText).toHaveBeenCalledWith('selected'));
    firstHost.dispatchEvent(new KeyboardEvent('keydown', { key: 'l', ctrlKey: true, bubbles: true }));
    expect(first.cleared).toBe(true);

    panel.handleEvents([{ kind: 'terminal_state_changed', terminal_id: 1, terminal_state: 'exited', exit_code: 7, terminal_process_group_active: true }]);
    expect(document.querySelector('.terminal-session-tab')?.textContent).toContain('exited');
    expect(document.querySelector('[data-terminal-status]')?.textContent).toContain('exit 7');
    document.querySelector<HTMLButtonElement>('[data-terminal-restart]')!.click();
    await vi.waitFor(() => expect(document.querySelector<HTMLDialogElement>('#terminal-approval-dialog')!.open).toBe(true));
    submit('deny');
    await vi.waitFor(() => expect(calls.some((item) => item.method === 'deny_terminal_start')).toBe(true));

    document.querySelector<HTMLButtonElement>('[data-terminal-new]')!.click();
    await vi.waitFor(() => expect(document.querySelector<HTMLDialogElement>('#terminal-approval-dialog')!.open).toBe(true));
    submit('approve');
    await vi.waitFor(() => expect(document.querySelectorAll('.terminal-session-tab')).toHaveLength(2));
    document.querySelector<HTMLButtonElement>('[data-terminal-rename]')!.click();
    expect(document.querySelector('.terminal-session-tab.selected')?.textContent).toContain('build shell');
    document.querySelector<HTMLButtonElement>('[data-terminal-close]')!.click();
    await vi.waitFor(() => expect(document.querySelectorAll('.terminal-session-tab')).toHaveLength(1));
    expect(calls.some((item) => item.method === 'close_terminal' && item.params?.terminate_active === true)).toBe(true);
  });

  it('asks to terminate a surviving process group after the shell exits', async () => {
    const calls: Array<{ method: string; params?: Record<string, unknown> }> = [];
    const client = { call: vi.fn(async (request: { method: string; params?: Record<string, unknown> }) => {
      calls.push(request);
      if (request.method === 'request_terminal_start') return { kind: 'terminal_approval_required', value: approval };
      if (request.method === 'approve_terminal_start') return { kind: 'terminal_approved', value: { request_id: 11, session: session(1) } };
      return { kind: 'terminal_action', value: { session_id: 1 } };
    }) } as unknown as KorenClient;
    const { TerminalPanel } = await import('./terminal');
    const panel = new TerminalPanel(document.querySelector<HTMLElement>('#terminal')!, client);
    document.querySelector<HTMLButtonElement>('[data-terminal-new]')!.click();
    await vi.waitFor(() => expect(document.querySelector<HTMLDialogElement>('#terminal-approval-dialog')!.open).toBe(true));
    const form = document.querySelector<HTMLFormElement>('#terminal-approval-dialog form')!;
    form.dispatchEvent(new SubmitEvent('submit', { bubbles: true, cancelable: true, submitter: form.querySelector<HTMLButtonElement>('[value="approve"]') }));
    await vi.waitFor(() => expect(document.querySelectorAll('.terminal-session-tab')).toHaveLength(1));
    panel.handleEvents([{ kind: 'terminal_state_changed', terminal_id: 1, terminal_state: 'exited', terminal_process_group_active: true }]);
    document.querySelector<HTMLButtonElement>('[data-terminal-close]')!.click();
    await vi.waitFor(() => expect(calls.some((item) => item.method === 'close_terminal' && item.params?.terminate_active === true)).toBe(true));
  });

  it('restarts an exited terminal by replacing the old session after approval', async () => {
    const calls: Array<{ method: string; params?: Record<string, unknown> }> = [];
    let nextSessionId = 1;
    const client = { call: vi.fn(async (request: { method: string; params?: Record<string, unknown> }) => {
      calls.push(request);
      if (request.method === 'request_terminal_start') return { kind: 'terminal_approval_required', value: { ...approval, request_id: nextSessionId } };
      if (request.method === 'approve_terminal_start') return { kind: 'terminal_approved', value: { request_id: request.params?.request_id, session: session(nextSessionId++) } };
      return { kind: 'terminal_action', value: { session_id: request.params?.session_id ?? 1 } };
    }) } as unknown as KorenClient;
    const { TerminalPanel } = await import('./terminal');
    const panel = new TerminalPanel(document.querySelector<HTMLElement>('#terminal')!, client);
    document.querySelector<HTMLButtonElement>('[data-terminal-new]')!.click();
    await vi.waitFor(() => expect(document.querySelector<HTMLDialogElement>('#terminal-approval-dialog')!.open).toBe(true));
    const form = document.querySelector<HTMLFormElement>('#terminal-approval-dialog form')!;
    const submit = () => form.dispatchEvent(new SubmitEvent('submit', { bubbles: true, cancelable: true, submitter: form.querySelector<HTMLButtonElement>('[value="approve"]') }));
    submit();
    await vi.waitFor(() => expect(document.querySelectorAll('.terminal-session-tab')).toHaveLength(1));
    panel.handleEvents([{ kind: 'terminal_state_changed', terminal_id: 1, terminal_state: 'exited', terminal_process_group_active: false }]);
    document.querySelector<HTMLButtonElement>('[data-terminal-restart]')!.click();
    await vi.waitFor(() => expect(document.querySelector<HTMLDialogElement>('#terminal-approval-dialog')!.open).toBe(true));
    submit();
    await vi.waitFor(() => expect(calls.some((call) => call.method === 'close_terminal' && call.params?.session_id === 1)).toBe(true));
    await vi.waitFor(() => expect(document.querySelectorAll('.terminal-session-tab')).toHaveLength(1));
    expect(document.querySelector('.terminal-session-tab.selected')?.textContent).not.toContain('exited');
  });
});
