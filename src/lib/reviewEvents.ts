import type { RuntimeEvent } from './types';

const listeners = new Set<(sessionId: string) => void>();

export function subscribeReviewInvalidation(listener: (sessionId: string) => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The existing application event stream also invalidates read-only review data. */
export function receiveReviewEvent(event: RuntimeEvent) {
  if (
    event.kind === 'review_updated' ||
    event.kind === 'review_invalidated' ||
    event.kind === 'status' ||
    event.kind === 'error' ||
    (event.kind === 'message' &&
      event.data.role === 'activity' &&
      event.data.activity?.status !== 'running')
  ) {
    for (const listener of listeners) listener(event.sessionId);
  }
}
