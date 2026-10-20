import * as Dropdown from '@radix-ui/react-dropdown-menu';
import {
  ArrowDown,
  ArrowRight,
  ArrowUp,
  Check,
  CheckCheck,
  ChevronDown,
  Code2,
  Compass,
  Copy,
  FileText,
  Folder,
  Lightbulb,
  LockKeyhole,
  MessageSquare,
  Plus,
  Server,
  ShieldCheck,
  Sparkles,
  Square,
  Terminal,
  Wrench,
} from 'lucide-react';
import { lazy, Suspense, useEffect, useRef, useState } from 'react';

import { useI18n } from '../i18n';
import { errorMessage } from '../lib/api';
import type { Conversation } from '../lib/conversation';
import { needsWorkPermissionConfirmation } from '../lib/sessionOptions';
import type {
  ModelSelection,
  PermissionMode,
  Project,
  Provider,
  RuntimeStatus,
  Session,
} from '../lib/types';
import { ContextUsageIndicator } from './ContextUsageIndicator';
import { PermissionModeSelector } from './PermissionModeSelector';
import { Modal, Notice, Spinner } from './ui';

interface ChatProps {
  session: Session | undefined;
  project: Project | undefined;
  providers: Provider[];
  selection: ModelSelection | null;
  onSelection: (selection: ModelSelection) => void;
  conversation: Conversation;
  historyLoading: boolean;
  historyLoaded: boolean;
  onReloadHistory: () => void;
  sending: boolean;
  permissionModeSaving: boolean;
  permissionModeError: string | null;
  modelSaving: boolean;
  modelError: string | null;
  onConfirmModel: () => void;
  onPermissionMode: (mode: PermissionMode) => void;
  onConfirmPermissionMode: () => void;
  runtime: RuntimeStatus | undefined;
  installing: boolean;
  onInstall: () => void;
  onSettings: () => void;
  onSend: (content: string, permissionMode: PermissionMode) => Promise<void>;
  onAbort: () => void;
  onPermission: (requestId: string, approved: boolean) => Promise<void>;
  connected: boolean;
}

export function Chat(props: ChatProps) {
  const { t } = useI18n();
  const [draft, setDraft] = useState('');
  const [draftPermissionMode, setDraftPermissionMode] = useState<PermissionMode>('ask');
  const [jumpVisible, setJumpVisible] = useState(false);
  const [pendingPermission, setPendingPermission] = useState<string | null>(null);
  const [localError, setLocalError] = useState<string | null>(null);
  const [workPermissionPromptOpen, setWorkPermissionPromptOpen] = useState(false);
  const transcript = useRef<HTMLDivElement>(null);
  const textarea = useRef<HTMLTextAreaElement>(null);
  const cancelWorkPermission = useRef<HTMLButtonElement>(null);
  const pendingWorkContent = useRef<string | null>(null);
  const submitting = useRef(false);
  const stickToBottom = useRef(true);
  const running =
    props.sending || props.conversation.status === 'running' || props.session?.status === 'running';
  const permissionMode = props.project
    ? (props.session?.permissionMode ?? draftPermissionMode)
    : 'ask';
  const workHistoryUncertain = Boolean(
    props.project &&
    props.session &&
    permissionMode === 'ask' &&
    !props.historyLoaded &&
    !props.conversation.messages.some((message) => message.role === 'user'),
  );
  const permissionModeDisabled =
    running ||
    props.historyLoading ||
    props.permissionModeSaving ||
    Boolean(props.permissionModeError) ||
    props.modelSaving ||
    Boolean(props.modelError) ||
    !props.connected ||
    props.conversation.permissions.length > 0;
  const hasMessages = Boolean(
    props.conversation.messages.length ||
    props.conversation.stream ||
    props.conversation.failedSend,
  );
  const currentProvider = props.providers.find(
    (provider) => provider.id === props.selection?.providerId,
  );
  const currentModel = currentProvider?.models.find(
    (model) => model.id === props.selection?.modelId,
  );
  const modelDisabled = permissionModeDisabled;
  const modelLabel = props.modelError
    ? t('permissions.unconfirmed')
    : currentModel?.name || currentModel?.id || props.selection?.modelId || t('chat.selectModel');
  const modelHint = running
    ? t('chat.modelRunningHint')
    : props.historyLoading
      ? t('chat.modelHistoryHint')
      : props.modelSaving
        ? t('chat.modelSaving')
        : props.modelError
          ? t('chat.modelUnconfirmedHint')
          : t('chat.modelSelectionHint');
  // Sending initializes the local or remote runtime on demand.
  const canSend = Boolean(
    draft.trim() &&
    props.selection &&
    currentModel &&
    props.connected &&
    !running &&
    !props.historyLoading &&
    !workHistoryUncertain &&
    !props.permissionModeSaving &&
    !props.permissionModeError &&
    !props.modelSaving &&
    !props.modelError &&
    props.conversation.permissions.length === 0,
  );

  useEffect(() => {
    if (stickToBottom.current && transcript.current)
      transcript.current.scrollTop = transcript.current.scrollHeight;
  }, [
    props.conversation.messages,
    props.conversation.stream,
    props.conversation.tools,
    props.conversation.permissions,
  ]);
  useEffect(() => {
    textarea.current?.focus();
  }, []);
  useEffect(() => {
    if (textarea.current) {
      textarea.current.style.height = 'auto';
      textarea.current.style.height = `${Math.min(textarea.current.scrollHeight, 190)}px`;
    }
  }, [draft]);

  async function submit() {
    if (!canSend || submitting.current || pendingWorkContent.current !== null) return;
    const content = draft.trim();
    if (
      needsWorkPermissionConfirmation({
        projectId: props.project?.id ?? null,
        sessionId: props.session?.id ?? null,
        permissionMode,
        historyLoaded: props.historyLoaded,
        messages: props.conversation.messages,
      })
    ) {
      pendingWorkContent.current = content;
      setWorkPermissionPromptOpen(true);
      return;
    }
    await sendDraft(content, permissionMode);
  }
  async function sendDraft(content: string, mode: PermissionMode) {
    if (submitting.current) return;
    submitting.current = true;
    setDraft('');
    setLocalError(null);
    stickToBottom.current = true;
    try {
      await props.onSend(content, mode);
    } catch (reason) {
      setDraft((current) => current || content);
      setLocalError(errorMessage(reason));
    } finally {
      submitting.current = false;
    }
  }
  function cancelWorkPrompt() {
    pendingWorkContent.current = null;
    setWorkPermissionPromptOpen(false);
  }
  async function confirmWorkPermission(mode: PermissionMode) {
    const content = pendingWorkContent.current;
    if (content === null || !props.project || !canSend || submitting.current) return;
    pendingWorkContent.current = null;
    setWorkPermissionPromptOpen(false);
    if (!props.session) setDraftPermissionMode(mode);
    await sendDraft(content, mode);
  }
  async function permission(requestId: string, approved: boolean) {
    setPendingPermission(requestId);
    setLocalError(null);
    try {
      await props.onPermission(requestId, approved);
    } catch (reason) {
      setLocalError(errorMessage(reason));
    } finally {
      setPendingPermission((current) => (current === requestId ? null : current));
    }
  }
  function suggestion(value: string) {
    setDraft(value);
    textarea.current?.focus();
  }

  return (
    <div className={`chat-surface ${hasMessages ? 'has-messages' : ''}`}>
      <Modal
        open={workPermissionPromptOpen}
        onOpenChange={(open) => {
          if (!open) cancelWorkPrompt();
        }}
        title={t('chat.confirmWorkPermissions')}
        description={t('chat.workPermissionDescription')}
        initialFocusRef={cancelWorkPermission}
        returnFocusRef={textarea}
      >
        <div className="work-permission-confirmation">
          <p>{t('chat.workPermissionExplanation')}</p>
          <p>{t('chat.fullAccessExplanation')}</p>
          <div className="work-permission-actions">
            <button
              className="button"
              type="button"
              ref={cancelWorkPermission}
              onClick={cancelWorkPrompt}
            >
              {t('chat.cancel')}
            </button>
            <button
              className="button"
              type="button"
              disabled={!canSend || !props.project}
              onClick={() => {
                void confirmWorkPermission('ask');
              }}
            >
              {t('chat.confirmAndContinue')}
            </button>
            <button
              className="button primary"
              type="button"
              disabled={!canSend || !props.project}
              onClick={() => {
                void confirmWorkPermission('allow-all');
              }}
            >
              {t('chat.allowAllAndContinue')}
            </button>
          </div>
        </div>
      </Modal>
      <div
        className="transcript-scroll"
        ref={transcript}
        onScroll={() => {
          const element = transcript.current;
          if (element) {
            stickToBottom.current =
              element.scrollHeight - element.scrollTop - element.clientHeight < 100;
            setJumpVisible(!stickToBottom.current);
          }
        }}
      >
        {!hasMessages && !props.historyLoading ? (
          <div className="welcome">
            <div className="welcome-eyebrow">
              <span className="welcome-emblem">
                <Sparkles size={20} strokeWidth={1.5} />
              </span>
              {props.project ? t('chat.workEyebrow') : t('chat.chatEyebrow')}
            </div>
            <h1>{props.project ? t('chat.workHeading') : t('chat.chatHeading')}</h1>
            <p>
              {props.project
                ? t('chat.workWelcome', { name: props.project.name })
                : t('chat.chatWelcome')}
            </p>
            {props.project && (
              <div className="project-location">
                {props.project.connection.kind === 'ssh' ? (
                  <Server size={14} />
                ) : (
                  <Folder size={14} />
                )}
                <span>
                  {props.project.connection.kind === 'ssh'
                    ? `${props.project.connection.user}@${props.project.connection.host}:`
                    : ''}
                  {props.project.path}
                </span>
              </div>
            )}
            <div className="suggestions">
              {(props.project
                ? [
                    {
                      icon: Compass,
                      title: t('chat.exploreProjectTitle'),
                      description: t('chat.exploreProjectDescription'),
                      prompt: t('chat.exploreProjectPrompt'),
                    },
                    {
                      icon: Code2,
                      title: t('chat.buildFeatureTitle'),
                      description: t('chat.buildFeatureDescription'),
                      prompt: t('chat.buildFeaturePrompt'),
                    },
                    {
                      icon: Wrench,
                      title: t('chat.reviewProjectTitle'),
                      description: t('chat.reviewProjectDescription'),
                      prompt: t('chat.reviewProjectPrompt'),
                    },
                  ]
                : [
                    {
                      icon: Lightbulb,
                      title: t('chat.organizeIdeaTitle'),
                      description: t('chat.organizeIdeaDescription'),
                      prompt: t('chat.organizeIdeaPrompt'),
                    },
                    {
                      icon: Compass,
                      title: t('chat.exploreQuestionTitle'),
                      description: t('chat.exploreQuestionDescription'),
                      prompt: t('chat.exploreQuestionPrompt'),
                    },
                    {
                      icon: FileText,
                      title: t('chat.writeTitle'),
                      description: t('chat.writeDescription'),
                      prompt: t('chat.writePrompt'),
                    },
                  ]
              ).map(({ icon: Icon, title, description, prompt }) => (
                <button className="suggestion" key={title} onClick={() => suggestion(prompt)}>
                  <Icon size={19} strokeWidth={1.5} />
                  <strong>{title}</strong>
                  <span>{description}</span>
                  <ArrowRight size={14} className="suggestion-arrow" />
                </button>
              ))}
            </div>
          </div>
        ) : (
          <div
            className="transcript"
            role="log"
            aria-label={t('chat.messagesLabel')}
            aria-live="polite"
          >
            {props.historyLoading && (
              <div className="history-loading">
                <Spinner />
                {t('chat.historyLoading')}
              </div>
            )}
            {props.conversation.messages.map((message) =>
              message.role === 'user' ? (
                <div className="user-message" key={message.id}>
                  <div>{message.content}</div>
                </div>
              ) : message.role === 'assistant' ? (
                <article className="assistant-message" key={message.id}>
                  <div className="assistant-label">
                    <img className="mini-logo" src="/tandem.svg" alt="" />
                    Tandem
                  </div>
                  <Markdown content={message.content} />
                  {message.interrupted && (
                    <span className="interrupted-label">{t('chat.interrupted')}</span>
                  )}
                  <CopyButton text={message.content} />
                </article>
              ) : (
                <details className="tool-message" key={message.id}>
                  <summary>
                    <Terminal size={14} />
                    {message.role === 'tool' ? t('chat.toolOutput') : t('chat.sessionInfo')}
                  </summary>
                  <pre>{message.content}</pre>
                </details>
              ),
            )}
            {props.conversation.stream && (
              <article className="assistant-message streaming">
                <div className="assistant-label">
                  <img className="mini-logo" src="/tandem.svg" alt="" />
                  Tandem
                </div>
                <Markdown content={props.conversation.stream} />
              </article>
            )}
            {props.conversation.tools.length > 0 && (
              <div className="tool-activities">
                {props.conversation.tools.map((tool, index) => (
                  <details
                    key={tool.toolCallId ?? `${tool.name}-${String(index)}`}
                    className="tool-activity"
                  >
                    <summary>
                      {tool.status === 'running' ? <Spinner size={13} /> : <CheckCheck size={13} />}
                      <span>{tool.name}</span>
                      <small>
                        {tool.status === 'running'
                          ? t('chat.toolRunning')
                          : tool.status === 'error'
                            ? t('chat.toolFailed')
                            : t('chat.toolCompleted')}
                      </small>
                      <ChevronDown size={13} />
                    </summary>
                    {tool.content && <pre>{tool.content}</pre>}
                  </details>
                ))}
              </div>
            )}
            {running && !props.conversation.stream && (
              <div className="thinking">
                <span />
                <span />
                <span />
                <small>
                  {props.conversation.permissions.length
                    ? t('chat.awaitingApproval')
                    : t('chat.thinking')}
                </small>
              </div>
            )}
            {props.conversation.permissions.map((request) => (
              <div className="permission-card" key={request.requestId}>
                <div className="permission-heading">
                  <ShieldCheck size={19} />
                  <div>
                    <strong>{t('chat.approvalRequired')}</strong>
                    <span>{request.kind}</span>
                  </div>
                </div>
                <pre className="permission-description">{request.description}</pre>
                <div className="permission-actions">
                  <button
                    className="button compact"
                    disabled={pendingPermission !== null}
                    onClick={() => {
                      void permission(request.requestId, false);
                    }}
                  >
                    {t('chat.deny')}
                  </button>
                  <button
                    className="button primary compact"
                    disabled={pendingPermission !== null}
                    onClick={() => {
                      void permission(request.requestId, true);
                    }}
                  >
                    {pendingPermission === request.requestId ? <Spinner /> : <Check size={14} />}
                    {t('chat.allowOnce')}
                  </button>
                </div>
              </div>
            ))}
            {props.conversation.error && (
              <Notice>
                {props.conversation.error}
                {props.conversation.failedSend && (
                  <button
                    className="text-button"
                    disabled={Boolean(draft)}
                    title={draft ? t('chat.restoreDraftHint') : undefined}
                    onClick={() => {
                      if (props.conversation.failedSend)
                        suggestion(props.conversation.failedSend.content);
                    }}
                  >
                    {t('chat.restoreDraft')}
                  </button>
                )}
              </Notice>
            )}
          </div>
        )}
      </div>
      <div className="composer-wrap">
        {jumpVisible && (
          <button
            className="jump-bottom icon-button"
            aria-label={t('chat.jumpToLatest')}
            onClick={() => {
              transcript.current?.scrollTo({
                top: transcript.current.scrollHeight,
                behavior: 'smooth',
              });
              stickToBottom.current = true;
            }}
          >
            <ArrowDown size={17} />
          </button>
        )}
        {localError && <Notice>{localError}</Notice>}
        {workHistoryUncertain && !props.historyLoading && (
          <Notice>
            <span>{t('chat.workHistoryUncertain')}</span>
            <button
              className="text-button"
              onClick={props.onReloadHistory}
              disabled={!props.connected}
            >
              {t('chat.reloadHistory')}
            </button>
          </Notice>
        )}
        {props.modelError && (
          <Notice>
            <span>{props.modelError}</span>
            <button
              className="text-button"
              type="button"
              onClick={props.onConfirmModel}
              disabled={props.modelSaving || !props.connected}
            >
              {props.modelSaving ? (
                <>
                  <Spinner size={13} />
                  {t('chat.modelConfirming')}
                </>
              ) : (
                t('chat.confirmModelAgain')
              )}
            </button>
          </Notice>
        )}
        {props.permissionModeError && (
          <Notice>
            <span>{props.permissionModeError}</span>
            <button
              className="text-button"
              type="button"
              onClick={props.onConfirmPermissionMode}
              disabled={props.permissionModeSaving || !props.connected}
            >
              {props.permissionModeSaving ? (
                <>
                  <Spinner size={13} />
                  {t('chat.permissionsConfirming')}
                </>
              ) : (
                t('chat.confirmPermissionsAgain')
              )}
            </button>
          </Notice>
        )}
        {!props.connected && (
          <div className="setup-banner">
            <span>
              <Spinner size={13} />
              {t('chat.reconnecting')}
            </span>
          </div>
        )}
        {props.providers.length === 0 ? (
          <div className="setup-banner">
            <span>
              <Sparkles size={16} />
              {t('chat.connectModel')}
            </span>
            <button className="text-button" onClick={props.onSettings}>
              {t('chat.setupProvider')}
              <ArrowRight size={13} />
            </button>
          </div>
        ) : (
          props.project?.connection.kind !== 'ssh' &&
          !props.runtime?.available && (
            <div className="setup-banner">
              <span>
                <Terminal size={16} />
                {props.runtime?.message || t('chat.prepareRuntimeDescription')}
              </span>
              <button className="text-button" disabled={props.installing} onClick={props.onInstall}>
                {props.installing ? (
                  <>
                    <Spinner />
                    {t('chat.preparingRuntime')}
                  </>
                ) : (
                  <>
                    {t('chat.prepareRuntime')}
                    <ArrowRight size={13} />
                  </>
                )}
              </button>
            </div>
          )
        )}
        <form
          className={`composer ${running ? 'composer-running' : ''}`}
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <textarea
            ref={textarea}
            aria-label={props.project ? t('chat.taskInput') : t('chat.messageInput')}
            placeholder={props.project ? t('chat.taskPlaceholder') : t('chat.messagePlaceholder')}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            rows={2}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
                event.preventDefault();
                void submit();
              }
            }}
          />
          <div className="composer-toolbar">
            <div className="composer-options">
              <span
                className="scope-pill"
                title={props.project ? t('chat.workScopeHint') : t('chat.chatScopeHint')}
              >
                {props.project ? <Folder size={13} /> : <MessageSquare size={13} />}
                {props.project ? t('chat.workMode') : t('chat.chatMode')}
              </span>
              <span className="toolbar-divider" />
              <Dropdown.Root>
                <Dropdown.Trigger
                  className="model-selector"
                  disabled={modelDisabled}
                  aria-label={t('chat.modelLabel', { name: modelLabel })}
                  aria-busy={props.modelSaving}
                  title={modelHint}
                >
                  {modelLabel}
                  {props.modelSaving ? <Spinner size={13} /> : <ChevronDown size={13} />}
                </Dropdown.Trigger>
                <Dropdown.Portal>
                  <Dropdown.Content
                    className="dropdown model-dropdown"
                    side="top"
                    align="start"
                    sideOffset={12}
                  >
                    {props.providers.length === 0 ? (
                      <Dropdown.Item className="dropdown-item" onSelect={props.onSettings}>
                        <Plus size={14} />
                        {t('chat.configureProvider')}
                      </Dropdown.Item>
                    ) : (
                      props.providers.map((provider) => (
                        <Dropdown.Group key={provider.id}>
                          <Dropdown.Label className="dropdown-label">
                            {provider.name}
                          </Dropdown.Label>
                          {provider.models.map((model) => (
                            <Dropdown.Item
                              key={model.id}
                              className="dropdown-item"
                              disabled={modelDisabled}
                              onSelect={() => {
                                if (!modelDisabled)
                                  props.onSelection({ providerId: provider.id, modelId: model.id });
                              }}
                            >
                              <span>{model.name || model.id}</span>
                              {props.selection?.providerId === provider.id &&
                                props.selection.modelId === model.id && <Check size={14} />}
                            </Dropdown.Item>
                          ))}
                        </Dropdown.Group>
                      ))
                    )}
                  </Dropdown.Content>
                </Dropdown.Portal>
              </Dropdown.Root>
              {props.project && (
                <PermissionModeSelector
                  mode={permissionMode}
                  disabled={permissionModeDisabled}
                  saving={props.permissionModeSaving}
                  uncertain={Boolean(props.permissionModeError)}
                  onChange={(mode) => {
                    if (permissionModeDisabled) return;
                    if (props.session) props.onPermissionMode(mode);
                    else setDraftPermissionMode(mode);
                  }}
                />
              )}
            </div>
            {running && !props.session ? (
              <button
                className="send-button"
                type="button"
                disabled
                aria-label={t('chat.creatingSession')}
              >
                <Spinner />
              </button>
            ) : running ? (
              <button
                className="send-button stop-button"
                type="button"
                aria-label={t('chat.stopResponse')}
                onClick={props.onAbort}
              >
                <Square size={14} fill="currentColor" />
              </button>
            ) : (
              <button
                className="send-button"
                type="submit"
                disabled={!canSend}
                aria-label={t('chat.sendMessage')}
              >
                <ArrowUp size={19} strokeWidth={2.4} />
              </button>
            )}
          </div>
        </form>
        <div className="composer-footnote">
          <span>
            {props.project ? (
              <>
                <ShieldCheck size={11} />
                {props.permissionModeError
                  ? t('chat.permissionsUnconfirmed')
                  : permissionMode === 'allow-all'
                    ? t('chat.fullAccessFootnote')
                    : t('chat.askPermissionFootnote')}
              </>
            ) : (
              <>
                <LockKeyhole size={11} />
                {t('chat.chatPrivacyFootnote')}
              </>
            )}
          </span>
          <div className="composer-status">
            <span className="composer-keyboard-hint">{t('chat.keyboardHint')}</span>
            {props.project && currentModel && (
              <ContextUsageIndicator
                usage={props.session?.contextUsage ?? null}
                contextWindow={currentModel.contextWindow}
              />
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

const MarkdownRenderer = lazy(() => import('./Markdown'));

function Markdown({ content }: { content: string }) {
  return (
    <Suspense fallback={<div className="markdown">{content}</div>}>
      <MarkdownRenderer content={content} />
    </Suspense>
  );
}

function CopyButton({ text }: { text: string }) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  const timeout = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timeout.current), []);
  return (
    <button
      className="icon-button copy-button"
      aria-label={copied ? t('chat.copied') : t('chat.copyResponse')}
      onClick={() => {
        void navigator.clipboard
          .writeText(text)
          .then(() => {
            setCopied(true);
            clearTimeout(timeout.current);
            timeout.current = setTimeout(() => setCopied(false), 1600);
          })
          .catch(() => setCopied(false));
      }}
    >
      {copied ? <Check size={14} /> : <Copy size={14} />}
    </button>
  );
}
