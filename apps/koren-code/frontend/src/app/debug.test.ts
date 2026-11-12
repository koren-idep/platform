import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { KorenClient } from '../services/client';
import type { DebugConfigurationSummary, DebugSessionInfo, ResponseValue } from '../services/protocol';

const configuration: DebugConfigurationSummary = {
  id: 'sample.launch', name: 'Sample', adapter_id: 'sample.dap', adapter_name: 'Sample DAP',
  request: 'launch', program: '/workspace/app', process_id: null,
};
const session: DebugSessionInfo = {
  session_id: 1, adapter_id: 'sample.dap', configuration_id: 'sample.launch', request: 'launch',
  state: 'paused', capabilities: { requests: { configuration_done: true, restart: false, cancel: false },
    execution: { terminate: true }, inspection: { variable_paging: true, evaluate_for_hovers: false } },
  stop_generation: 2, stopped_thread_id: 7, exit_code: null, error: null,
};

function markup(): void {
  document.body.innerHTML = `
    <main id="debug">
      <select data-debug-configuration></select><button data-debug-start></button>
      <button data-debug-continue></button><button data-debug-pause></button>
      <button data-debug-step-over></button><button data-debug-step-into></button>
      <button data-debug-step-out></button><button data-debug-stop></button>
      <button data-debug-refresh></button><select data-debug-session></select>
      <span data-debug-state></span><div data-debug-threads></div><div data-debug-stack></div>
      <div data-debug-variables></div><div data-debug-breakpoints></div><div data-debug-output></div>
      <details data-debug-config-editor><form data-debug-config-form></form></details>
    </main>
    <dialog id="debug-approval-dialog"><form><dl data-debug-approval-details></dl>
      <button type="submit" value="deny"></button><button type="submit" value="approve"></button>
    </form></dialog>`;
  const dialog = document.querySelector<HTMLDialogElement>('dialog')!;
  dialog.showModal = () => { dialog.open = true; };
  dialog.close = () => { dialog.open = false; dialog.dispatchEvent(new Event('close')); };
}

function fakeClient(call: (method: string, params?: Record<string, unknown>) => ResponseValue) {
  return { call: vi.fn((request: { method: string; params?: Record<string, unknown> }) => Promise.resolve(call(request.method, request.params))) } as unknown as KorenClient;
}

async function makePanel(call?: (method: string, params?: Record<string, unknown>) => ResponseValue) {
  markup();
  const client = fakeClient(call ?? ((method) => {
    if (method === 'list_debug_configurations') return { kind: 'debug_configurations', value: [configuration] };
    if (method === 'list_debug_sessions') return { kind: 'debug_sessions', value: [] };
    return { kind: 'debug_request_accepted', value: { session_id: 1, request_id: 1 } };
  }));
  const { DebugPanel } = await import('./debug');
  const panel = new DebugPanel(document.querySelector<HTMLElement>('#debug')!, client, vi.fn(async () => {}), vi.fn());
  await vi.waitFor(() => expect(document.querySelector<HTMLSelectElement>('[data-debug-configuration]')!.options).toHaveLength(1));
  return { panel, client };
}

describe('DebugPanel', () => {
  beforeEach(() => vi.resetModules());

  it('requests explicit approval and displays exact adapter and target details', async () => {
    const { client } = await makePanel((method) => {
      if (method === 'list_debug_configurations') return { kind: 'debug_configurations', value: [configuration] };
      if (method === 'list_debug_sessions') return { kind: 'debug_sessions', value: [] };
      if (method === 'request_debug_start') return { kind: 'debug_approval_required', value: {
        approval_id: 9, adapter_name: 'Sample DAP', adapter_command: '/usr/bin/sample-dap', adapter_arguments: ['--stdio'],
        adapter_working_directory: '/workspace', adapter_environment_keys: ['TOKEN'], request: 'launch',
        program: '/workspace/app', process_id: null, arguments: ['--verbose'], working_directory: '/workspace',
        environment_keys: ['MODE'], expires_in_ms: 120000,
      } };
      return { kind: 'debug_request_accepted', value: { session_id: 1, request_id: 1 } };
    });
    document.querySelector<HTMLButtonElement>('[data-debug-start]')!.click();
    await vi.waitFor(() => expect(document.querySelector<HTMLDialogElement>('#debug-approval-dialog')!.open).toBe(true));
    const details = document.querySelector('[data-debug-approval-details]')!.textContent!;
    expect(details).toContain('/usr/bin/sample-dap --stdio');
    expect(details).toContain('TOKEN');
    expect(details).toContain('/workspace/app');
    expect(client.call).toHaveBeenCalledWith({ method: 'request_debug_start', params: { configuration_id: 'sample.launch' } });
    document.querySelector<HTMLButtonElement>('[value="approve"]')!.click();
    await vi.waitFor(() => expect(client.call).toHaveBeenCalledWith({ method: 'approve_debug_start', params: { approval_id: 9 } }));
  });

  it('enables controls for paused sessions and renders adapter output as text', async () => {
    const { panel } = await makePanel();
    panel.handleEvent({ kind: 'debug_event', debug_event: { kind: 'state_changed', session_id: 1, info: session } });
    expect(document.querySelector<HTMLButtonElement>('[data-debug-continue]')!.disabled).toBe(false);
    expect(document.querySelector<HTMLButtonElement>('[data-debug-pause]')!.disabled).toBe(true);
    panel.handleEvent({ kind: 'debug_event', debug_event: { kind: 'output', session_id: 1, category: 'stdout', output: '<img src=x onerror=alert(1)>' } });
    expect(document.querySelector('[data-debug-output] img')).toBeNull();
    expect(document.querySelector('[data-debug-output]')!.textContent).toContain('<img src=x onerror=alert(1)>');
  });

  it('replaces source breakpoints and exposes their verification status', async () => {
    const set = vi.fn();
    const { panel } = await makePanel((method, params) => {
      if (method === 'list_debug_configurations') return { kind: 'debug_configurations', value: [configuration] };
      if (method === 'list_debug_sessions') return { kind: 'debug_sessions', value: [] };
      if (method === 'set_debug_breakpoints') {
        set(params?.breakpoints);
        return { kind: 'debug_breakpoints', value: [{ id: 2, line: 4, column: null, condition: null,
          hit_condition: null, log_message: null, enabled: true, verified: false, actual_line: 5, message: 'pending' }] };
      }
      return { kind: 'debug_request_accepted', value: { session_id: 1, request_id: 1 } };
    });
    await panel.toggleBreakpoint('/workspace/main.c', 4);
    expect(set).toHaveBeenCalledWith([{ line: 4, column: null, condition: null, hit_condition: null, log_message: null, enabled: true }]);
    expect(panel.breakpointState('/workspace/main.c', 4)).toBe('unverified');
  });

  it('selects threads and frames, navigates to source, and expands bounded variables', async () => {
    const navigate = vi.fn(async () => {});
    markup();
    const client = fakeClient((method) => {
      if (method === 'list_debug_configurations') return { kind: 'debug_configurations', value: [configuration] };
      if (method === 'list_debug_sessions') return { kind: 'debug_sessions', value: [session] };
      return { kind: 'debug_request_accepted', value: { session_id: 1, request_id: 2 } };
    });
    const { DebugPanel } = await import('./debug');
    const panel = new DebugPanel(document.querySelector<HTMLElement>('#debug')!, client, navigate, vi.fn());
    await vi.waitFor(() => expect(document.querySelector<HTMLSelectElement>('[data-debug-session]')!.options).toHaveLength(1));
    panel.handleEvent({ kind: 'debug_event', debug_event: { kind: 'response', session_id: 1, request_id: 10, command: 'threads', success: true,
      body: { threads: [{ id: 7, name: 'worker' }] } } });
    panel.handleEvent({ kind: 'debug_event', debug_event: { kind: 'response', session_id: 1, request_id: 11, command: 'stackTrace', success: true,
      body: { stackFrames: [{ id: 42, name: 'run', line: 8, column: 1, source: { name: 'main.c', path: '/workspace/main.c' } }] } } });
    document.querySelector<HTMLButtonElement>('[data-debug-threads] button')!.click();
    document.querySelector<HTMLButtonElement>('[data-debug-stack] button')!.click();
    await vi.waitFor(() => expect(navigate).toHaveBeenCalledWith('/workspace/main.c', 7));
    expect(client.call).toHaveBeenCalledWith({ method: 'debug_inspect', params: expect.objectContaining({ operation: 'stack_trace', thread_id: 7 }) });
    expect(client.call).toHaveBeenCalledWith({ method: 'debug_inspect', params: expect.objectContaining({ operation: 'scopes', frame_id: 42 }) });

    panel.handleEvent({ kind: 'debug_event', debug_event: { kind: 'response', session_id: 1, request_id: 12, command: 'scopes', success: true,
      body: { scopes: [{ name: 'Locals', variablesReference: 11 }] } } });
    document.querySelector<HTMLButtonElement>('[data-debug-variables] button')!.click();
    panel.handleEvent({ kind: 'debug_event', debug_event: { kind: 'response', session_id: 1, request_id: 13, command: 'variables', success: true,
      body: { variables: [{ name: 'point', value: 'Point', variablesReference: 19 }] } } });
    document.querySelector<HTMLButtonElement>('[aria-label="Expand point"]')!.click();
    expect(client.call).toHaveBeenCalledWith({ method: 'debug_inspect', params: expect.objectContaining({ operation: 'variables', reference: {
      session_id: 1, stop_generation: 2, adapter_reference: 19,
    } }) });
  });
});
