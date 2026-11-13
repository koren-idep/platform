export type WindowAction = 'minimize' | 'toggle_maximize' | 'close' | 'drag';

export function windowActionMessage(action: WindowAction): string {
  return JSON.stringify({ type: 'window_action', action });
}
