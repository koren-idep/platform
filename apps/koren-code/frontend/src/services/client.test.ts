import { describe, expect, it, vi } from 'vitest';
import { createDesktopTransport, KorenClient, KorenError, type Transport } from './client';

describe('typed protocol client', () => {
  it('correlates concurrent responses by unique request ids', async () => {
    const transport: Transport = { request: async (body) => ({ version: 2, request_id: body.request_id, result: { Ok: { kind: 'pong' } } }), events: async () => [] };
    const client = new KorenClient(transport);
    const replies = await Promise.all([client.call({ method: 'ping' }), client.call({ method: 'ping' })]);
    expect(replies).toEqual([{ kind: 'pong' }, { kind: 'pong' }]);
  });
  it('surfaces protocol errors and rejects miscorrelated replies', async () => {
    const transport: Transport = { request: async () => ({ version: 2, request_id: 1, result: { Err: { code: 'conflict', message: 'changed' } } }), events: async () => [] };
    await expect(new KorenClient(transport).call({ method: 'ping' })).rejects.toMatchObject({ code: 'conflict' });
    const mismatch: Transport = { ...transport, request: async () => ({ version: 2, request_id: 99, result: { Ok: { kind: 'pong' } } }) };
    await expect(new KorenClient(mismatch).call({ method: 'ping' })).rejects.toBeInstanceOf(KorenError);
  });
  it('stops future requests after closing the client', async () => {
    const transport: Transport = { request: vi.fn(), events: async () => [] };
    const client = new KorenClient(transport); client.close();
    await expect(client.call({ method: 'ping' })).rejects.toMatchObject({ code: 'not_open' });
    expect(transport.request).not.toHaveBeenCalled();
  });

  it('sends typed desktop requests through IPC and correlates out-of-order replies', async () => {
    const previousIpc = window.ipc;
    const messages: Array<{ version: number; request_id: number }> = [];
    window.ipc = { postMessage: (message) => messages.push(JSON.parse(message) as { version: number; request_id: number }) };
    try {
      const transport = createDesktopTransport();
      const first = transport.request({ version: 2, request_id: 7, method: { method: 'ping' } });
      const second = transport.request({ version: 2, request_id: 8, method: { method: 'ping' } });
      expect(messages.map(({ request_id }) => request_id)).toEqual([7, 8]);
      window.dispatchEvent(new CustomEvent('koren-platform-response', { detail: { version: 2, request_id: 8, result: { Ok: { kind: 'pong' } } } }));
      window.dispatchEvent(new CustomEvent('koren-platform-response', { detail: { version: 2, request_id: 7, result: { Ok: { kind: 'pong' } } } }));
      expect((await Promise.all([first, second])).map(({ request_id }) => request_id)).toEqual([7, 8]);
    } finally {
      window.ipc = previousIpc;
    }
  });
});
