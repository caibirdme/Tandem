// @vitest-environment jsdom
import { act } from 'react';
import type { ComponentProps } from 'react';
import { createRoot } from 'react-dom/client';
import type { Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { setLocale } from '../i18n';
import { CommandError } from '../lib/api';
import { emptyConversation } from '../lib/conversation';
import { createStagedMessages } from '../lib/stagedMessages';
import type { Project, Provider, Session } from '../lib/types';
import { Chat } from './Chat';

const project: Project = {
  id: 'project-1',
  name: 'Project',
  path: '/project',
  connection: { kind: 'local' },
  createdAt: '2026-10-17T00:00:00Z',
};
const session: Session = {
  reasoningEffort: null,
  contextUsage: null,
  titleState: 'preserved',
  id: 'session-1',
  title: 'Work',
  projectId: project.id,
  providerId: 'provider-1',
  modelId: 'model-1',
  createdAt: project.createdAt,
  updatedAt: project.createdAt,
  status: 'idle',
  permissionMode: 'allow-all',
};
const provider: Provider = {
  id: session.providerId,
  name: 'Provider',
  protocol: 'openai-completions',
  baseUrl: 'http://localhost:8080',
  listModelsPath: '/v1/models',
  hasApiKey: true,
  models: [
    {
      id: session.modelId,
      name: 'Model',
      reasoningEffort: null,
      contextWindow: 1_000_000,
      maxOutputTokens: null,
    },
  ],
};
const props: ComponentProps<typeof Chat> = {
  detailPanelId: 'details',
  selectedAgentId: null,
  onSelectAgent: () => undefined,
  session: undefined,
  project,
  providers: [],
  selection: null,
  onSelection: () => undefined,
  conversation: emptyConversation,
  historyLoading: false,
  sending: false,
  permissionModeSaving: false,
  permissionModeError: null,
  modelSaving: false,
  modelError: null,
  reasoningEffortSaving: false,
  reasoningEffortError: null,
  onReasoningEffort: () => undefined,
  onConfirmReasoningEffort: () => undefined,
  onConfirmModel: () => undefined,
  onConfirmPermissionMode: () => undefined,
  onSettings: () => undefined,
  onSend: async () => undefined,
  onAbort: () => undefined,
  onPermission: async () => undefined,
  connected: true,
};

let container: HTMLDivElement;
let root: Root;
let store: ReturnType<typeof createStagedMessages>;
let send = vi.fn<ComponentProps<typeof Chat>['onSend']>();
let steer = vi.fn<NonNullable<ComponentProps<typeof Chat>['onSteer']>>();
beforeEach(() => {
  vi.stubGlobal('IS_REACT_ACT_ENVIRONMENT', true);
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
  store = createStagedMessages();
  send = vi.fn<ComponentProps<typeof Chat>['onSend']>().mockResolvedValue(undefined);
  steer = vi.fn<NonNullable<ComponentProps<typeof Chat>['onSteer']>>().mockResolvedValue(undefined);
});
afterEach(async () => {
  await act(() => root.unmount());
  container.remove();
  setLocale('en');
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
async function render(running = true, id = session.id) {
  await act(() =>
    root.render(
      <Chat
        {...props}
        key={id}
        session={{ ...session, id, status: running ? 'running' : 'idle' }}
        providers={[provider]}
        selection={{ providerId: provider.id, modelId: session.modelId }}
        conversation={{ ...emptyConversation, status: running ? 'running' : 'idle' }}
        stagedMessages={store}
        onSend={send}
        onSteer={steer}
      />,
    ),
  );
}
function element<T extends HTMLElement>(selector: string): T {
  const value = container.querySelector<T>(selector);
  if (!value) throw new Error(`Missing element ${selector}`);
  return value;
}
async function type(value: string) {
  const input = element<HTMLTextAreaElement>('textarea');
  await act(() => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set?.call(
      input,
      value,
    );
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
}
async function click(selector: string) {
  await act(() => element<HTMLButtonElement>(selector).click());
}

describe('staged composer', () => {
  it('stages while busy without sending and explicitly steers in the same conversation', async () => {
    await render();
    await type('Use a smaller patch');
    await click('[aria-label="Stage message"]');
    expect(send).not.toHaveBeenCalled();
    expect(steer).not.toHaveBeenCalled();
    expect(store.getSnapshot().get(session.id)).toHaveLength(1);
    expect(element<HTMLTextAreaElement>('textarea').value).toBe('');
    await click('.staged-steer');
    expect(steer).toHaveBeenCalledWith(expect.any(String), 'Use a smaller patch', [], []);
    expect(store.getSnapshot().get(session.id)).toBeUndefined();
  });
  it('keeps staged messages on idle and saves edits back to the same item, not the agent', async () => {
    await render();
    await type('Original');
    await click('[aria-label="Stage message"]');
    const id = store.getSnapshot().get(session.id)?.[0]?.id;
    await click('[aria-label="Edit staged message"]');
    await type('Edited');
    await render(false);
    await click('[aria-label="Save to staged messages"]');
    expect(send).not.toHaveBeenCalled();
    expect(steer).not.toHaveBeenCalled();
    expect(store.getSnapshot().get(session.id)?.[0]).toMatchObject({
      id,
      draft: { text: 'Edited' },
    });
    await click('.staged-steer');
    expect(steer).toHaveBeenCalledWith(id, 'Edited', [], []);
  });
  it('preserves staged attachments across navigation and editing', async () => {
    const attachments = [{ name: 'notes.txt', mimeType: 'text/plain', data: 'aGk=' }];
    store.save(session.id, { text: 'Read notes', mentions: [] }, attachments, 'with-file');
    await render();
    await render(true, 'another-session');
    expect(container.querySelector('.staged-messages')).toBeNull();
    await render();
    await click('[aria-label="Edit staged message"]');
    await type('Read updated notes');
    await click('[aria-label="Save to staged messages"]');
    await click('.staged-steer');
    expect(steer).toHaveBeenCalledWith('with-file', 'Read updated notes', [], attachments);
  });
  it('keeps selected skills and permits retry only after a definitive rejection', async () => {
    const skill = { name: 'review', path: '/project/.agents/skills/review/SKILL.md' };
    store.save(
      session.id,
      { text: '$review this', mentions: [{ start: 0, end: 7, skill }] },
      [],
      'skill-message',
    );
    steer.mockRejectedValueOnce(new CommandError('Try again after preparation', 'conflict'));
    await render();
    await click('.staged-steer');
    expect(store.getSnapshot().get(session.id)?.[0]?.status).toBe('ready');
    await click('.staged-steer');
    expect(steer).toHaveBeenNthCalledWith(2, 'skill-message', '$review this', [skill], []);
    expect(store.getSnapshot().get(session.id)).toBeUndefined();
  });

  it('blocks duplicate clicks and retains ambiguous failures without blind retry', async () => {
    let reject: ((reason: unknown) => void) | undefined;
    steer.mockImplementation(
      () =>
        new Promise((_, fail) => {
          reject = fail;
        }),
    );
    store.save(session.id, { text: 'Change plan', mentions: [] }, [], 'one');
    await render();
    await click('.staged-steer');
    await click('.staged-steer');
    expect(steer).toHaveBeenCalledTimes(1);
    await act(() => reject?.(new CommandError('Acknowledgement lost', 'delivery_unknown')));
    expect(store.getSnapshot().get(session.id)?.[0]?.status).toBe('uncertain');
    expect(element<HTMLButtonElement>('.staged-steer').disabled).toBe(true);
    expect(element<HTMLButtonElement>('[aria-label="Edit staged message"]').disabled).toBe(true);
    expect(container.textContent).toContain('Delivery is uncertain');
  });
});
