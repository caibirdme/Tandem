import { describe, expect, it, vi } from 'vitest';

import { createReviewController, reviewSelectionKey } from './reviews';
import type { ReviewTransport } from './reviews';
import type {
  ReviewFile,
  ReviewFileDiff,
  ReviewFilePage,
  ReviewOverview,
  ReviewSelection,
  ReviewSummary,
} from './reviewTypes';

const fileA: ReviewFile = {
  id: 'a',
  path: 'src/a.ts',
  oldPath: null,
  oldMode: '100644',
  newMode: '100644',
  status: 'modified',
  indexStatus: '.',
  worktreeStatus: 'M',
  contentKind: 'text',
  additions: 1,
  deletions: 1,
};
const fileB: ReviewFile = { ...fileA, id: 'b', path: 'src/b.ts' };
function summary(
  revision = 'one',
  selection: ReviewSelection = { kind: 'workspace', comparison: 'net' },
): ReviewSummary {
  return {
    reviewId: `review-${revision}`,
    revision,
    selection,
    workspacePath: '/project',
    repositoryPath: '/project',
    capturedAt: 1000,
    filesTotal: 2,
    additions: 2,
    deletions: 2,
    complete: true,
    excludedFiles: 0,
  };
}
function overview(value = summary()): ReviewOverview {
  return { summary: value, unavailable: null, turns: [] };
}
function diff(file: ReviewFile, value = summary()): ReviewFileDiff {
  return { reviewId: value.reviewId, revision: value.revision, file, patch: 'patch', hunks: [] };
}
function deferred<T>() {
  let resolve: ((value: T) => void) | undefined;
  const promise = new Promise<T>((accept) => {
    resolve = accept;
  });
  return { promise, resolve: (value: T) => resolve?.(value) };
}
function setup(overrides: Partial<ReviewTransport> = {}) {
  const transport = {
    summary: vi.fn<ReviewTransport['summary']>().mockResolvedValue(overview()),
    files: vi
      .fn<ReviewTransport['files']>()
      .mockResolvedValue({ files: [fileA, fileB], nextCursor: null }),
    file: vi
      .fn<ReviewTransport['file']>()
      .mockImplementation(async (value, id) => diff(id === 'a' ? fileA : fileB, value)),
    ...overrides,
  };
  return { transport, controller: createReviewController(transport) };
}

describe('review selection identity', () => {
  it('separates staged, net and individual turn comparisons', () => {
    const selections: ReviewSelection[] = [
      { kind: 'workspace', comparison: 'net' },
      { kind: 'workspace', comparison: 'staged' },
      { kind: 'workspace', comparison: 'unstaged' },
      { kind: 'session' },
      { kind: 'turn', messageId: 'one' },
      { kind: 'turn', messageId: 'two' },
    ];
    expect(new Set(selections.map(reviewSelectionKey)).size).toBe(selections.length);
  });
});

describe('review request consistency', () => {
  it('does not claim to show a previous capture when the selected scope never loaded', async () => {
    const { controller } = setup({
      summary: async () => {
        throw new Error('Disconnected');
      },
    });
    controller.select({ kind: 'session' });
    await controller.refresh();
    expect(controller.getSnapshot().error).toBe('Disconnected');
    expect(controller.getSnapshot().overview?.summary).toBeNull();
    expect(controller.getSnapshot().stale).toBe(false);
  });

  it('loads metadata while closed and reads source only after opening', async () => {
    const { controller, transport } = setup();
    await controller.refresh();
    expect(controller.getSnapshot().files).toEqual([fileA, fileB]);
    expect(transport.file).not.toHaveBeenCalled();
    controller.enableDetails(true);
    await vi.waitFor(() => expect(controller.getSnapshot().diff?.file.id).toBe('a'));
    await controller.refresh();
    expect(transport.files).toHaveBeenCalledTimes(1);
    expect(transport.file).toHaveBeenCalledTimes(1);
  });

  it('updates lazily computed file counts without inventing totals for the remaining files', async () => {
    const pendingFile = { ...fileA, additions: null, deletions: null };
    const pendingSummary = { ...summary(), additions: null, deletions: null };
    const { controller } = setup({
      summary: async () => overview(pendingSummary),
      files: async () => ({ files: [pendingFile, fileB], nextCursor: null }),
    });
    await controller.refresh();
    expect(controller.getSnapshot().files[0]?.additions).toBeNull();
    controller.enableDetails(true);
    await vi.waitFor(() => expect(controller.getSnapshot().diff?.file.id).toBe('a'));
    expect(controller.getSnapshot().files[0]).toEqual(fileA);
    expect(controller.getSnapshot().overview?.summary?.additions).toBeNull();
    expect(controller.getSnapshot().overview?.summary?.deletions).toBeNull();
    expect(controller.getSnapshot().overview?.summary?.complete).toBe(true);
  });

  it('ignores an old scope response after selecting a turn', async () => {
    const old = deferred<ReviewOverview>();
    const selected: ReviewSelection = { kind: 'turn', messageId: 'task-two' };
    const transportSummary = vi
      .fn<ReviewTransport['summary']>()
      .mockReturnValueOnce(old.promise)
      .mockResolvedValueOnce(overview(summary('two', selected)));
    const { controller } = setup({ summary: transportSummary });
    const loading = controller.refresh();
    await Promise.resolve();
    controller.select(selected);
    await controller.refresh();
    old.resolve(overview());
    await loading;
    expect(controller.getSnapshot().overview?.summary?.revision).toBe('two');
    expect(controller.getSnapshot().selection).toEqual(selected);
  });

  it('does not replace the selected file with a slower previous file request', async () => {
    const old = deferred<ReviewFileDiff>();
    const { controller } = setup({
      file: async (value, id) => (id === 'a' ? old.promise : diff(fileB, value)),
    });
    controller.enableDetails(true);
    const loading = controller.refresh();
    await vi.waitFor(() => expect(controller.getSnapshot().selectedFileId).toBe('a'));
    await controller.selectFile('b');
    old.resolve(diff(fileA));
    await loading;
    expect(controller.getSnapshot().selectedFileId).toBe('b');
    expect(controller.getSnapshot().diff?.file.id).toBe('b');
  });

  it('preserves the last successful capture on refresh failure', async () => {
    const transportSummary = vi
      .fn<ReviewTransport['summary']>()
      .mockResolvedValueOnce(overview())
      .mockRejectedValueOnce(new Error('Connection closed'));
    const { controller } = setup({ summary: transportSummary });
    controller.enableDetails(true);
    await controller.refresh();
    await controller.refresh(true);
    expect(controller.getSnapshot()).toMatchObject({
      stale: true,
      error: 'Connection closed',
      loading: false,
      overview: { summary: { revision: 'one' } },
      diff: { file: fileA },
    });
  });

  it('preserves cached content when an unavailable remote reports a typed result', async () => {
    const transportSummary = vi
      .fn<ReviewTransport['summary']>()
      .mockResolvedValueOnce(overview())
      .mockResolvedValueOnce({
        summary: null,
        unavailable: { code: 'remote_unavailable', message: 'Reconnect to the project' },
        turns: [],
      });
    const { controller } = setup({ summary: transportSummary });
    await controller.refresh();
    await controller.refresh(true);
    expect(controller.getSnapshot().overview?.summary?.revision).toBe('one');
    expect(controller.getSnapshot().stale).toBe(true);
  });

  it('rejects a file body that does not match the list revision', async () => {
    const { controller } = setup({ file: async () => diff(fileA, summary('another')) });
    controller.enableDetails(true);
    await controller.refresh();
    expect(controller.getSnapshot().diff).toBeNull();
    expect(controller.getSnapshot().fileError).toContain('no longer matches');
    expect(controller.getSnapshot().stale).toBe(true);
  });

  it('discards a metadata page from a replaced revision', async () => {
    const oldPage = deferred<ReviewFilePage>();
    const transportSummary = vi
      .fn<ReviewTransport['summary']>()
      .mockResolvedValueOnce(overview())
      .mockResolvedValueOnce(overview(summary('two')));
    const transportFiles = vi
      .fn<ReviewTransport['files']>()
      .mockResolvedValueOnce({ files: [fileA], nextCursor: 1 })
      .mockReturnValueOnce(oldPage.promise)
      .mockResolvedValueOnce({ files: [{ ...fileA, id: 'new-a' }], nextCursor: null });
    const { controller } = setup({ summary: transportSummary, files: transportFiles });
    await controller.refresh();
    const loadingPage = controller.loadMore();
    await controller.refresh(true);
    oldPage.resolve({ files: [fileB], nextCursor: null });
    await loadingPage;
    expect(controller.getSnapshot().files.map((file) => file.id)).toEqual(['new-a']);
  });

  it('settles unavailable baseline independently from an empty valid comparison', async () => {
    const { controller } = setup({
      summary: async () => ({
        summary: null,
        unavailable: { code: 'baseline_unavailable', message: 'No baseline' },
        turns: [],
      }),
    });
    await controller.refresh();
    expect(controller.getSnapshot().overview?.summary).toBeNull();
    expect(controller.getSnapshot().overview?.unavailable?.code).toBe('baseline_unavailable');
    expect(controller.getSnapshot().error).toBeNull();
  });
});
