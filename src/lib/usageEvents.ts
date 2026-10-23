import type { RuntimeEvent } from './types';

const listeners = new Set<() => void>();
let captureFailed = false;

export function receiveUsageEvent(event: RuntimeEvent) {
  if (event.kind !== 'usage_updated' && event.kind !== 'usage_error') return;
  // Keep a failure visible: a later successful write cannot recover the missing event.
  if (event.kind === 'usage_error') captureFailed = true;
  for (const listener of listeners) listener();
}

export function usageCaptureFailed() {
  return captureFailed;
}

export function subscribeUsageUpdates(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
