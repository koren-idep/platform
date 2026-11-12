import type { KorenClient } from '../services/client';
import type {
  DebugApprovalSummary, DebugBreakpointSpec, DebugConfigurationSummary,
  DebugSessionInfo, DebugSourceBreakpoint, DebugVariableReference, PlatformEvent,
} from '../services/protocol';

type DapBody = Record<string, unknown>;
type Thread = { id: number; name: string };
type StackFrame = { id: number; name: string; line: number; source?: { name?: string; path?: string } };
type Scope = { name: string; variablesReference: number; expensive?: boolean };
type Variable = { name: string; value: string; type?: string; variablesReference: number };

export class DebugPanel {
  private readonly configurations = new Map<string, DebugConfigurationSummary>();
  private readonly sessions = new Map<number, DebugSessionInfo>();
  private readonly breakpoints = new Map<string, DebugSourceBreakpoint[]>();
  private readonly threads = new Map<number, Thread>();
  private readonly frames = new Map<number, StackFrame>();
  private readonly approvalDialog: HTMLDialogElement;
  private readonly approvalDetails: HTMLElement;
  private readonly output: HTMLElement;
  private activeSessionId: number | null = null;
  private selectedThreadId: number | null = null;
  private activeReference: DebugVariableReference | null = null;

  constructor(
    private readonly root: HTMLElement,
    private readonly client: KorenClient,
    private readonly navigate: (path: string, line: number) => Promise<void>,
    private readonly reportError: (message: string) => void,
    private readonly breakpointsChanged: () => void = () => {},
  ) {
    this.approvalDialog = document.querySelector<HTMLDialogElement>('#debug-approval-dialog')!;
    this.approvalDetails = this.approvalDialog.querySelector<HTMLElement>('[data-debug-approval-details]')!;
    this.output = root.querySelector<HTMLElement>('[data-debug-output]')!;
    this.bind();
    void this.refresh();
  }

  async refresh(): Promise<void> {
    try {
      const [configurations, sessions] = await Promise.all([
        this.client.call({ method: 'list_debug_configurations' }),
        this.client.call({ method: 'list_debug_sessions' }),
      ]);
      if (configurations.kind === 'debug_configurations') {
        this.configurations.clear();
        for (const configuration of configurations.value) this.configurations.set(configuration.id, configuration);
      }
      if (sessions.kind === 'debug_sessions') {
        this.sessions.clear();
        for (const session of sessions.value) this.sessions.set(session.session_id, session);
        if (!this.sessions.has(this.activeSessionId ?? -1)) this.activeSessionId = this.sessions.keys().next().value ?? null;
      }
      this.render();
    } catch (error) { this.reportError(error instanceof Error ? error.message : String(error)); }
  }

  handleEvent(event: PlatformEvent): void {
    if (event.kind !== 'debug_event' || !event.debug_event) return;
    const debug = event.debug_event;
    if (debug.kind === 'state_changed' && debug.info) {
      this.sessions.set(debug.session_id, debug.info);
      this.activeSessionId = debug.session_id;
      if (debug.info.state === 'paused' && debug.info.stopped_thread_id !== null) {
        this.selectedThreadId = debug.info.stopped_thread_id;
        void this.inspect('threads');
        void this.inspect('stack_trace');
      }
      if (debug.info.state === 'running' || debug.info.state === 'terminated' || debug.info.state === 'failed') {
        this.activeReference = null;
        this.frames.clear();
      }
    } else if (debug.kind === 'response' && debug.body) {
      this.consumeResponse(debug.command ?? '', debug.body as DapBody);
    } else if (debug.kind === 'output') {
      this.appendOutput(`[${debug.category ?? 'console'}] ${debug.output ?? ''}`);
    } else if (debug.kind === 'adapter_diagnostic') {
      this.appendOutput(`[adapter] ${debug.message ?? ''}`);
    } else if (debug.kind === 'error') {
      this.appendOutput(`[debug error] ${debug.message ?? 'Unknown adapter error'}`);
    } else if (debug.kind === 'breakpoint_changed' && debug.breakpoint) {
      const items = this.breakpoints.get(debug.source ?? '') ?? [];
      const changed = debug.breakpoint;
      const index = items.findIndex((item) => item.id === changed.id);
      if (index >= 0) items[index] = changed; else items.push(changed);
      this.breakpoints.set(debug.source ?? '', items);
      this.breakpointsChanged();
    }
    this.render();
  }

  async toggleBreakpoint(source: string, zeroBasedLine: number): Promise<void> {
    const items = this.breakpoints.get(source) ?? [];
    const index = items.findIndex((item) => item.line === zeroBasedLine);
    const next: DebugBreakpointSpec[] = items
      .filter((_, itemIndex) => itemIndex !== index)
      .map(({ line, column, condition, hit_condition, log_message, enabled }) => ({ line, column, condition, hit_condition, log_message, enabled }));
    if (index < 0) next.push({ line: zeroBasedLine, column: null, condition: null, hit_condition: null, log_message: null, enabled: true });
    try {
      const response = await this.client.call({ method: 'set_debug_breakpoints', params: { source, breakpoints: next } });
      if (response.kind === 'debug_breakpoints') this.breakpoints.set(source, response.value);
      this.breakpointsChanged();
      this.render();
    } catch (error) { this.reportError(error instanceof Error ? error.message : String(error)); }
  }

  breakpointState(source: string, zeroBasedLine: number): 'active' | 'disabled' | 'unverified' | null {
    const item = (this.breakpoints.get(source) ?? []).find((breakpoint) => breakpoint.line === zeroBasedLine);
    if (!item) return null;
    if (!item.enabled) return 'disabled';
    if (item.verified === false) return 'unverified';
    return 'active';
  }

  executionLine(source: string): number | null {
    for (const frame of this.frames.values()) if (frame.source?.path === source) return Math.max(0, frame.line - 1);
    return null;
  }

  private bind(): void {
    this.root.querySelector<HTMLButtonElement>('[data-debug-refresh]')!.addEventListener('click', () => void this.refresh());
    this.root.querySelector<HTMLButtonElement>('[data-debug-start]')!.addEventListener('click', () => void this.start());
    this.root.querySelector<HTMLButtonElement>('[data-debug-continue]')!.addEventListener('click', () => void this.action('continue'));
    this.root.querySelector<HTMLButtonElement>('[data-debug-pause]')!.addEventListener('click', () => void this.action('pause'));
    this.root.querySelector<HTMLButtonElement>('[data-debug-step-over]')!.addEventListener('click', () => void this.action('step_over'));
    this.root.querySelector<HTMLButtonElement>('[data-debug-step-into]')!.addEventListener('click', () => void this.action('step_into'));
    this.root.querySelector<HTMLButtonElement>('[data-debug-step-out]')!.addEventListener('click', () => void this.action('step_out'));
    this.root.querySelector<HTMLButtonElement>('[data-debug-stop]')!.addEventListener('click', () => void this.stop());
    this.root.querySelector<HTMLSelectElement>('[data-debug-session]')!.addEventListener('change', (event) => {
      const id = Number((event.currentTarget as HTMLSelectElement).value);
      this.activeSessionId = Number.isSafeInteger(id) ? id : null;
      this.threads.clear(); this.frames.clear(); this.activeReference = null;
      this.render();
      if (this.session()?.state === 'paused') { void this.inspect('threads'); void this.inspect('stack_trace'); }
    });
    this.root.querySelector<HTMLFormElement>('[data-debug-config-form]')!.addEventListener('submit', (event) => {
      event.preventDefault(); void this.registerConfiguration(new FormData(event.currentTarget as HTMLFormElement));
    });
    this.approvalDialog.querySelector('form')!.addEventListener('submit', (event) => {
      event.preventDefault();
      const action = (event.submitter as HTMLButtonElement | null)?.value;
      const approvalId = Number(this.approvalDialog.dataset.approvalId);
      this.approvalDialog.close();
      void this.consumeApproval(approvalId, action === 'approve');
    });
  }

  private async registerConfiguration(form: FormData): Promise<void> {
    const adapterId = String(form.get('adapter_id') ?? '').trim();
    const profileId = String(form.get('configuration_id') ?? '').trim();
    const adapterCommand = String(form.get('adapter_command') ?? '').trim();
    const adapterDirectory = String(form.get('adapter_directory') ?? '').trim();
    const program = String(form.get('program') ?? '').trim();
    const workingDirectory = String(form.get('working_directory') ?? '').trim();
    if (!adapterId || !profileId || !adapterCommand || !adapterDirectory || !program || !workingDirectory) {
      this.reportError('Fill in the adapter, program, and working directories before saving the debug profile.'); return;
    }
    try {
      const response = await this.client.call({ method: 'register_debug_configuration', params: {
        adapter: { id: adapterId, name: String(form.get('adapter_name') || adapterId), command: adapterCommand,
          arguments: words(String(form.get('adapter_arguments') ?? '')), working_directory: adapterDirectory, environment: {} },
        configuration: { id: profileId, name: String(form.get('configuration_name') || profileId), adapter_id: adapterId,
          request: 'launch', program, process_id: null, arguments: words(String(form.get('arguments') ?? '')),
          working_directory: workingDirectory, environment: {}, adapter_settings: {}, stop_on_entry: false },
      } });
      if (response.kind === 'debug_configurations') {
        this.configurations.clear(); for (const item of response.value) this.configurations.set(item.id, item);
        this.root.querySelector<HTMLDetailsElement>('[data-debug-config-editor]')!.open = false;
        this.render();
      }
    } catch (error) { this.reportError(error instanceof Error ? error.message : String(error)); }
  }

  private async start(): Promise<void> {
    const select = this.root.querySelector<HTMLSelectElement>('[data-debug-configuration]')!;
    if (!select.value) { this.root.querySelector<HTMLDetailsElement>('[data-debug-config-editor]')!.open = true; return; }
    try {
      const response = await this.client.call({ method: 'request_debug_start', params: { configuration_id: select.value } });
      if (response.kind !== 'debug_approval_required') throw new Error('The debug approval request was not returned.');
      this.showApproval(response.value);
    } catch (error) { this.reportError(error instanceof Error ? error.message : String(error)); }
  }

  private showApproval(approval: DebugApprovalSummary): void {
    this.approvalDialog.dataset.approvalId = String(approval.approval_id);
    this.approvalDetails.replaceChildren();
    const items: Array<[string, string]> = [
      ['Adapter', `${approval.adapter_command} ${approval.adapter_arguments.join(' ')}`],
      ['Adapter folder', approval.adapter_working_directory],
      ['Adapter environment keys', approval.adapter_environment_keys.join(', ') || 'none'],
      ['Target action', approval.request],
      ['Program', approval.program ?? `process ${approval.process_id ?? ''}`],
      ['Target arguments', approval.arguments.join(' ') || 'none'],
      ['Target folder', approval.working_directory],
      ['Target environment keys', approval.environment_keys.join(', ') || 'none'],
    ];
    for (const [term, description] of items) {
      const dt = document.createElement('dt'); dt.textContent = term;
      const dd = document.createElement('dd'); dd.textContent = description;
      this.approvalDetails.append(dt, dd);
    }
    this.approvalDialog.showModal();
  }

  private async consumeApproval(approvalId: number, approve: boolean): Promise<void> {
    try {
      const response = await this.client.call({ method: approve ? 'approve_debug_start' : 'deny_debug_start', params: { approval_id: approvalId } });
      if (response.kind === 'debug_session_started') {
        this.sessions.set(response.value.session_id, response.value); this.activeSessionId = response.value.session_id;
        this.appendOutput(`Session ${response.value.session_id}: starting ${response.value.configuration_id}`);
        this.render();
      }
    } catch (error) { this.reportError(error instanceof Error ? error.message : String(error)); }
  }

  private async action(action: string): Promise<void> {
    const session = this.session(); if (!session) return;
    try {
      await this.client.call({ method: 'debug_action', params: { session_id: session.session_id, action, thread_id: this.selectedThreadId ?? session.stopped_thread_id } });
    } catch (error) { this.reportError(error instanceof Error ? error.message : String(error)); }
  }

  private async stop(): Promise<void> {
    const session = this.session(); if (!session) return;
    try { await this.client.call({ method: 'stop_debug_session', params: { session_id: session.session_id } }); }
    catch (error) { this.reportError(error instanceof Error ? error.message : String(error)); }
  }

  private async inspect(operation: string, frameId?: number, reference?: DebugVariableReference, start = 0): Promise<void> {
    const session = this.session(); if (!session) return;
    try {
      await this.client.call({ method: 'debug_inspect', params: {
        session_id: session.session_id, operation, thread_id: this.selectedThreadId ?? session.stopped_thread_id,
        frame_id: frameId ?? null, reference: reference ?? null, start, count: 100,
      } });
    } catch (error) { this.reportError(error instanceof Error ? error.message : String(error)); }
  }

  private consumeResponse(command: string, body: DapBody): void {
    if (command === 'threads' && Array.isArray(body.threads)) {
      this.threads.clear();
      for (const thread of body.threads as Thread[]) this.threads.set(thread.id, thread);
    } else if (command === 'stackTrace' && Array.isArray(body.stackFrames)) {
      this.frames.clear();
      for (const frame of body.stackFrames as StackFrame[]) this.frames.set(frame.id, frame);
    } else if (command === 'scopes' && Array.isArray(body.scopes)) {
      this.renderScopes(body.scopes as Scope[]);
    } else if (command === 'variables' && Array.isArray(body.variables)) {
      this.renderVariables(body.variables as Variable[]);
    }
  }

  private renderScopes(scopes: Scope[]): void {
    const host = this.root.querySelector<HTMLElement>('[data-debug-variables]')!;
    host.replaceChildren();
    for (const scope of scopes) {
      const button = document.createElement('button'); button.type = 'button'; button.textContent = scope.name;
      button.addEventListener('click', () => {
        const session = this.session(); if (!session) return;
        this.activeReference = { session_id: session.session_id, stop_generation: session.stop_generation, adapter_reference: scope.variablesReference };
        void this.inspect('variables', undefined, this.activeReference);
      });
      host.append(button);
    }
  }

  private renderVariables(variables: Variable[]): void {
    const host = this.root.querySelector<HTMLElement>('[data-debug-variables]')!;
    const list = document.createElement('ul'); list.className = 'debug-variable-list';
    for (const variable of variables) {
      const row = document.createElement('li');
      if (variable.variablesReference > 0) {
        const expand = document.createElement('button'); expand.type = 'button'; expand.textContent = '›';
        expand.setAttribute('aria-label', `Expand ${variable.name}`);
        expand.addEventListener('click', () => {
          const session = this.session(); if (!session) return;
          const reference = { session_id: session.session_id, stop_generation: session.stop_generation, adapter_reference: variable.variablesReference };
          void this.inspect('variables', undefined, reference);
        });
        row.append(expand);
      }
      const name = document.createElement('span'); name.className = 'debug-variable-name'; name.textContent = variable.name;
      const value = document.createElement('span'); value.className = 'debug-variable-value'; value.textContent = ` = ${variable.value}`;
      row.append(name, value); list.append(row);
    }
    host.replaceChildren(list);
  }

  private render(): void {
    const configurationSelect = this.root.querySelector<HTMLSelectElement>('[data-debug-configuration]')!;
    const oldConfiguration = configurationSelect.value;
    configurationSelect.replaceChildren();
    for (const configuration of this.configurations.values()) {
      const option = document.createElement('option'); option.value = configuration.id;
      option.textContent = `${configuration.name} · ${configuration.adapter_name}`; configurationSelect.append(option);
    }
    if (this.configurations.has(oldConfiguration)) configurationSelect.value = oldConfiguration;
    const sessionSelect = this.root.querySelector<HTMLSelectElement>('[data-debug-session]')!;
    sessionSelect.replaceChildren();
    for (const session of this.sessions.values()) {
      const option = document.createElement('option'); option.value = String(session.session_id);
      option.textContent = `${session.configuration_id} · ${session.state}`; sessionSelect.append(option);
    }
    sessionSelect.value = this.activeSessionId === null ? '' : String(this.activeSessionId);
    const session = this.session();
    const paused = session?.state === 'paused'; const running = session?.state === 'running';
    setDisabled(this.root, 'continue', !paused); setDisabled(this.root, 'step-over', !paused);
    setDisabled(this.root, 'step-into', !paused); setDisabled(this.root, 'step-out', !paused);
    setDisabled(this.root, 'pause', !running); setDisabled(this.root, 'stop', !session || ['terminated', 'failed'].includes(session.state));
    setDisabled(this.root, 'start', this.configurations.size === 0);
    this.root.querySelector<HTMLElement>('[data-debug-state]')!.textContent = session ? `Session ${session.session_id} · ${session.state}` : 'No active debug session';
    const threadsHost = this.root.querySelector<HTMLElement>('[data-debug-threads]')!; threadsHost.replaceChildren();
    for (const thread of this.threads.values()) {
      const button = document.createElement('button'); button.type = 'button'; button.textContent = thread.name;
      button.setAttribute('aria-pressed', String(thread.id === this.selectedThreadId));
      button.addEventListener('click', () => { this.selectedThreadId = thread.id; void this.inspect('stack_trace'); this.render(); });
      threadsHost.append(button);
    }
    const stackHost = this.root.querySelector<HTMLElement>('[data-debug-stack]')!; stackHost.replaceChildren();
    for (const frame of this.frames.values()) {
      const button = document.createElement('button'); button.type = 'button';
      button.textContent = `${frame.name} · ${frame.source?.name ?? ''}:${frame.line}`;
      button.addEventListener('click', () => {
        if (frame.source?.path) void this.navigate(frame.source.path, Math.max(0, frame.line - 1));
        void this.inspect('scopes', frame.id);
      });
      stackHost.append(button);
    }
    this.renderBreakpoints();
  }

  private renderBreakpoints(): void {
    const host = this.root.querySelector<HTMLElement>('[data-debug-breakpoints]')!; host.replaceChildren();
    for (const [source, breakpoints] of this.breakpoints) for (const breakpoint of breakpoints) {
      const row = document.createElement('button'); row.type = 'button';
      row.textContent = `${breakpoint.verified === false ? '○' : '●'} ${source}:${(breakpoint.actual_line ?? breakpoint.line) + 1}${breakpoint.message ? ` · ${breakpoint.message}` : ''}`;
      row.addEventListener('click', () => void this.navigate(source, breakpoint.line)); host.append(row);
    }
  }

  private appendOutput(text: string): void {
    const line = document.createElement('div'); line.textContent = text; this.output.append(line);
    while (this.output.childElementCount > 1000) this.output.firstElementChild?.remove();
    this.output.scrollTop = this.output.scrollHeight;
  }

  private session(): DebugSessionInfo | undefined {
    return this.activeSessionId === null ? undefined : this.sessions.get(this.activeSessionId);
  }
}

function setDisabled(root: HTMLElement, action: string, disabled: boolean): void {
  root.querySelector<HTMLButtonElement>(`[data-debug-${action}]`)!.disabled = disabled;
}

function words(input: string): string[] {
  return input.trim() ? input.trim().split(/\s+/u) : [];
}
