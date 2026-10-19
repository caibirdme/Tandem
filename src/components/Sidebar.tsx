import * as Dropdown from '@radix-ui/react-dropdown-menu';
import {
  ArrowUpRight,
  ChevronDown,
  Folder,
  MessageSquare,
  MoreHorizontal,
  PanelLeftClose,
  Plus,
  Server,
  Settings2,
  SquarePen,
  Sparkles,
  Trash2,
} from 'lucide-react';

import { useI18n } from '../i18n';
import { displaySessionTitle } from '../lib/sessionTitles';
import type { Project, Session } from '../lib/types';
import { Spinner } from './ui';

interface SidebarProps {
  sessions: Session[];
  projects: Project[];
  activeSessionId: string | null;
  activeProjectId: string | null;
  connected: boolean;
  onNewChat: () => void;
  onSession: (session: Session) => void;
  onProject: (project: Project) => void;
  onCreateProject: () => void;
  onSettings: () => void;
  onDeleteSession: (session: Session) => void;
  onRenameSession: (session: Session) => void;
  onRegenerateTitle: (session: Session) => void;
  titleGenerations: ReadonlySet<string>;
  onClose: () => void;
}

export function Sidebar(props: SidebarProps) {
  const { t } = useI18n();
  const chats = props.sessions.filter((session) => session.projectId === null);
  return (
    <aside className="sidebar" aria-label={t('sidebar.navigation')}>
      <div className="sidebar-brand">
        <img className="brand-symbol" src="/tandem.svg" alt="" />
        <span>Tandem</span>
        <button
          className="icon-button sidebar-collapse"
          onClick={props.onClose}
          aria-label={t('sidebar.collapse')}
        >
          <PanelLeftClose size={17} />
        </button>
      </div>
      <button className="new-chat-button" onClick={props.onNewChat}>
        <SquarePen size={17} />
        {t('common.createChat')}
        <span className="shortcut">⌘ N</span>
      </button>
      <div className="sidebar-scroll">
        <section className="sidebar-section">
          <div className="section-label">
            <span>{t('sidebar.chats')}</span>
            <MessageSquare size={13} />
          </div>
          {chats.length === 0 ? (
            <p className="sidebar-empty">{t('sidebar.empty')}</p>
          ) : (
            chats.map((session) => (
              <SessionItem
                key={session.id}
                session={session}
                active={session.id === props.activeSessionId}
                onSelect={props.onSession}
                onDelete={props.onDeleteSession}
                onRename={props.onRenameSession}
                onRegenerateTitle={props.onRegenerateTitle}
                titleGenerating={props.titleGenerations.has(session.id)}
                connected={props.connected}
              />
            ))
          )}
        </section>
        <section className="sidebar-section">
          <div className="section-label">
            <span>{t('sidebar.projects')}</span>
            <button
              className="icon-button tiny"
              onClick={props.onCreateProject}
              aria-label={t('common.createProject')}
            >
              <Plus size={15} />
            </button>
          </div>
          {props.projects.map((project) => {
            const active = project.id === props.activeProjectId;
            const sessions = props.sessions.filter((session) => session.projectId === project.id);
            return (
              <div className="project-group" key={project.id}>
                <button
                  className={`project-nav ${active && !props.activeSessionId ? 'selected' : ''}`}
                  onClick={() => props.onProject(project)}
                  title={project.path}
                >
                  {project.connection.kind === 'ssh' ? <Server size={16} /> : <Folder size={16} />}
                  <span>{project.name}</span>
                  <ChevronDown size={13} className={active ? '' : 'chevron-closed'} />
                </button>
                {active && (
                  <div className="project-sessions">
                    {sessions.map((session) => (
                      <SessionItem
                        key={session.id}
                        session={session}
                        active={session.id === props.activeSessionId}
                        onSelect={props.onSession}
                        onDelete={props.onDeleteSession}
                        onRename={props.onRenameSession}
                        onRegenerateTitle={props.onRegenerateTitle}
                        titleGenerating={props.titleGenerations.has(session.id)}
                        connected={props.connected}
                      />
                    ))}
                    <button className="project-new" onClick={() => props.onProject(project)}>
                      <Plus size={13} />
                      {t('common.createWork')}
                    </button>
                  </div>
                )}
              </div>
            );
          })}
          <button className="add-project-button" onClick={props.onCreateProject}>
            <Plus size={15} />
            {t('common.addProject')}
          </button>
        </section>
      </div>
      <div className="sidebar-bottom">
        <div className="workspace-hint">
          <span className="hint-mark">
            <ArrowUpRight size={16} />
          </span>
          <div>
            <strong>{t('sidebar.tagline')}</strong>
            <small>{t('sidebar.hint')}</small>
          </div>
        </div>
        <button
          className="settings-button"
          onClick={props.onSettings}
          aria-label={t('common.settings')}
        >
          <Settings2 size={17} />
          {t('common.settings')}
          <span
            className={`connection-dot ${props.connected ? 'online' : ''}`}
            title={props.connected ? t('sidebar.connected') : t('sidebar.disconnected')}
          />
        </button>
      </div>
    </aside>
  );
}

function SessionItem({
  session,
  active,
  onSelect,
  onDelete,
  onRename,
  onRegenerateTitle,
  titleGenerating,
  connected,
}: {
  session: Session;
  active: boolean;
  onSelect: (session: Session) => void;
  onDelete: (session: Session) => void;
  onRename: (session: Session) => void;
  onRegenerateTitle: (session: Session) => void;
  titleGenerating: boolean;
  connected: boolean;
}) {
  const { t } = useI18n();
  return (
    <div className={`session-item ${active ? 'selected' : ''}`}>
      <button
        className="session-select"
        onClick={() => onSelect(session)}
        title={displaySessionTitle(session)}
      >
        {session.status === 'running' && <span className="activity-dot" />}
        {titleGenerating && (
          <span title={t('common.titleGenerating')}>
            <Spinner size={11} />
          </span>
        )}
        <span>{displaySessionTitle(session)}</span>
      </button>
      <Dropdown.Root>
        <Dropdown.Trigger
          className="icon-button session-menu"
          aria-label={t('sidebar.sessionOptions', { title: displaySessionTitle(session) })}
        >
          <MoreHorizontal size={15} />
        </Dropdown.Trigger>
        <Dropdown.Portal>
          <Dropdown.Content className="dropdown" side="right" align="start" sideOffset={6}>
            <Dropdown.Item className="dropdown-item" onSelect={() => onRename(session)}>
              <SquarePen size={14} />
              {t('sidebar.rename')}
            </Dropdown.Item>
            <Dropdown.Item
              className="dropdown-item"
              disabled={!connected || titleGenerating}
              onSelect={() => onRegenerateTitle(session)}
            >
              <Sparkles size={14} />
              {titleGenerating ? t('common.titleGeneratingBusy') : t('common.regenerateTitle')}
            </Dropdown.Item>
            <Dropdown.Item
              className="dropdown-item danger"
              disabled={session.status === 'running'}
              onSelect={() => onDelete(session)}
            >
              <Trash2 size={14} />
              {t('common.deleteChat')}
            </Dropdown.Item>
          </Dropdown.Content>
        </Dropdown.Portal>
      </Dropdown.Root>
    </div>
  );
}
