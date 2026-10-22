import { translate } from '../i18n';
import { dispatch, errorMessage } from './api';
import type {
  ReviewFile,
  ReviewFileDiff,
  ReviewFilePage,
  ReviewOverview,
  ReviewSelection,
  ReviewSummary,
} from './reviewTypes';

export interface ReviewState {
  selection: ReviewSelection;
  overview: ReviewOverview | null;
  files: ReviewFile[];
  nextCursor: number | null;
  selectedFileId: string | null;
  diff: ReviewFileDiff | null;
  loading: boolean;
  loadingFiles: boolean;
  loadingDiff: boolean;
  error: string | null;
  fileError: string | null;
  stale: boolean;
}

export interface ReviewTransport {
  summary: (selection: ReviewSelection, refresh: boolean) => Promise<ReviewOverview>;
  files: (summary: ReviewSummary, cursor?: number) => Promise<ReviewFilePage>;
  file: (summary: ReviewSummary, fileId: string) => Promise<ReviewFileDiff>;
}

export function reviewSelectionKey(selection: ReviewSelection): string {
  switch (selection.kind) {
    case 'workspace':
      return `workspace:${selection.comparison}`;
    case 'session':
      return 'session';
    case 'turn':
      return `turn:${selection.messageId}`;
  }
}

function initialState(selection: ReviewSelection): ReviewState {
  return {
    selection,
    overview: null,
    files: [],
    nextCursor: null,
    selectedFileId: null,
    diff: null,
    loading: false,
    loadingFiles: false,
    loadingDiff: false,
    error: null,
    fileError: null,
    stale: false,
  };
}

export function reviewTransport(sessionId: string): ReviewTransport {
  return {
    summary: (selection, refresh) =>
      dispatch(refresh ? 'refresh_review' : 'get_review_summary', { sessionId, selection }),
    files: (summary, cursor) =>
      dispatch('list_review_files', {
        sessionId,
        reviewId: summary.reviewId,
        revision: summary.revision,
        ...(cursor === undefined ? {} : { cursor }),
      }),
    file: (summary, fileId) =>
      dispatch('get_review_file', {
        sessionId,
        reviewId: summary.reviewId,
        revision: summary.revision,
        fileId,
      }),
  };
}

/** Session-local state; no source content is written to browser storage. */
export function createReviewController(transport: ReviewTransport) {
  let state = initialState({ kind: 'workspace', comparison: 'net' });
  const listeners = new Set<() => void>();
  let generation = 0;
  let fileRequest = 0;
  let pageRequest = 0;
  let detailsEnabled = false;
  let pending: Promise<void> | undefined;
  let refreshAgain = false;

  function update(changes: Partial<ReviewState>) {
    state = { ...state, ...changes };
    for (const listener of listeners) listener();
  }

  function sameSummary(summary: ReviewSummary) {
    const current = state.overview?.summary;
    return current?.reviewId === summary.reviewId && current.revision === summary.revision;
  }

  async function selectFile(fileId: string) {
    const summary = state.overview?.summary;
    if (!summary || !state.files.some((file) => file.id === fileId)) return;
    if (
      state.selectedFileId === fileId &&
      state.diff?.revision === summary.revision &&
      state.diff.reviewId === summary.reviewId
    )
      return;
    const requested = ++fileRequest;
    const scope = generation;
    update({ selectedFileId: fileId, diff: null, fileError: null, loadingDiff: detailsEnabled });
    if (!detailsEnabled) return;
    try {
      const diff = await transport.file(summary, fileId);
      if (scope !== generation || requested !== fileRequest || !sameSummary(summary)) return;
      if (
        diff.reviewId !== summary.reviewId ||
        diff.revision !== summary.revision ||
        diff.file.id !== fileId
      ) {
        update({ stale: true, fileError: translate('review.revisionChanged') });
        return;
      }
      update({
        diff,
        files: state.files.map((file) => (file.id === fileId ? diff.file : file)),
      });
    } catch (reason) {
      if (scope === generation && requested === fileRequest && sameSummary(summary))
        update({ fileError: errorMessage(reason) });
    } finally {
      if (scope === generation && requested === fileRequest) update({ loadingDiff: false });
    }
  }

  function refresh(force = false): Promise<void> {
    if (pending) {
      refreshAgain ||= force;
      return pending;
    }
    const scope = generation;
    const selection = state.selection;
    update({ loading: true, error: null });
    const request = Promise.resolve().then(async () => {
      try {
        const overview = await transport.summary(selection, force);
        if (scope !== generation) return;
        const summary = overview.summary;
        if (!summary) {
          if (
            state.overview?.summary &&
            (overview.unavailable?.code === 'remote_unavailable' ||
              overview.unavailable?.code === 'inspection_failed')
          ) {
            update({ stale: true, error: overview.unavailable.message });
            return;
          }
          ++fileRequest;
          ++pageRequest;
          update({
            overview,
            files: [],
            nextCursor: null,
            selectedFileId: null,
            diff: null,
            fileError: null,
            loadingDiff: false,
            loadingFiles: false,
            stale: false,
          });
          return;
        }
        if (reviewSelectionKey(summary.selection) !== reviewSelectionKey(selection))
          throw new Error(translate('review.revisionChanged'));
        if (sameSummary(summary)) {
          update({ overview, stale: false });
          return;
        }
        const page = await transport.files(summary);
        if (scope !== generation) return;
        const selectedPath = state.files.find((file) => file.id === state.selectedFileId)?.path;
        const selected = page.files.find((file) => file.path === selectedPath) ?? page.files[0];
        ++fileRequest;
        ++pageRequest;
        update({
          overview,
          files: page.files,
          nextCursor: page.nextCursor,
          selectedFileId: selected?.id ?? null,
          diff: null,
          fileError: null,
          loadingDiff: false,
          loadingFiles: false,
          stale: false,
        });
        if (selected && detailsEnabled) await selectFile(selected.id);
      } catch (reason) {
        if (scope === generation)
          update({ error: errorMessage(reason), stale: Boolean(state.overview?.summary) });
      } finally {
        if (scope === generation) update({ loading: false });
        if (pending === request) {
          pending = undefined;
          if (refreshAgain) {
            refreshAgain = false;
            void refresh(true);
          }
        }
      }
    });
    pending = request;
    return request;
  }

  async function loadMore() {
    const summary = state.overview?.summary;
    const cursor = state.nextCursor;
    if (!summary || cursor === null || state.loadingFiles) return;
    const scope = generation;
    const requested = ++pageRequest;
    update({ loadingFiles: true, error: null });
    try {
      const page = await transport.files(summary, cursor);
      if (scope !== generation || requested !== pageRequest || !sameSummary(summary)) return;
      const known = new Set(state.files.map((file) => file.id));
      update({
        files: [...state.files, ...page.files.filter((file) => !known.has(file.id))],
        nextCursor: page.nextCursor,
      });
    } catch (reason) {
      if (scope === generation && requested === pageRequest && sameSummary(summary))
        update({ error: errorMessage(reason), stale: true });
    } finally {
      if (scope === generation && requested === pageRequest) update({ loadingFiles: false });
    }
  }

  return {
    getSnapshot: () => state,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    refresh,
    selectFile,
    loadMore,
    select(selection: ReviewSelection) {
      if (reviewSelectionKey(selection) === reviewSelectionKey(state.selection)) return;
      ++generation;
      ++fileRequest;
      ++pageRequest;
      pending = undefined;
      refreshAgain = false;
      const turns = state.overview?.turns ?? [];
      state = { ...initialState(selection), overview: { summary: null, unavailable: null, turns } };
      update({});
      void refresh();
    },
    enableDetails(enabled: boolean) {
      detailsEnabled = enabled;
      if (enabled && state.selectedFileId) void selectFile(state.selectedFileId);
      if (!enabled) {
        ++fileRequest;
        update({ loadingDiff: false });
      }
    },
    cancel() {
      ++generation;
      ++fileRequest;
      ++pageRequest;
      pending = undefined;
      refreshAgain = false;
      update({ loading: false, loadingDiff: false, loadingFiles: false });
    },
  };
}

export type ReviewController = ReturnType<typeof createReviewController>;
