import { translate as t } from '../i18n';
import type { Commands, RuntimeEvent } from './types';

export function isDesktop(): boolean {
  return '__TAURI_INTERNALS__' in window;
}

export async function dispatch<C extends keyof Commands>(
  command: C,
  payload: Commands[C]['payload'],
): Promise<Commands[C]['result']> {
  const request = { command, payload };
  if (isDesktop()) {
    const { invoke } = await import('@tauri-apps/api/core');
    return invoke<Commands[C]['result']>('dispatch', { request }).catch((error: unknown) => {
      throw commandError(error);
    });
  }
  const response = await fetch('/api/dispatch', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(request),
  }).catch(() => {
    throw new Error(t('error.serviceConnection'));
  });
  let result: unknown;
  try {
    result = await response.json();
  } catch {
    throw new Error(
      response.ok
        ? t('error.serviceData')
        : t('error.serviceUnavailable', { status: response.status }),
    );
  }
  if (!response.ok || (typeof result === 'object' && result !== null && 'error' in result)) {
    throw commandError(result, t('error.requestFailed', { status: response.status }));
  }
  return result as Commands[C]['result'];
}

export async function subscribe(
  onEvent: (event: RuntimeEvent) => void,
  onConnection: (connected: boolean) => void,
): Promise<() => void> {
  if (isDesktop()) {
    const { listen } = await import('@tauri-apps/api/event');
    const unlisten = await listen<RuntimeEvent>('runtime-event', ({ payload }) => onEvent(payload));
    onConnection(true);
    return unlisten;
  }
  const source = new EventSource('/api/events');
  source.addEventListener('open', () => onConnection(true));
  source.addEventListener('error', () => onConnection(false));
  source.addEventListener('message', (event: MessageEvent<string>) => {
    try {
      const payload: unknown = JSON.parse(event.data);
      if (
        typeof payload === 'object' &&
        payload !== null &&
        'sessionId' in payload &&
        'kind' in payload &&
        'data' in payload &&
        [
          'delta',
          'message',
          'permission',
          'status',
          'preparation',
          'error',
          'title',
          'title_error',
          'context_usage',
          'review_updated',
          'review_invalidated',
        ].includes(String(payload.kind))
      ) {
        onEvent(payload as RuntimeEvent);
      }
    } catch {
      // A malformed event must not disable a healthy event connection.
      return;
    }
  });
  return () => source.close();
}

export type ErrorCode = 'invalid_input' | 'not_found' | 'conflict' | 'unavailable' | 'internal';

export class CommandError extends Error {
  constructor(
    message: string,
    public readonly code: ErrorCode,
  ) {
    super(message);
    this.name = 'CommandError';
  }
}

function isErrorCode(value: unknown): value is ErrorCode {
  return ['invalid_input', 'not_found', 'conflict', 'unavailable', 'internal'].some(
    (code) => code === value,
  );
}

export function commandError(value: unknown, fallback?: string): Error {
  if (value instanceof Error) return value;
  if (
    typeof value === 'object' &&
    value !== null &&
    'error' in value &&
    'code' in value &&
    typeof value.error === 'string' &&
    isErrorCode(value.code)
  ) {
    return new CommandError(value.error, value.code);
  }
  return new Error(fallback ?? String(value));
}

export function errorMessage(error: unknown): string {
  return commandError(error).message;
}
