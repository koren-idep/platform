import { it } from 'vitest';
import { DebugPanel } from './debug';
import type { KorenClient } from '../services/client';

it('measures production Debug panel output event processing', async () => {
  document.body.innerHTML = `<main id="debug">
    <select data-debug-configuration></select><button data-debug-start></button><button data-debug-continue></button>
    <button data-debug-pause></button><button data-debug-step-over></button><button data-debug-step-into></button>
    <button data-debug-step-out></button><button data-debug-stop></button><button data-debug-refresh></button>
    <select data-debug-session></select><span data-debug-state></span><div data-debug-threads></div>
    <div data-debug-stack></div><div data-debug-variables></div><div data-debug-breakpoints></div>
    <div data-debug-output></div><details data-debug-config-editor><form data-debug-config-form></form></details>
  </main><dialog id="debug-approval-dialog"><form><dl data-debug-approval-details></dl></form></dialog>`;
  const client = { call: async (request: { method: string }) => request.method === 'list_debug_configurations'
    ? { kind: 'debug_configurations', value: [] }
    : { kind: 'debug_sessions', value: [] } } as unknown as KorenClient;
  const panel = new DebugPanel(document.querySelector<HTMLElement>('#debug')!, client, async () => {}, () => {});
  await Promise.resolve();
  const start = performance.now();
  for (let index = 0; index < 1000; index++) {
    panel.handleEvent({ kind: 'debug_event', debug_event: {
      kind: 'output', session_id: 1, category: 'stdout', output: `line-${index}`,
    } });
  }
  const elapsed = performance.now() - start;
  console.log(`Debug panel output dispatch: 1,000 events in ${elapsed.toFixed(2)} ms (jsdom DOM updates; excludes browser paint and adapter time)`);
  if (document.querySelectorAll('[data-debug-output] > div').length !== 1000) throw new Error('Debug output cap/render mismatch');
});
