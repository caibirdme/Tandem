import { translate as t } from '../i18n';
import { errorMessage } from './api';
import { createSessionSettingChanges } from './sessionSettingChanges';
import type { ReasoningEffort } from './types';

export type ReasoningEffortChangeEvent =
  | { type: 'reasoningEffortChangeStarted'; sessionId: string }
  | {
      type: 'reasoningEffortChanged';
      sessionId: string;
      reasoningEffort: ReasoningEffort;
      notice?: string;
    }
  | { type: 'reasoningEffortUnconfirmed'; sessionId: string; error: string };

interface Dependencies {
  getRequests: () => Set<string>;
  advance: (sessionId: string) => void;
  update: (event: ReasoningEffortChangeEvent) => void;
  save: (sessionId: string, reasoningEffort: ReasoningEffort) => Promise<ReasoningEffort>;
  read: (sessionId: string) => Promise<ReasoningEffort>;
}

export function createReasoningEffortChanges(dependencies: Dependencies) {
  const { update } = dependencies;
  return createSessionSettingChanges({
    ...dependencies,
    started: (sessionId) => update({ type: 'reasoningEffortChangeStarted', sessionId }),
    confirmed: (sessionId, reasoningEffort, notice) =>
      update({
        type: 'reasoningEffortChanged',
        sessionId,
        reasoningEffort,
        ...(notice ? { notice } : {}),
      }),
    unconfirmed: (sessionId, error) =>
      update({ type: 'reasoningEffortUnconfirmed', sessionId, error }),
    get uncertainMessage() {
      return t('error.reasoningUnconfirmed');
    },
    get timeoutMessage() {
      return t('error.reasoningTimeout');
    },
    saveFailure: (reason) => t('error.reasoningSaveFailed', { error: errorMessage(reason) }),
    readFailure: (reason, failure) =>
      t('error.reasoningConfirmationFailed', {
        failure: failure ? `${failure} ` : '',
        error: errorMessage(reason),
      }),
    recovered: (reasoningEffort, failure) =>
      t('error.reasoningRecovered', {
        failure,
        effort: reasoningEffort === null ? t('chat.reasoningDefault') : reasoningEffort,
      }),
  });
}
