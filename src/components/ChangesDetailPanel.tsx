import {
  AlertCircle,
  ChevronLeft,
  ChevronRight,
  FileDiff,
  FileText,
  RefreshCw,
  Search,
} from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { RefObject } from 'react';

import { useI18n } from '../i18n';
import type { MessageKey } from '../i18n';
import type { ReviewController, ReviewState } from '../lib/reviews';
import type { ReviewFile, ReviewUnavailableCode } from '../lib/reviewTypes';
import type { Message } from '../lib/types';
import { DetailPanel } from './DetailPanel';
import { ReviewDiff } from './ReviewDiff';
import { Spinner } from './ui';

import './ChangesDetailPanel.css';

const unavailableLabels: Record<ReviewUnavailableCode, MessageKey> = {
  not_repository: 'review.notRepository',
  git_unavailable: 'review.gitUnavailable',
  remote_unavailable: 'review.remoteUnavailable',
  baseline_unavailable: 'review.baselineUnavailable',
  capturing: 'review.capturing',
  inspection_failed: 'review.inspectionFailed',
};
const fileLabels: Record<ReviewFile['status'], MessageKey> = {
  added: 'review.added',
  modified: 'review.modified',
  deleted: 'review.deleted',
  renamed: 'review.renamed',
  type_changed: 'review.typeChanged',
  conflict: 'review.conflict',
  unchanged: 'review.unchanged',
};
const contentLabels: Record<Exclude<ReviewFile['contentKind'], 'text'>, MessageKey> = {
  binary: 'review.binary',
  too_large: 'review.tooLarge',
  unsupported: 'review.unsupported',
  conflict: 'review.conflict',
};
const statusLetters: Record<ReviewFile['status'], string> = {
  added: 'A',
  modified: 'M',
  deleted: 'D',
  renamed: 'R',
  type_changed: 'T',
  conflict: 'U',
  unchanged: '•',
};

export function ReviewCounts({
  additions,
  deletions,
}: {
  additions: number | null;
  deletions: number | null;
}) {
  return (
    <span className="review-counts">
      <span className="review-additions">{additions === null ? '+?' : `+${additions}`}</span>
      <span className="review-deletions">{deletions === null ? '−?' : `−${deletions}`}</span>
    </span>
  );
}

export function ChangesDetailPanel({
  id,
  state,
  controller,
  messages,
  running,
  onClose,
  returnFocusRef,
}: {
  id: string;
  state: ReviewState;
  controller: ReviewController;
  messages: Message[];
  running: boolean;
  onClose: () => void;
  returnFocusRef?: RefObject<HTMLElement | null> | undefined;
}) {
  const { t, locale } = useI18n();
  const summary = state.overview?.summary;
  const unavailable = state.overview?.unavailable;
  const [filter, setFilter] = useState('');
  const [layout, setLayout] = useState<'unified' | 'split'>('unified');
  const [wrap, setWrap] = useState(false);
  const [wide, setWide] = useState(false);
  const container = useCallback((element: HTMLDivElement | null) => {
    if (!element) return;
    const measure = () => setWide(element.clientWidth >= 720);
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    measure();
    return () => observer.disconnect();
  }, []);
  const content = useRef<HTMLDivElement>(null);
  const userMessages = messages.filter(
    (message) => message.role === 'user' && !message.id.startsWith('pending:'),
  );
  const turns = state.overview?.turns ?? [];
  const selected = state.files.find((file) => file.id === state.selectedFileId);
  const selectedIndex = state.files.findIndex((file) => file.id === state.selectedFileId);
  const filtered = state.files.filter((file) =>
    `${file.path}\n${file.oldPath ?? ''}`.toLocaleLowerCase().includes(filter.toLocaleLowerCase()),
  );
  useEffect(() => {
    if (content.current) content.current.scrollTop = 0;
  }, [state.selectedFileId, state.selection]);
  const scopeValue =
    state.selection.kind === 'turn' ? `turn:${state.selection.messageId}` : state.selection.kind;
  return (
    <DetailPanel
      id={id}
      title={t('review.title')}
      onClose={onClose}
      returnFocusRef={returnFocusRef}
      bodyClassName="review-panel-body"
    >
      <div className="changes-panel" ref={container}>
        <div className="review-toolbar">
          <label className="review-scope">
            <span>{t('review.scope')}</span>
            <select
              value={scopeValue}
              onChange={(event) => {
                const value = event.currentTarget.value;
                if (value === 'workspace')
                  controller.select({ kind: 'workspace', comparison: 'net' });
                else if (value === 'session') controller.select({ kind: 'session' });
                else if (value.startsWith('turn:'))
                  controller.select({ kind: 'turn', messageId: value.slice(5) });
              }}
            >
              <option value="workspace">{t('review.workspace')}</option>
              <option value="session">{t('review.session')}</option>
              {userMessages.map((message, index) => (
                <option key={message.id} value={`turn:${message.id}`}>
                  {t('review.turnNumber', { number: index + 1 })} · {message.content.slice(0, 48)}
                </option>
              ))}
              {turns
                .filter((turn) => !userMessages.some((message) => message.id === turn.messageId))
                .map((turn) => (
                  <option key={turn.messageId} value={`turn:${turn.messageId}`}>
                    {t('review.turn')} · {new Date(turn.capturedAt).toLocaleString(locale)}
                  </option>
                ))}
            </select>
          </label>
          <button
            type="button"
            className="icon-button"
            disabled={state.loading}
            aria-label={t('review.refresh')}
            title={t('review.refresh')}
            onClick={() => {
              void controller.refresh(true);
            }}
          >
            {state.loading ? <Spinner size={15} /> : <RefreshCw size={15} />}
          </button>
        </div>
        {state.selection.kind === 'workspace' && (
          <fieldset className="review-comparisons" aria-label={t('review.comparison')}>
            {(['net', 'staged', 'unstaged'] as const).map((comparison) => (
              <button
                type="button"
                key={comparison}
                aria-pressed={
                  state.selection.kind === 'workspace' && state.selection.comparison === comparison
                }
                onClick={() => controller.select({ kind: 'workspace', comparison })}
              >
                {t(`review.${comparison}`)}
              </button>
            ))}
          </fieldset>
        )}
        <p className="review-attribution">
          {t(
            state.selection.kind === 'workspace'
              ? 'review.workspaceAttribution'
              : 'review.sessionAttribution',
          )}{' '}
          {t('review.coverage')}
        </p>
        {running && state.selection.kind !== 'workspace' && (
          <p className="review-notice">{t('review.running')}</p>
        )}
        {summary && (
          <div className="review-summary">
            <strong>{t('review.files', { count: summary.filesTotal })}</strong>
            <ReviewCounts additions={summary.additions} deletions={summary.deletions} />
            <span
              className="review-time"
              title={new Date(summary.capturedAt).toLocaleString(locale)}
            >
              {t('review.captured', {
                time: new Date(summary.capturedAt).toLocaleTimeString(locale),
              })}
            </span>
            <span className="review-root" title={summary.workspacePath}>
              {summary.workspacePath}
            </span>
          </div>
        )}
        {state.error && (
          <div className="review-notice review-warning" role="alert">
            <AlertCircle size={14} />
            <span>
              {state.stale && t('review.stale')} {state.error}
            </span>
          </div>
        )}
        {summary && !summary.complete && (
          <div className="review-notice review-warning">
            <AlertCircle size={14} />
            <span>
              {t('review.partial')}{' '}
              {summary.excludedFiles > 0 && t('review.excluded', { count: summary.excludedFiles })}
            </span>
          </div>
        )}
        {unavailable ? (
          <div className="review-empty">
            <FileDiff size={28} />
            <p>{t(unavailableLabels[unavailable.code])}</p>
            {unavailable.message && (
              <details>
                <summary>{t('review.details')}</summary>
                <p>{unavailable.message}</p>
              </details>
            )}
          </div>
        ) : !summary ? (
          <div className="review-empty">
            {state.loading && (
              <>
                <Spinner />
                <span>{t('review.loading')}</span>
              </>
            )}
          </div>
        ) : summary.filesTotal === 0 ? (
          <div className="review-empty">
            <FileDiff size={28} />
            <strong>{t('review.empty')}</strong>
            <p>{t('review.emptyHint')}</p>
          </div>
        ) : (
          <>
            <div className="review-file-browser">
              <label className="review-filter">
                <Search size={14} />
                <input
                  value={filter}
                  aria-label={t('review.filter')}
                  placeholder={t('review.filter')}
                  onChange={(event) => setFilter(event.currentTarget.value)}
                />
              </label>
              <nav className="review-file-list" aria-label={t('review.fileList')}>
                {filtered.map((file) => (
                  <button
                    type="button"
                    className="review-file"
                    key={file.id}
                    aria-current={file.id === state.selectedFileId ? 'true' : undefined}
                    title={file.path}
                    onClick={() => {
                      void controller.selectFile(file.id);
                    }}
                  >
                    <span
                      className={`review-file-status ${file.status}`}
                      title={t(fileLabels[file.status])}
                    >
                      {statusLetters[file.status]}
                    </span>
                    <span className="review-file-path">{file.path}</span>
                    <ReviewCounts additions={file.additions} deletions={file.deletions} />
                  </button>
                ))}
                {filtered.length === 0 && (
                  <p className="review-no-matches">{t('review.noMatches')}</p>
                )}
              </nav>
              {state.nextCursor !== null && (
                <div className="review-pagination">
                  <span>
                    {t('review.filesLoaded', {
                      loaded: state.files.length,
                      total: summary.filesTotal,
                    })}
                  </span>
                  <button
                    type="button"
                    className="text-button"
                    disabled={state.loadingFiles}
                    onClick={() => {
                      void controller.loadMore();
                    }}
                  >
                    {state.loadingFiles && <Spinner size={12} />}
                    {t('review.moreFiles')}
                  </button>
                </div>
              )}
            </div>
            {selected && (
              <header className="review-file-header">
                <div className="review-file-heading">
                  <FileText size={15} />
                  <strong title={selected.path}>{selected.path}</strong>
                  <span>{t(fileLabels[selected.status])}</span>
                </div>
                {selected.oldPath && <p>{t('review.oldPath', { path: selected.oldPath })}</p>}
                {selected.oldMode !== selected.newMode && (
                  <p>
                    {t('review.modeChanged', {
                      before: selected.oldMode ?? '—',
                      after: selected.newMode ?? '—',
                    })}
                  </p>
                )}
                <div className="review-file-options">
                  <fieldset className="review-layout" aria-label={t('review.layout')}>
                    <button
                      type="button"
                      aria-pressed={layout === 'unified' || !wide}
                      onClick={() => setLayout('unified')}
                    >
                      {t('review.unified')}
                    </button>
                    <button
                      type="button"
                      aria-pressed={layout === 'split' && wide}
                      disabled={!wide}
                      onClick={() => setLayout('split')}
                    >
                      {t('review.split')}
                    </button>
                  </fieldset>
                  <label className="review-wrap-control">
                    <input
                      type="checkbox"
                      checked={wrap}
                      onChange={(event) => setWrap(event.currentTarget.checked)}
                    />
                    {t('review.wrap')}
                  </label>
                  <div className="review-file-navigation">
                    <button
                      type="button"
                      className="icon-button"
                      aria-label={t('review.previousFile')}
                      disabled={selectedIndex <= 0}
                      onClick={() => {
                        const file = state.files[selectedIndex - 1];
                        if (file) void controller.selectFile(file.id);
                      }}
                    >
                      <ChevronLeft size={15} />
                    </button>
                    <button
                      type="button"
                      className="icon-button"
                      aria-label={t('review.nextFile')}
                      disabled={selectedIndex < 0 || selectedIndex >= state.files.length - 1}
                      onClick={() => {
                        const file = state.files[selectedIndex + 1];
                        if (file) void controller.selectFile(file.id);
                      }}
                    >
                      <ChevronRight size={15} />
                    </button>
                  </div>
                </div>
              </header>
            )}
            <div className="review-content" ref={content}>
              {state.loadingDiff ? (
                <div className="review-empty">
                  <Spinner />
                  <span>{t('review.loadingFile')}</span>
                </div>
              ) : state.fileError ? (
                <div className="review-empty" role="alert">
                  <AlertCircle size={24} />
                  <p>{state.fileError}</p>
                  <button
                    type="button"
                    className="button"
                    onClick={() => {
                      if (selected) void controller.selectFile(selected.id);
                    }}
                  >
                    {t('common.retry')}
                  </button>
                </div>
              ) : state.diff ? (
                state.diff.file.contentKind !== 'text' ? (
                  <div className="review-empty">
                    <FileDiff size={24} />
                    <p>{t(contentLabels[state.diff.file.contentKind])}</p>
                  </div>
                ) : state.diff.hunks.length === 0 ? (
                  <div className="review-empty">
                    <p>{t('review.noTextChange')}</p>
                    <div className="review-file-git-state">
                      {selected?.indexStatus && (
                        <span>{t('review.indexState', { status: selected.indexStatus })}</span>
                      )}
                      {selected?.worktreeStatus && (
                        <span>
                          {t('review.worktreeState', { status: selected.worktreeStatus })}
                        </span>
                      )}
                    </div>
                  </div>
                ) : (
                  <ReviewDiff
                    key={`${state.diff.reviewId}:${state.diff.revision}:${state.diff.file.id}`}
                    diff={state.diff}
                    split={layout === 'split' && wide}
                    wrap={wrap}
                  />
                )
              ) : (
                <div className="review-empty">
                  <p>{t('review.selectFile')}</p>
                </div>
              )}
            </div>
          </>
        )}
        <footer className="review-footer">{t('review.readOnly')}</footer>
      </div>
    </DetailPanel>
  );
}
