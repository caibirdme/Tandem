import * as Dropdown from '@radix-ui/react-dropdown-menu';
import {
  ChevronRight,
  Folder,
  FileDiff,
  MoreHorizontal,
  PanelLeft,
  Plus,
  RefreshCw,
  Server,
  Settings2,
  SquarePen,
  Sparkles,
  Trash2,
  X,
} from 'lucide-react';
import { lazy, Suspense, useCallback, useEffect, useId, useRef, useState } from 'react';

import { ChangesDetailPanel, ReviewCounts } from './components/ChangesDetailPanel';
import { Chat } from './components/Chat';
import { Sidebar } from './components/Sidebar';
import { SubagentDetailPanel } from './components/SubagentDetailPanel';
import { Modal, Notice, Spinner } from './components/ui';
import { useReview } from './hooks/useReview';
import { useTandem } from './hooks/useTandem';
import { useI18n } from './i18n';
import { dispatch, errorMessage } from './lib/api';
import { resolveDetailPanel } from './lib/detailPanel';
import type { DetailPanelSelection } from './lib/detailPanel';
import { newSessionPayload } from './lib/sessionOptions';
import { displaySessionTitle } from './lib/sessionTitles';
import type {
  AttachmentInput,
  ModelSelection,
  PermissionMode,
  Project,
  ReasoningEffort,
  SelectedSkill,
  Session,
} from './lib/types';

const Settings = lazy(async () => ({ default: (await import('./components/Settings')).Settings }));
const ProjectDialog = lazy(async () => ({
  default: (await import('./components/ProjectDialog')).ProjectDialog,
}));

export function App() {
  const { t } = useI18n();
  const tandem = useTandem();
  const detailPanelId = useId();
  const detailPanelTrigger = useRef<HTMLElement | null>(null);
  const [detailPanel, setDetailPanel] = useState<DetailPanelSelection | null>(null);
  const [projectId, setProjectId] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [projectOpen, setProjectOpen] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(() => window.innerWidth >= 800);
  const [selection, setSelection] = useState<ModelSelection | null>(null);
  const [sendingScopes, setSendingScopes] = useState<ReadonlySet<string>>(new Set());
  // Explicit navigation resets the view; creating a session from its draft keeps it mounted.
  const [viewKey, setViewKey] = useState(() => crypto.randomUUID());
  const navigationRevision = useRef(0);
  const [actionBusy, setActionBusy] = useState(false);
  const [sessionAction, setSessionAction] = useState<{
    session: Session;
    action: 'rename' | 'delete';
  } | null>(null);
  const [deleteProject, setDeleteProject] = useState<Project | null>(null);
  const [sessionTitle, setSessionTitle] = useState('');
  const snapshot = tandem.snapshot;
  const fontSize = snapshot?.settings.fontSize ?? 'medium';
  useEffect(() => {
    document.documentElement.dataset.fontSize = fontSize;
  }, [fontSize]);
  const session = snapshot?.sessions.find((item) => item.id === tandem.activeSessionId);
  const project = snapshot?.projects.find((item) => item.id === (session?.projectId ?? projectId));
  const selectedDetail = resolveDetailPanel(detailPanel, viewKey, session?.id, tandem.conversation);
  const review = useReview(
    session?.projectId ? session.id : undefined,
    selectedDetail?.kind === 'changes',
    tandem.connected,
  );
  const closeDetailPanel = useCallback(() => setDetailPanel(null), []);
  const sendScope = session?.id ?? viewKey;
  const sending = sendingScopes.has(sendScope);
  const providers = snapshot?.settings.providers ?? [];
  const firstProvider = providers.find((provider) => provider.models.length > 0);
  const defaultModel = firstProvider?.models[0];
  const effectiveSelection = session
    ? { providerId: session.providerId, modelId: session.modelId }
    : selection &&
        providers.some(
          (provider) =>
            provider.id === selection.providerId &&
            provider.models.some((model) => model.id === selection.modelId),
        )
      ? selection
      : firstProvider && defaultModel
        ? { providerId: firstProvider.id, modelId: defaultModel.id }
        : null;
  const { setActiveSessionId } = tandem;
  const newDraft = useCallback(
    (targetProjectId: string | null) => {
      navigationRevision.current += 1;
      setViewKey(crypto.randomUUID());
      setActiveSessionId(null);
      setProjectId(targetProjectId);
    },
    [setActiveSessionId],
  );
  const newChat = useCallback(() => newDraft(null), [newDraft]);

  useEffect(() => {
    function keydown(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'n') {
        event.preventDefault();
        newChat();
      }
      if ((event.metaKey || event.ctrlKey) && event.key === ',') {
        event.preventDefault();
        setSettingsOpen(true);
      }
    }
    window.addEventListener('keydown', keydown);
    return () => window.removeEventListener('keydown', keydown);
  }, [newChat]);

  async function send(
    content: string,
    permissionMode: PermissionMode,
    skills: SelectedSkill[],
    reasoningEffort: ReasoningEffort,
    attachments: AttachmentInput[],
    fleet: boolean,
  ) {
    if (!effectiveSelection) {
      setSettingsOpen(true);
      return;
    }
    const submittedNavigation = navigationRevision.current;
    let requestScope = sendScope;
    setSendingScopes((current) => new Set([...current, sendScope]));
    try {
      let target = session;
      if (!target) {
        target = await dispatch(
          'create_session',
          newSessionPayload(
            project?.id ?? null,
            effectiveSelection,
            permissionMode,
            reasoningEffort,
          ),
        );
        const targetId = target.id;
        setSendingScopes((current) => {
          const next = new Set(current);
          next.delete(sendScope);
          next.add(targetId);
          return next;
        });
        requestScope = targetId;
        tandem.addSession(target, navigationRevision.current === submittedNavigation);
      }
      if (target.projectId && target.permissionMode !== permissionMode) {
        const confirmedMode = await tandem.changePermissionMode(target.id, permissionMode);
        if (confirmedMode !== permissionMode) throw new Error(t('app.permissionUnconfirmed'));
      }
      await tandem.send(target.id, content, skills, attachments, fleet);
    } finally {
      setSendingScopes((current) => {
        const next = new Set(current);
        next.delete(requestScope);
        return next;
      });
    }
  }
  function selectSession(value: Session) {
    if (value.id === tandem.activeSessionId) return;
    navigationRevision.current += 1;
    setViewKey(crypto.randomUUID());
    tandem.setActiveSessionId(value.id);
    setProjectId(value.projectId);
    if (window.innerWidth < 800) setSidebarOpen(false);
  }
  function selectProject(value: Project) {
    newDraft(value.id);
    if (window.innerWidth < 800) setSidebarOpen(false);
  }
  async function actOnSession() {
    if (!sessionAction) return;
    setActionBusy(true);
    tandem.setError(null);
    try {
      if (sessionAction.action === 'rename')
        await tandem.renameSession(sessionAction.session.id, sessionTitle.trim());
      else {
        await tandem.deleteSession(sessionAction.session.id);
        if (tandem.activeSessionId === sessionAction.session.id) newDraft(project?.id ?? null);
      }
      await tandem.reload();
      setSessionAction(null);
    } catch (reason) {
      tandem.setError(errorMessage(reason));
    } finally {
      setActionBusy(false);
    }
  }
  function rename(value: Session) {
    setSessionTitle(displaySessionTitle(value));
    setSessionAction({ session: value, action: 'rename' });
  }

  return (
    <div
      className={`app-shell ${sidebarOpen ? '' : 'sidebar-hidden'} ${selectedDetail ? 'detail-panel-open' : ''}`}
    >
      {sidebarOpen && (
        <>
          <button
            className="sidebar-backdrop"
            aria-label={t('app.closeSidebar')}
            onClick={() => setSidebarOpen(false)}
          />
          <Sidebar
            sessions={snapshot?.sessions ?? []}
            projects={snapshot?.projects ?? []}
            activeSessionId={tandem.activeSessionId}
            activeProjectId={project?.id ?? null}
            connected={tandem.connected}
            onNewChat={newChat}
            onSession={selectSession}
            onProject={selectProject}
            onCreateProject={() => setProjectOpen(true)}
            onSettings={() => setSettingsOpen(true)}
            onDeleteSession={(value) => setSessionAction({ session: value, action: 'delete' })}
            onRenameSession={rename}
            onRegenerateTitle={(value) => {
              void tandem.regenerateTitle(value.id);
            }}
            titleGenerations={tandem.titleGenerations}
            onClose={() => setSidebarOpen(false)}
          />
        </>
      )}
      <main className="main-content">
        <header className="topbar" data-tauri-drag-region>
          <div className="topbar-title">
            {!sidebarOpen && (
              <button
                className="icon-button"
                aria-label={t('app.expandSidebar')}
                onClick={() => setSidebarOpen(true)}
              >
                <PanelLeft size={18} />
              </button>
            )}
            {project ? (
              <>
                <span className="breadcrumb">
                  {project.connection.kind === 'ssh' ? <Server size={14} /> : <Folder size={14} />}
                  {project.name}
                </span>
                <ChevronRight size={13} />
              </>
            ) : null}
            <span className="current-session-title">
              {session
                ? displaySessionTitle(session)
                : project
                  ? t('common.newWork')
                  : t('common.newChat')}
            </span>
            {session && tandem.titleGenerations.has(session.id) && (
              <output className="title-progress">
                <Spinner size={12} />
                {t('common.titleGenerating')}
              </output>
            )}
          </div>
          <div className="topbar-actions">
            {session?.projectId && (
              <button
                type="button"
                className="review-header-button"
                aria-label={t('review.open')}
                aria-expanded={selectedDetail?.kind === 'changes'}
                aria-controls={detailPanelId}
                onClick={(event) => {
                  detailPanelTrigger.current = event.currentTarget;
                  if (selectedDetail?.kind === 'changes') closeDetailPanel();
                  else setDetailPanel({ kind: 'changes', sessionId: session.id, viewKey });
                }}
              >
                <FileDiff size={15} />
                <span>{t('review.title')}</span>
                {review.state.overview?.summary && (
                  <>
                    <span className="review-header-file-count">
                      {review.state.overview.summary.filesTotal}
                    </span>
                    <ReviewCounts
                      additions={review.state.overview.summary.additions}
                      deletions={review.state.overview.summary.deletions}
                    />
                  </>
                )}
              </button>
            )}
            {project && (
              <span className="environment-badge">
                <span />
                {project.connection.kind === 'ssh' ? 'SSH' : t('app.local')}
              </span>
            )}
            {session && (
              <button
                className="icon-button"
                aria-label={project ? t('common.createWork') : t('common.createChat')}
                onClick={() => newDraft(project?.id ?? null)}
              >
                <Plus size={18} />
              </button>
            )}
            <Dropdown.Root>
              <Dropdown.Trigger className="icon-button" aria-label={t('app.moreOptions')}>
                <MoreHorizontal size={20} />
              </Dropdown.Trigger>
              <Dropdown.Portal>
                <Dropdown.Content className="dropdown" sideOffset={8} align="end">
                  <Dropdown.Item className="dropdown-item" onSelect={() => setSettingsOpen(true)}>
                    <Settings2 size={14} />
                    {t('common.settings')}
                  </Dropdown.Item>
                  {session && (
                    <Dropdown.Item className="dropdown-item" onSelect={() => rename(session)}>
                      <SquarePen size={14} />
                      {t('common.renameChat')}
                    </Dropdown.Item>
                  )}
                  {session && (
                    <Dropdown.Item
                      className="dropdown-item"
                      disabled={!tandem.connected || tandem.titleGenerations.has(session.id)}
                      onSelect={() => {
                        void tandem.regenerateTitle(session.id);
                      }}
                    >
                      <Sparkles size={14} />
                      {tandem.titleGenerations.has(session.id)
                        ? t('common.titleGeneratingBusy')
                        : t('common.regenerateTitle')}
                    </Dropdown.Item>
                  )}
                  <Dropdown.Item
                    className="dropdown-item"
                    onSelect={() => {
                      void tandem
                        .reload()
                        .catch((reason: unknown) => tandem.setError(errorMessage(reason)));
                    }}
                  >
                    <RefreshCw size={14} />
                    {t('app.refreshConnection')}
                  </Dropdown.Item>
                  {project && (
                    <Dropdown.Item
                      className="dropdown-item danger"
                      onSelect={() => setDeleteProject(project)}
                    >
                      <Trash2 size={14} />
                      {t('common.removeProject')}
                    </Dropdown.Item>
                  )}
                </Dropdown.Content>
              </Dropdown.Portal>
            </Dropdown.Root>
          </div>
        </header>
        {session && tandem.titleError && (
          <div className="title-notice">
            <Notice tone="neutral">
              <span>{t('app.titleFailed', { error: tandem.titleError })}</span>
              <button
                className="text-button"
                disabled={!tandem.connected || tandem.titleGenerations.has(session.id)}
                onClick={() => {
                  void tandem.regenerateTitle(session.id);
                }}
              >
                {t('app.regenerate')}
              </button>
              <button
                className="icon-button"
                aria-label={t('app.dismissTitleNotice')}
                onClick={() => tandem.dismissTitleError(session.id)}
              >
                <X size={14} />
              </button>
            </Notice>
          </div>
        )}
        {tandem.error && (
          <div className="global-error">
            <Notice>
              {tandem.error}
              <button
                className="text-button"
                onClick={() => {
                  void tandem
                    .reload()
                    .catch((reason: unknown) => tandem.setError(errorMessage(reason)));
                }}
              >
                {t('common.retry')}
                <RefreshCw size={12} />
              </button>
            </Notice>
          </div>
        )}
        {tandem.loading ? (
          <div className="app-loading">
            <Spinner size={23} />
            <span>{t('app.connecting')}</span>
          </div>
        ) : (
          <Chat
            key={viewKey}
            detailPanelId={detailPanelId}
            selectedAgentId={selectedDetail?.kind === 'subagent' ? selectedDetail.agent.id : null}
            onReviewTurn={(messageId, trigger) => {
              if (!session?.projectId) return;
              detailPanelTrigger.current = trigger;
              review.controller.select({ kind: 'turn', messageId });
              setDetailPanel({ kind: 'changes', sessionId: session.id, viewKey });
            }}
            onSelectAgent={(agentId, trigger) => {
              detailPanelTrigger.current = trigger;
              if (session)
                setDetailPanel({ kind: 'subagent', sessionId: session.id, viewKey, agentId });
            }}
            session={session}
            project={project}
            providers={providers}
            selection={effectiveSelection}
            onSelection={(value) => {
              if (session)
                void tandem
                  .changeSessionModel(session.id, value)
                  .catch((reason: unknown) => tandem.setError(errorMessage(reason)));
              else setSelection(value);
            }}
            modelSaving={tandem.modelSaving}
            modelError={tandem.modelError}
            onConfirmModel={() => {
              if (session)
                void tandem
                  .confirmSessionModel(session.id)
                  .catch((reason: unknown) => tandem.setError(errorMessage(reason)));
            }}
            conversation={tandem.conversation}
            reasoningEffortSaving={tandem.reasoningEffortSaving}
            reasoningEffortError={tandem.reasoningEffortError}
            onReasoningEffort={(value) => {
              if (session)
                void tandem
                  .changeReasoningEffort(session.id, value)
                  .catch((reason: unknown) => tandem.setError(errorMessage(reason)));
            }}
            onConfirmReasoningEffort={() => {
              if (session)
                void tandem
                  .confirmReasoningEffort(session.id)
                  .catch((reason: unknown) => tandem.setError(errorMessage(reason)));
            }}
            historyLoading={tandem.historyLoading}
            sending={sending}
            permissionModeSaving={tandem.permissionModeSaving}
            permissionModeError={tandem.permissionModeError}
            onConfirmPermissionMode={() => {
              if (session) void tandem.confirmPermissionMode(session.id);
            }}
            onSettings={() => setSettingsOpen(true)}
            onSend={send}
            onAbort={() => {
              if (session)
                void dispatch('abort_session', { sessionId: session.id }).catch((reason: unknown) =>
                  tandem.setError(errorMessage(reason)),
                );
            }}
            onPermission={async (requestId, approved) => {
              if (session) await tandem.respondPermission(session.id, requestId, approved);
            }}
            connected={tandem.connected}
          />
        )}
      </main>
      {selectedDetail?.kind === 'changes' && (
        <ChangesDetailPanel
          id={detailPanelId}
          state={review.state}
          controller={review.controller}
          messages={tandem.conversation.messages}
          running={session?.status === 'running'}
          onClose={closeDetailPanel}
          returnFocusRef={detailPanelTrigger}
        />
      )}
      {selectedDetail?.kind === 'subagent' && (
        <SubagentDetailPanel
          id={detailPanelId}
          agent={selectedDetail.agent}
          messages={tandem.conversation.messages}
          stream={tandem.conversation.agentStreams[selectedDetail.agent.id] ?? ''}
          onClose={closeDetailPanel}
          returnFocusRef={detailPanelTrigger}
        />
      )}
      <Suspense fallback={null}>
        {settingsOpen && snapshot && (
          <Settings
            open={settingsOpen}
            onClose={() => setSettingsOpen(false)}
            settings={snapshot.settings}
            onSaved={tandem.reload}
          />
        )}
        {projectOpen && (
          <ProjectDialog
            open={projectOpen}
            onClose={() => setProjectOpen(false)}
            onCreated={async (value) => {
              await tandem.reload();
              selectProject(value);
            }}
          />
        )}
      </Suspense>
      <Modal
        open={sessionAction !== null}
        onOpenChange={(value) => {
          if (!value) setSessionAction(null);
        }}
        title={sessionAction?.action === 'rename' ? t('common.renameChat') : t('common.deleteChat')}
        description={
          sessionAction?.action === 'rename'
            ? t('app.renameDescription')
            : t('app.deleteDescription')
        }
      >
        <form
          className="simple-form"
          onSubmit={(event) => {
            event.preventDefault();
            void actOnSession();
          }}
        >
          {tandem.error && <Notice>{tandem.error}</Notice>}
          {sessionAction?.action === 'rename' ? (
            <label>
              {t('app.conversationTitle')}
              <input
                required
                value={sessionTitle}
                onChange={(event) => setSessionTitle(event.target.value)}
              />
            </label>
          ) : (
            <p>
              {t('app.confirmDelete', {
                title: sessionAction ? displaySessionTitle(sessionAction.session) : '',
              })}
            </p>
          )}
          <div className="form-footer">
            <button type="button" className="button" onClick={() => setSessionAction(null)}>
              {t('common.cancel')}
            </button>
            <button
              className={`button ${sessionAction?.action === 'delete' ? 'destructive' : 'primary'}`}
              type="submit"
              disabled={actionBusy}
            >
              {actionBusy && <Spinner />}
              {sessionAction?.action === 'rename' ? t('common.save') : t('common.delete')}
            </button>
          </div>
        </form>
      </Modal>
      <Modal
        open={deleteProject !== null}
        onOpenChange={(value) => {
          if (!value) setDeleteProject(null);
        }}
        title={t('common.removeProject')}
        description={t('app.removeDescription')}
      >
        <div className="simple-form">
          {tandem.error && <Notice>{tandem.error}</Notice>}
          <p>{t('app.confirmRemove', { name: deleteProject?.name ?? '' })}</p>
          <div className="form-footer">
            <button className="button" onClick={() => setDeleteProject(null)}>
              {t('common.cancel')}
            </button>
            <button
              className="button destructive"
              disabled={actionBusy}
              onClick={() => {
                if (!deleteProject) return;
                setActionBusy(true);
                tandem.setError(null);
                void tandem
                  .deleteProject(deleteProject.id)
                  .then(async () => {
                    if (project?.id === deleteProject.id) newChat();
                    await tandem.reload();
                    setDeleteProject(null);
                  })
                  .catch((reason: unknown) => tandem.setError(errorMessage(reason)))
                  .finally(() => setActionBusy(false));
              }}
            >
              {actionBusy && <Spinner />}
              {t('common.removeProject')}
            </button>
          </div>
        </div>
      </Modal>
    </div>
  );
}
