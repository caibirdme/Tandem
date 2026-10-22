import { useEffect, useMemo, useSyncExternalStore } from 'react';

import { subscribeReviewInvalidation } from '../lib/reviewEvents';
import { createReviewController, reviewTransport } from '../lib/reviews';

export function useReview(sessionId: string | undefined, visible: boolean, connected: boolean) {
  const controller = useMemo(
    () => createReviewController(reviewTransport(sessionId ?? '')),
    [sessionId],
  );
  const state = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
    controller.getSnapshot,
  );
  useEffect(() => {
    if (!sessionId) return;
    void controller.refresh();
    return () => controller.cancel();
  }, [controller, sessionId, connected]);
  useEffect(() => {
    controller.enableDetails(visible);
    return () => controller.enableDetails(false);
  }, [controller, visible]);
  useEffect(() => {
    if (!sessionId) return;
    let scheduled: ReturnType<typeof setTimeout> | undefined;
    function refreshSoon() {
      if (scheduled) return;
      scheduled = setTimeout(() => {
        scheduled = undefined;
        void controller.refresh(true);
      }, 500);
    }
    const unsubscribe = subscribeReviewInvalidation((changedSession) => {
      if (changedSession === sessionId) refreshSoon();
    });
    window.addEventListener('focus', refreshSoon);
    const interval = visible
      ? setInterval(() => {
          if (!document.hidden) void controller.refresh();
        }, 10000)
      : undefined;
    return () => {
      unsubscribe();
      window.removeEventListener('focus', refreshSoon);
      if (scheduled) clearTimeout(scheduled);
      if (interval) clearInterval(interval);
    };
  }, [controller, sessionId, visible]);
  return { state, controller };
}
