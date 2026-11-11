import { PROTOCOL_VERSION, type PlatformEvent, type ProtocolError, type RequestEnvelope, type RequestMethod, type ResponseEnvelope, type ResponseValue } from './protocol';

export interface Transport {
  request(body: RequestEnvelope): Promise<ResponseEnvelope>;
  events(): Promise<PlatformEvent[]>;
}

interface DesktopIpc { postMessage(message: string): void }

declare global {
  interface Window { ipc?: DesktopIpc }
}

export class KorenError extends Error {
  constructor(readonly code: string, message: string) { super(message); }
}

export class KorenClient {
  private nextId = 1;
  private closed = false;
  constructor(private readonly transport: Transport) {}

  async call(method: RequestMethod): Promise<ResponseValue> {
    if (this.closed) throw new KorenError('not_open', 'The desktop bridge is closed.');
    const requestId = this.nextId++;
    const envelope: RequestEnvelope = { version: PROTOCOL_VERSION, request_id: requestId, method };
    const response = await this.transport.request(envelope);
    if (response.version !== PROTOCOL_VERSION) throw new KorenError('unsupported_version', `Bridge protocol ${response.version} is not supported.`);
    if (response.request_id !== requestId) throw new KorenError('request_mismatch', 'The bridge returned a response for another request.');
    if ('Err' in response.result) {
      const error: ProtocolError = response.result.Err;
      throw new KorenError(error.code, error.message);
    }
    return response.result.Ok;
  }

  async *subscribeEvents(signal?: AbortSignal): AsyncGenerator<PlatformEvent[]> {
    while (!this.closed && !signal?.aborted) {
      yield await this.transport.events();
    }
  }

  close(): void { this.closed = true; }
}

export function createDesktopTransport(base = ''): Transport {
  const pending = new Map<number, (response: ResponseEnvelope) => void>();
  window.addEventListener('koren-platform-response', (event) => {
    const response = (event as CustomEvent<ResponseEnvelope>).detail;
    pending.get(response.request_id)?.(response);
    pending.delete(response.request_id);
  });
  return {
    async request(body) {
      if (!window.ipc) throw new KorenError('ipc_unavailable', 'The desktop bridge is unavailable.');
      return await new Promise<ResponseEnvelope>((resolve) => {
        pending.set(body.request_id, resolve);
        window.ipc!.postMessage(JSON.stringify(body));
      });
    },
    async events() {
      const response = await fetch(`${base}/events`, { cache: 'no-store' });
      return await response.json() as PlatformEvent[];
    },
  };
}
