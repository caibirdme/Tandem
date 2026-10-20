import * as Dropdown from '@radix-ui/react-dropdown-menu';
import {
  ArrowDown,
  ArrowRight,
  ArrowUp,
  Check,
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
import { lazy, Suspense, useEffect, useId, useRef, useState } from 'react';

import { useProjectSkills } from '../hooks/useProjectSkills';
import { useI18n } from '../i18n';
import { errorMessage } from '../lib/api';
import type { Conversation } from '../lib/conversation';
import { needsWorkPermissionConfirmation } from '../lib/sessionOptions';
import {
  editSkillDraft,
  findSkillQuery,
  restoreSkillDraft,
  selectedSkills,
  selectSkill,
  skillPickerKey,
} from '../lib/skillMentions';
import type { SkillDraft, SkillInputEdit } from '../lib/skillMentions';
import type {
  ModelSelection,
  PermissionMode,
  Project,
  ProjectSkill,
  Provider,
  ReasoningEffort,
  RuntimeStatus,
  Session,
  SelectedSkill,
} from '../lib/types';
import { ActivityMessage } from './ActivityMessage';
import { ContextUsageIndicator } from './ContextUsageIndicator';
import { PermissionModeSelector } from './PermissionModeSelector';
import { ReasoningEffortSelector } from './ReasoningEffortSelector';
import { SkillPicker } from './SkillPicker';
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
  reasoningEffortSaving: boolean;
  reasoningEffortError: string | null;
  onReasoningEffort: (reasoningEffort: ReasoningEffort) => void;
  onConfirmReasoningEffort: () => void;
  onPermissionMode: (mode: PermissionMode) => void;
  onConfirmPermissionMode: () => void;
  runtime: RuntimeStatus | undefined;
  installing: boolean;
  onInstall: () => void;
  onSettings: () => void;
  onSend: (
    content: string,
    permissionMode: PermissionMode,
    skills: SelectedSkill[],
    reasoningEffort: ReasoningEffort,
  ) => Promise<void>;
  onAbort: () => void;
  onPermission: (requestId: string, approved: boolean) => Promise<void>;
  connected: boolean;
}

export function Chat(props: ChatProps) {
  const { t } = useI18n();
  const [skillDraft, setSkillDraft] = useState<SkillDraft>({ text: '', mentions: [] });
  const draft = skillDraft.text;
  const [caret, setCaret] = useState({ start: 0, end: 0 });
  const [inputFocused, setInputFocused] = useState(false);
  const [composing, setComposing] = useState(false);
  const [pickerDismissed, setPickerDismissed] = useState(false);
  const [skillIndex, setSkillIndex] = useState(0);
  const skillPickerId = useId();
  const skillQuery =
    props.project && inputFocused && !composing && !pickerDismissed
      ? findSkillQuery(draft, caret.start, caret.end)
      : null;
  const skillCatalog = useProjectSkills(props.project?.id, skillQuery !== null, props.connected);
  const matchingSkills = skillCatalog.skills.filter((skill) =>
    skill.name.toLocaleLowerCase().startsWith(skillQuery?.prefix.toLocaleLowerCase() ?? ''),
  );
  const activeSkillIndex = Math.min(skillIndex, Math.max(matchingSkills.length - 1, 0));
  const skillsSelectable = !skillCatalog.loading && props.connected && matchingSkills.length > 0;
  const [draftPermissionMode, setDraftPermissionMode] = useState<PermissionMode>('ask');
  const [draftReasoningEffort, setDraftReasoningEffort] = useState<ReasoningEffort>(null);
  const [jumpVisible, setJumpVisible] = useState(false);
  const [pendingPermission, setPendingPermission] = useState<string | null>(null);
  const [localError, setLocalError] = useState<string | null>(null);
  const [workPermissionPromptOpen, setWorkPermissionPromptOpen] = useState(false);
  const transcript = useRef<HTMLDivElement>(null);
  const textarea = useRef<HTMLTextAreaElement>(null);
  const pendingInputEdit = useRef<SkillInputEdit | undefined>(undefined);
  const cancelWorkPermission = useRef<HTMLButtonElement>(null);
  const pendingWorkContent = useRef<SkillDraft | null>(null);
  const submitting = useRef(false);
  const stickToBottom = useRef(true);
  const running =
    props.sending || props.conversation.status === 'running' || props.session?.status === 'running';
  const permissionMode = props.project
    ? (props.session?.permissionMode ?? draftPermissionMode)
    : 'ask';
  const reasoningEffort = props.session ? props.session.reasoningEffort : draftReasoningEffort;
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
    props.reasoningEffortSaving ||
    Boolean(props.reasoningEffortError) ||
    !props.connected ||
    props.conversation.permissions.length > 0;
  const hasMessages = Boolean(
    props.conversation.messages.length ||
    props.conversation.stream ||
    props.conversation.failedSend,
  );
  const hasActiveActivity = props.conversation.messages.some(
    (message) => message.activity?.status === 'running',
  );
  const currentProvider = props.providers.find(
    (provider) => provider.id === props.selection?.providerId,
  );
  const currentModel = currentProvider?.models.find(
    (model) => model.id === props.selection?.modelId,
  );
  const modelDisabled = permissionModeDisabled;
  const reasoningEffortDisabled =
    !props.connected ||
    !currentModel ||
    (props.sending && !props.session) ||
    props.modelSaving ||
    Boolean(props.modelError);
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
    !props.reasoningEffortSaving &&
    !props.reasoningEffortError &&
    props.conversation.permissions.length === 0,
  );

  useEffect(() => {
    if (stickToBottom.current && transcript.current)
      transcript.current.scrollTop = transcript.current.scrollHeight;
  }, [props.conversation.messages, props.conversation.stream, props.conversation.permissions]);
  useEffect(() => {
    textarea.current?.focus();
  }, []);
  useEffect(() => {
    const input = textarea.current;
    if (!input) return;
    function captureEdit(event: InputEvent) {
      if (!input) return;
      pendingInputEdit.current = {
        text: input.value,
        start: input.selectionStart,
        end: input.selectionEnd,
        inputType: event.inputType,
      };
    }
    input.addEventListener('beforeinput', captureEdit);
    return () => input.removeEventListener('beforeinput', captureEdit);
  }, []);
  useEffect(() => {
    if (textarea.current) {
      textarea.current.style.height = 'auto';
      textarea.current.style.height = `${Math.min(textarea.current.scrollHeight, 190)}px`;
    }
  }, [draft]);

  async function submit() {
    if (!canSend || submitting.current || pendingWorkContent.current !== null) return;
    if (
      needsWorkPermissionConfirmation({
        projectId: props.project?.id ?? null,
        sessionId: props.session?.id ?? null,
        permissionMode,
        historyLoaded: props.historyLoaded,
        messages: props.conversation.messages,
      })
    ) {
      pendingWorkContent.current = skillDraft;
      setWorkPermissionPromptOpen(true);
      return;
    }
    await sendDraft(skillDraft, permissionMode);
  }
  async function sendDraft(savedDraft: SkillDraft, mode: PermissionMode) {
    if (submitting.current) return;
    submitting.current = true;
    setSkillDraft({ text: '', mentions: [] });
    setCaret({ start: 0, end: 0 });
    setLocalError(null);
    stickToBottom.current = true;
    try {
      await props.onSend(
        savedDraft.text.trim(),
        mode,
        props.project ? selectedSkills(savedDraft) : [],
        reasoningEffort,
      );
    } catch (reason) {
      setSkillDraft((current) => (current.text ? current : savedDraft));
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
  function suggestion(value: string, skills: SelectedSkill[] = []) {
    setSkillDraft(restoreSkillDraft(value, props.project ? skills : []));
    setCaret({ start: value.length, end: value.length });
    setPickerDismissed(true);
    textarea.current?.focus();
  }
  function chooseSkill(skill: ProjectSkill) {
    if (!skillQuery) return;
    const result = selectSkill(skillDraft, skillQuery, skill);
    setSkillDraft(result.draft);
    setCaret({ start: result.caret, end: result.caret });
    setPickerDismissed(true);
    requestAnimationFrame(() => {
      textarea.current?.focus();
      textarea.current?.setSelectionRange(result.caret, result.caret);
    });
  }
  function composerBlur(next: EventTarget | null) {
    if (
      next !== textarea.current &&
      !(next instanceof HTMLElement && next.closest('.skill-picker'))
    )
      setInputFocused(false);
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
              ) : message.role === 'activity' ? (
                <ActivityMessage
                  key={message.id}
                  content={message.content}
                  activity={message.activity}
                />
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
            {running &&
              !props.conversation.stream &&
              (!hasActiveActivity || props.conversation.permissions.length > 0) && (
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
                        suggestion(
                          props.conversation.failedSend.content,
                          props.conversation.failedSend.selectedSkills,
                        );
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
        {props.reasoningEffortError && (
          <Notice>
            <span>{props.reasoningEffortError}</span>
            <button
              className="text-button"
              type="button"
              onClick={props.onConfirmReasoningEffort}
              disabled={props.reasoningEffortSaving || !props.connected}
            >
              {props.reasoningEffortSaving ? (
                <>
                  <Spinner size={13} />
                  {t('chat.reasoningConfirming')}
                </>
              ) : (
                t('chat.confirmReasoningAgain')
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
          {skillQuery && (
            <SkillPicker
              id={skillPickerId}
              skills={matchingSkills}
              index={activeSkillIndex}
              loading={skillCatalog.loading}
              connected={props.connected}
              filtered={Boolean(skillQuery.prefix)}
              onIndex={setSkillIndex}
              onSelect={chooseSkill}
              onBlur={composerBlur}
            />
          )}
          <textarea
            ref={textarea}
            aria-label={props.project ? t('chat.taskInput') : t('chat.messageInput')}
            placeholder={props.project ? t('chat.taskPlaceholder') : t('chat.messagePlaceholder')}
            value={draft}
            role={props.project ? 'combobox' : undefined}
            aria-autocomplete={props.project ? 'list' : undefined}
            aria-expanded={props.project ? skillQuery !== null : undefined}
            aria-controls={skillQuery ? skillPickerId : undefined}
            aria-activedescendant={
              skillQuery && skillsSelectable
                ? `${skillPickerId}-${String(activeSkillIndex)}`
                : undefined
            }
            onFocus={() => setInputFocused(true)}
            onBlur={(event) => composerBlur(event.relatedTarget)}
            onSelect={(event) => {
              const input = event.currentTarget;
              setCaret({ start: input.selectionStart, end: input.selectionEnd });
            }}
            onChange={(event) => {
              const input = event.currentTarget;
              const text = input.value;
              const edit = pendingInputEdit.current;
              pendingInputEdit.current = undefined;
              setSkillDraft((current) => editSkillDraft(current, text, edit));
              setCaret({ start: input.selectionStart, end: input.selectionEnd });
              setPickerDismissed(false);
              setSkillIndex(0);
            }}
            onCompositionStart={() => setComposing(true)}
            onCompositionEnd={() => setComposing(false)}
            rows={2}
            onKeyDown={(event) => {
              const isComposing =
                composing || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229;
              const action = skillPickerKey(
                event.key,
                event,
                isComposing,
                skillQuery !== null,
                skillsSelectable ? matchingSkills.length : 0,
              );
              if (action) {
                event.preventDefault();
                if (action === 'dismiss') setPickerDismissed(true);
                else if (action === 'previous')
                  setSkillIndex(
                    (activeSkillIndex - 1 + matchingSkills.length) % matchingSkills.length,
                  );
                else if (action === 'next')
                  setSkillIndex((activeSkillIndex + 1) % matchingSkills.length);
                else {
                  const skill = matchingSkills[activeSkillIndex];
                  if (skill) chooseSkill(skill);
                }
                return;
              }
              if (event.key === 'Enter' && !event.shiftKey && !isComposing) {
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
              <ReasoningEffortSelector
                effort={reasoningEffort}
                defaultEffort={currentModel?.reasoningEffort ?? null}
                disabled={reasoningEffortDisabled}
                saving={props.reasoningEffortSaving}
                uncertain={Boolean(props.reasoningEffortError)}
                onChange={(effort) => {
                  if (reasoningEffortDisabled) return;
                  if (props.session) props.onReasoningEffort(effort);
                  else setDraftReasoningEffort(effort);
                }}
              />
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
            <span className="composer-keyboard-hint">
              {t(props.project ? 'chat.workKeyboardHint' : 'chat.keyboardHint')}
            </span>
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
