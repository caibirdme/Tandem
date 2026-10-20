import {
  ArrowLeft,
  Check,
  ChevronRight,
  Folder,
  FolderOpen,
  Laptop,
  Plug,
  Server,
} from 'lucide-react';
import { useEffect, useRef, useState } from 'react';

import { useI18n } from '../i18n';
import { dispatch, errorMessage, isDesktop } from '../lib/api';
import type { Connection, DirectoryListing, Project } from '../lib/types';
import { Modal, Notice, RequiredMark, Spinner } from './ui';

export function ProjectDialog({
  open,
  onClose,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  onCreated: (project: Project) => Promise<void>;
}) {
  const { t } = useI18n();
  const [kind, setKind] = useState<'local' | 'ssh'>('local');
  const [name, setName] = useState('');
  const [path, setPath] = useState('');
  const [host, setHost] = useState('');
  const [user, setUser] = useState('');
  const [port, setPort] = useState('22');
  const [identityFile, setIdentityFile] = useState('');
  const [busy, setBusy] = useState<'save' | 'check' | 'browse' | null>(null);
  const [notice, setNotice] = useState<{ text: string; error: boolean } | null>(null);
  const [connectionNotice, setConnectionNotice] = useState<{
    text: string;
    error: boolean;
    connectionKey: string;
  } | null>(null);
  const [listing, setListing] = useState<DirectoryListing | null>(null);
  const directoryBrowserRef = useRef<HTMLDivElement>(null);
  const noticeRef = useRef<HTMLDivElement>(null);
  const connectionNoticeRef = useRef<HTMLDivElement>(null);
  const browseStatusRef = useRef<HTMLDivElement>(null);
  const connection: Connection =
    kind === 'local'
      ? { kind }
      : {
          kind,
          host: host.trim(),
          user: user.trim(),
          port: Number(port),
          identityFile: identityFile.trim() || null,
        };
  const connectionKey = JSON.stringify(connection);
  const visibleConnectionNotice =
    connectionNotice?.connectionKey === connectionKey ? connectionNotice : null;
  const connectionValid =
    kind === 'local' ||
    (host.trim().length > 0 &&
      user.trim().length > 0 &&
      Number.isInteger(Number(port)) &&
      Number(port) > 0 &&
      Number(port) <= 65535);

  useEffect(() => {
    if (listing) directoryBrowserRef.current?.scrollIntoView({ block: 'nearest' });
  }, [listing]);
  useEffect(() => {
    if (notice) noticeRef.current?.scrollIntoView({ block: 'nearest' });
  }, [notice]);
  useEffect(() => {
    if (visibleConnectionNotice) connectionNoticeRef.current?.scrollIntoView({ block: 'nearest' });
  }, [visibleConnectionNotice]);
  useEffect(() => {
    if (kind === 'ssh' && busy === 'browse')
      browseStatusRef.current?.scrollIntoView({ block: 'nearest' });
  }, [busy, kind]);

  async function check() {
    setBusy('check');
    setNotice(null);
    setConnectionNotice(null);
    try {
      const result = await dispatch('check_connection', { connection });
      setConnectionNotice({ text: result.message, error: false, connectionKey });
    } catch (reason) {
      setConnectionNotice({ text: errorMessage(reason), error: true, connectionKey });
    } finally {
      setBusy(null);
    }
  }
  async function browse(directory?: string) {
    setBusy('browse');
    setNotice(null);
    try {
      if (kind === 'local' && isDesktop() && directory === undefined) {
        const { open: openDirectory } = await import('@tauri-apps/plugin-dialog');
        const selected = await openDirectory({
          directory: true,
          multiple: false,
          title: t('project.selectDirectory'),
        });
        if (typeof selected === 'string') {
          setPath(selected);
          if (!name) setName(selected.split(/[\\/]/u).findLast(Boolean) ?? t('project.new'));
        }
      } else {
        const result = await dispatch('browse_directory', {
          connection,
          path: directory ?? (path || '~'),
        });
        setListing(result);
      }
    } catch (reason) {
      setNotice({ text: errorMessage(reason), error: true });
    } finally {
      setBusy(null);
    }
  }
  async function save() {
    setBusy('save');
    setNotice(null);
    try {
      const project = await dispatch('create_project', {
        name: name.trim(),
        path: path.trim(),
        connection,
      });
      await onCreated(project);
      onClose();
    } catch (reason) {
      setNotice({ text: errorMessage(reason), error: true });
    } finally {
      setBusy(null);
    }
  }
  return (
    <Modal
      open={open}
      onOpenChange={(value) => {
        if (!value && !busy) onClose();
      }}
      title={t('common.addProject')}
      description={t('project.description')}
    >
      <form
        className="project-form"
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <fieldset className="connection-choice" aria-label={t('project.location')}>
          <button
            type="button"
            className={kind === 'local' ? 'active' : ''}
            onClick={() => {
              setKind('local');
              setListing(null);
              setNotice(null);
              setConnectionNotice(null);
            }}
          >
            <Laptop size={20} />
            <strong>{t('project.local')}</strong>
            <span>{t('project.localHint')}</span>
            {kind === 'local' && <Check size={13} className="choice-check" />}
          </button>
          <button
            type="button"
            className={kind === 'ssh' ? 'active' : ''}
            onClick={() => {
              setKind('ssh');
              setListing(null);
              setNotice(null);
              setConnectionNotice(null);
            }}
          >
            <Server size={20} />
            <strong>{t('project.remote')}</strong>
            <span>{t('project.remoteHint')}</span>
            {kind === 'ssh' && <Check size={13} className="choice-check" />}
          </button>
        </fieldset>
        {kind === 'ssh' && (
          <div className="ssh-fields">
            <div className="form-grid host-port">
              <label>
                <span>
                  {t('project.host')} <RequiredMark />
                </span>
                <input
                  required
                  value={host}
                  onChange={(event) => {
                    setHost(event.target.value);
                    setConnectionNotice(null);
                  }}
                  placeholder={t('project.hostExample')}
                  autoComplete="off"
                />
              </label>
              <label>
                <span>
                  {t('project.port')} <RequiredMark />
                </span>
                <input
                  className="number-input"
                  type="number"
                  required
                  min="1"
                  max="65535"
                  value={port}
                  onChange={(event) => {
                    setPort(event.target.value);
                    setConnectionNotice(null);
                  }}
                />
              </label>
            </div>
            <label>
              <span>
                {t('project.user')} <RequiredMark />
              </span>
              <input
                required
                value={user}
                onChange={(event) => {
                  setUser(event.target.value);
                  setConnectionNotice(null);
                }}
                placeholder={t('project.userExample')}
                autoComplete="username"
              />
            </label>
            <details>
              <summary>{t('project.sshKey')}</summary>
              <label>
                {t('project.keyPath')}
                <input
                  value={identityFile}
                  onChange={(event) => {
                    setIdentityFile(event.target.value);
                    setConnectionNotice(null);
                  }}
                  placeholder="~/.ssh/id_ed25519"
                />
                <small>{t('project.keyHint')}</small>
              </label>
            </details>
            <button
              className="button compact"
              type="button"
              disabled={!connectionValid || busy !== null}
              onClick={() => {
                void check();
              }}
            >
              {busy === 'check' ? <Spinner /> : <Plug size={14} />}
              {busy === 'check' ? t('project.checkingConnection') : t('project.testConnection')}
            </button>
            {visibleConnectionNotice && (
              <div ref={connectionNoticeRef}>
                <Notice tone={visibleConnectionNotice.error ? 'error' : 'success'}>
                  {visibleConnectionNotice.text}
                </Notice>
              </div>
            )}
          </div>
        )}
        <label>
          <span>
            {t('project.name')} <RequiredMark />
          </span>
          <input
            value={name}
            onChange={(event) => setName(event.target.value)}
            required
            placeholder={t('project.nameExample')}
          />
        </label>
        <label>
          <span>
            {t('project.directory')} <RequiredMark />
          </span>
          <div className="input-with-button">
            <input
              required
              value={path}
              onChange={(event) => setPath(event.target.value)}
              placeholder={
                kind === 'ssh' ? '/home/ubuntu/projects/my-app' : '/Users/you/projects/my-app'
              }
            />
            <button
              className="button"
              type="button"
              disabled={!connectionValid || busy !== null}
              onClick={() => {
                void browse();
              }}
              aria-label={t('project.browse')}
            >
              {busy === 'browse' ? <Spinner /> : <FolderOpen size={17} />}
            </button>
          </div>
          <small>{kind === 'ssh' ? t('project.remoteSetup') : t('project.directoryHint')}</small>
        </label>
        {kind === 'ssh' && busy === 'browse' && (
          <div ref={browseStatusRef}>
            <Notice tone="neutral">{t('project.loadingDirectories')}</Notice>
          </div>
        )}
        {listing && (
          <div className="directory-browser" ref={directoryBrowserRef}>
            <div className="directory-heading">
              <button
                type="button"
                className="icon-button"
                disabled={busy !== null}
                onClick={() => {
                  void browse(`${listing.path}/..`);
                }}
                aria-label={t('project.parentDirectory')}
              >
                <ArrowLeft size={15} />
              </button>
              <span title={listing.path}>{listing.path}</span>
              <button
                type="button"
                className="text-button"
                onClick={() => {
                  setPath(listing.path);
                  if (!name) setName(listing.path.split('/').findLast(Boolean) ?? t('project.new'));
                  setListing(null);
                }}
              >
                {t('project.selectCurrent')}
              </button>
            </div>
            <div className="directory-entries">
              {listing.entries.length === 0 ? (
                <p>{t('project.noDirectories')}</p>
              ) : (
                listing.entries.map((entry) => (
                  <button
                    type="button"
                    key={entry.path}
                    disabled={busy !== null}
                    onClick={() => {
                      void browse(entry.path);
                    }}
                  >
                    <Folder size={15} />
                    <span>{entry.name}</span>
                    <ChevronRight size={14} />
                  </button>
                ))
              )}
            </div>
          </div>
        )}
        {notice && (
          <div ref={noticeRef}>
            <Notice tone={notice.error ? 'error' : 'success'}>{notice.text}</Notice>
          </div>
        )}
        <div className="form-footer">
          <button className="button" type="button" onClick={onClose} disabled={busy !== null}>
            {t('common.cancel')}
          </button>
          <button className="button primary" type="submit" disabled={busy !== null}>
            {busy === 'save' ? <Spinner /> : <Folder size={15} />}
            {busy === 'save' ? t('project.preparing') : t('common.createProject')}
          </button>
        </div>
      </form>
    </Modal>
  );
}
