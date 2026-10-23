import { Activity, ChartNoAxesCombined, RefreshCw, Sparkles } from 'lucide-react';
import { useEffect, useState, useSyncExternalStore } from 'react';

import { useI18n } from '../i18n';
import { dispatch } from '../lib/api';
import { formatTokens, usageDayWindows, usageSeries } from '../lib/usage';
import type { UsageDashboardData, UsageDayWindow, UsageRange } from '../lib/usage';
import { subscribeUsageUpdates, usageCaptureFailed } from '../lib/usageEvents';
import { UsageChart } from './UsageChart';

import './UsageDashboard.css';

export function UsageDashboard() {
  const { t } = useI18n();
  const captureFailed = useSyncExternalStore(
    subscribeUsageUpdates,
    usageCaptureFailed,
    usageCaptureFailed,
  );
  const [range, setRange] = useState<UsageRange>(7);
  const [revision, setRevision] = useState(0);
  const [result, setResult] = useState<{ data: UsageDashboardData; days: UsageDayWindow[] } | null>(
    null,
  );
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    const days = usageDayWindows(range);
    void dispatch('get_usage_dashboard', { days })
      .then((data) => {
        if (current) {
          setResult({ data, days });
          setError(null);
        }
      })
      .catch((reason: unknown) => {
        if (current) setError(reason instanceof Error ? reason.message : String(reason));
      })
      .finally(() => {
        if (current) setLoading(false);
      });
    return () => {
      current = false;
    };
  }, [range, revision]);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    function refreshSoon() {
      if (timer !== undefined) return;
      timer = setTimeout(() => {
        timer = undefined;
        setRevision((value) => value + 1);
      }, 300);
    }
    const unsubscribe = subscribeUsageUpdates(refreshSoon);
    window.addEventListener('focus', refreshSoon);
    return () => {
      unsubscribe();
      window.removeEventListener('focus', refreshSoon);
      if (timer !== undefined) clearTimeout(timer);
    };
  }, []);
  function reload() {
    setLoading(true);
    setError(null);
    setRevision((value) => value + 1);
  }
  return (
    <section className="usage-dashboard" aria-label={t('usage.title')} aria-busy={loading}>
      <header className="usage-heading">
        <div>
          <h2>{t('usage.title')}</h2>
          <p>{t('usage.description')}</p>
        </div>
        <div className="usage-controls">
          <label>
            <span className="usage-visually-hidden">{t('usage.range')}</span>
            <select
              value={range}
              onChange={(event) => {
                const value = Number(event.target.value);
                if (value === 7 || value === 14 || value === 30) {
                  setRange(value);
                  reload();
                }
              }}
            >
              {([7, 14, 30] as const).map((days) => (
                <option key={days} value={days}>
                  {t('usage.lastDays', { days })}
                </option>
              ))}
            </select>
          </label>
          <button
            type="button"
            className="usage-refresh"
            disabled={loading}
            aria-label={t('usage.refresh')}
            title={t('usage.refresh')}
            onClick={reload}
          >
            <RefreshCw size={15} />
          </button>
        </div>
      </header>
      {captureFailed && (
        <p className="usage-capture-warning" role="alert">
          {t('usage.captureWarning')}
        </p>
      )}
      {loading ? (
        <output className="usage-loading">
          <RefreshCw size={20} />
          <span>{t('usage.loading')}</span>
        </output>
      ) : error ? (
        <div className="usage-error" role="alert">
          <p>{error}</p>
          <button type="button" className="usage-retry" onClick={reload}>
            {t('common.retry')}
          </button>
        </div>
      ) : (
        result && <UsageDashboardContent data={result.data} days={result.days} />
      )}
    </section>
  );
}

export function UsageDashboardContent({
  data,
  days,
}: {
  data: UsageDashboardData;
  days: UsageDayWindow[];
}) {
  const { t, locale } = useI18n();
  const series = usageSeries(data, days);
  const activeDays = new Set([
    ...data.dailyTokens.filter((row) => row.totalTokens > 0).map((row) => row.date),
    ...data.dailySkills.filter((row) => row.count > 0).map((row) => row.date),
  ]).size;
  const dateTime = (value: string) =>
    new Intl.DateTimeFormat(locale, {
      month: 'short',
      day: 'numeric',
      hour: 'numeric',
      minute: '2-digit',
    }).format(new Date(value));
  return (
    <>
      <div className="usage-summary">
        <div>
          <ChartNoAxesCombined size={17} />
          <span>{t('usage.totalTokens')}</span>
          <strong title={data.totalTokens.toLocaleString(locale)}>
            {formatTokens(data.totalTokens)}
          </strong>
        </div>
        <div>
          <Sparkles size={17} />
          <span>{t('usage.totalSkills')}</span>
          <strong>{data.skillInvocations.toLocaleString(locale)}</strong>
        </div>
        <div>
          <Activity size={17} />
          <span>{t('usage.activeDays')}</span>
          <strong>
            {activeDays}
            <small> / {days.length}</small>
          </strong>
        </div>
      </div>
      <section className="usage-card">
        <header>
          <h3>{t('usage.tokens')}</h3>
          <p>{t('usage.tokensDescription')}</p>
        </header>
        {series.tokens.length ? (
          <UsageChart
            key={`tokens-${days[0]?.date}-${days.length}`}
            kind="tokens"
            series={series.tokens}
            days={days}
          />
        ) : (
          <div className="usage-empty">
            <ChartNoAxesCombined size={28} />
            <p>{t('usage.emptyTokens')}</p>
          </div>
        )}
        <p className="usage-footnote">{t('usage.countingNote')}</p>
      </section>
      <section className="usage-card">
        <header>
          <h3>{t('usage.skills')}</h3>
          <p>{t('usage.skillsDescription')}</p>
        </header>
        {series.skills.length ? (
          <UsageChart
            key={`skills-${days[0]?.date}-${days.length}`}
            kind="skills"
            series={series.skills}
            days={days}
          />
        ) : (
          <div className="usage-empty">
            <Sparkles size={28} />
            <p>{t('usage.emptySkills')}</p>
          </div>
        )}
      </section>
      <section className="usage-card usage-sessions">
        <header>
          <h3>{t('usage.sessions')}</h3>
          <p>{t('usage.sessionsDescription')}</p>
        </header>
        {data.recentSessions.length ? (
          <div className="usage-table-scroll">
            <table>
              <thead>
                <tr>
                  <th>{t('usage.conversation')}</th>
                  <th>{t('usage.lastActive')}</th>
                  <th>{t('usage.sessionTokens')}</th>
                </tr>
              </thead>
              <tbody>
                {data.recentSessions.map((session) => (
                  <tr key={session.sessionId}>
                    <td>
                      <span title={session.title}>{session.title}</span>
                    </td>
                    <td>
                      <time dateTime={session.updatedAt}>{dateTime(session.updatedAt)}</time>
                    </td>
                    <td title={session.totalTokens.toLocaleString(locale)}>
                      {formatTokens(session.totalTokens)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <p className="usage-empty">{t('usage.emptySessions')}</p>
        )}
      </section>
      <p className="usage-history-note">
        {t('usage.historyNote')}{' '}
        <span>{t('usage.trackingSince', { date: dateTime(data.trackingStartedAt) })}</span>
      </p>
    </>
  );
}
