import { expect, it, vi } from 'vitest';

import { receiveUsageEvent, subscribeUsageUpdates, usageCaptureFailed } from './usageEvents';

it('refreshes on usage events only, retains capture failures and unsubscribes cleanly', () => {
  const listener = vi.fn();
  const unsubscribe = subscribeUsageUpdates(listener);
  expect(usageCaptureFailed()).toBe(false);
  receiveUsageEvent({ sessionId: 's', kind: 'status', data: { status: 'idle' } });
  expect(listener).not.toHaveBeenCalled();
  receiveUsageEvent({ sessionId: 's', kind: 'usage_updated', data: {} });
  expect(listener).toHaveBeenCalledOnce();
  receiveUsageEvent({ sessionId: 's', kind: 'usage_error', data: { message: 'Storage failed' } });
  expect(usageCaptureFailed()).toBe(true);
  receiveUsageEvent({ sessionId: 's', kind: 'usage_updated', data: {} });
  expect(usageCaptureFailed()).toBe(true);
  expect(listener).toHaveBeenCalledTimes(3);
  unsubscribe();
  receiveUsageEvent({ sessionId: 's', kind: 'usage_updated', data: {} });
  expect(listener).toHaveBeenCalledTimes(3);
});
