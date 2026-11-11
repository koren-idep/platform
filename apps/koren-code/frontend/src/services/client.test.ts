import { describe, expect, it, vi } from 'vitest';
import { KorenClient, KorenError, type Transport } from './client';

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
});
