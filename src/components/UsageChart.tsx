import { useEffect, useState } from 'react';
import {
  Bar,
  CartesianGrid,
  ComposedChart,
  Line,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';

import { useI18n } from '../i18n';
import { formatTokens, seriesColor, usageDateLabel } from '../lib/usage';
import type { UsageDayWindow, UsageSeries } from '../lib/usage';

export interface UsageChartProps {
  days: UsageDayWindow[];
  series: UsageSeries[];
  kind: 'tokens' | 'skills';
}

export function UsageChart({ days, series, kind }: UsageChartProps) {
  const { t, locale } = useI18n();
  const [hidden, setHidden] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<string | number | undefined>();
  const visible = series
    .map((item, index) => ({ ...item, color: seriesColor(index), index }))
    .filter((item) => !hidden.has(item.id));
  const format = kind === 'tokens' ? formatTokens : (count: number) => count.toLocaleString(locale);
  const data = days.map((day, index) => ({
    date: day.date,
    values: series.map((item) => item.values[index] ?? 0),
  }));
  return (
    <div className="usage-chart">
      <p className="usage-chart-hint">{t('usage.chartHint')}</p>
      {visible.length === 0 && <p className="usage-chart-empty">{t('usage.noSeries')}</p>}
      <div className="usage-chart-scroll">
        <div className="usage-plot">
          <ResponsiveContainer
            width="100%"
            height={230}
            initialDimension={{ width: 688, height: 230 }}
          >
            <ComposedChart
              data={data}
              title={t(kind === 'tokens' ? 'usage.tokens' : 'usage.skills')}
              desc={t('usage.chartHint')}
              accessibilityLayer
              margin={{ top: 16, right: 16, bottom: 0, left: 0 }}
            >
              <CartesianGrid vertical={false} stroke="var(--line)" strokeDasharray="3 4" />
              <XAxis
                dataKey="date"
                tickFormatter={(date: string) => usageDateLabel(date, locale)}
                minTickGap={28}
                tickLine={false}
                axisLine={false}
                tick={{ fill: 'var(--muted)', fontSize: 11 }}
              />
              <YAxis
                allowDecimals={false}
                tickFormatter={format}
                tickLine={false}
                axisLine={false}
                tick={{ fill: 'var(--muted)', fontSize: 11 }}
                width={60}
              />
              {visible.map((item) =>
                kind === 'tokens' ? (
                  <Bar
                    key={item.id}
                    dataKey={(day: (typeof data)[number]) => day.values[item.index] ?? 0}
                    name={item.label}
                    stackId="tokens"
                    fill={item.color}
                    maxBarSize={36}
                    isAnimationActive={false}
                  />
                ) : (
                  <Line
                    key={item.id}
                    dataKey={(day: (typeof data)[number]) => day.values[item.index] ?? 0}
                    name={item.label}
                    stroke={item.color}
                    strokeWidth={2.5}
                    dot={{ r: 2.5, stroke: 'var(--surface)', strokeWidth: 1.5 }}
                    activeDot={{ r: 4 }}
                    strokeDasharray={item.index >= 8 ? `${3 + (item.index % 4) * 2} 3` : ''}
                    isAnimationActive={false}
                  />
                ),
              )}
              <Tooltip
                isAnimationActive={false}
                cursor={{ stroke: 'var(--muted)', strokeDasharray: '3 3', fill: 'var(--hover)' }}
                content={<SelectedDay onSelect={setSelected} />}
              />
            </ComposedChart>
          </ResponsiveContainer>
        </div>
      </div>
      <div className="usage-day-details" aria-label={t('usage.dayDetails')} aria-live="polite">
        <UsageTooltip days={days} series={visible} kind={kind} label={selected} />
      </div>
      <div
        className="usage-legend"
        aria-label={t(kind === 'tokens' ? 'usage.tokens' : 'usage.skills')}
      >
        {series.map((item, index) => (
          <button
            key={item.id}
            type="button"
            aria-pressed={!hidden.has(item.id)}
            aria-label={t('usage.showSeries', { name: item.label })}
            onClick={() =>
              setHidden((current) => {
                const next = new Set(current);
                if (next.has(item.id)) next.delete(item.id);
                else next.add(item.id);
                return next;
              })
            }
          >
            <i style={{ background: seriesColor(index) }} />
            <span>{item.label}</span>
          </button>
        ))}
        {hidden.size > 0 && (
          <button className="usage-show-all" type="button" onClick={() => setHidden(new Set())}>
            {t('usage.allSeries')}
          </button>
        )}
      </div>
    </div>
  );
}

function UsageTooltip({
  days,
  series,
  kind,
  label,
}: Omit<UsageChartProps, 'series'> & {
  series: (UsageSeries & { color: string })[];
  label?: string | number | undefined;
}) {
  const { locale } = useI18n();
  const index = days.findIndex((day) => day.date === label);
  const day = days[index];
  if (!day) return null;
  return (
    <div className="usage-tooltip">
      <strong>{usageDateLabel(day.date, locale, true)}</strong>
      <div>
        {series.map((item) => {
          const value = item.values[index] ?? 0;
          return (
            <span key={item.id}>
              <i style={{ background: item.color }} />
              {item.label}
              <b title={value.toLocaleString(locale)}>
                {kind === 'tokens' ? formatTokens(value) : value.toLocaleString(locale)}
              </b>
            </span>
          );
        })}
      </div>
    </div>
  );
}

// Keep details outside the chart's event tree so users can scroll them without
// moving the active day. Recharts still owns hit testing and keyboard navigation.
function SelectedDay({
  label,
  onSelect,
}: {
  label?: string | number;
  onSelect: (label: string | number | undefined) => void;
}) {
  useEffect(() => {
    if (label !== undefined) onSelect(label);
  }, [label, onSelect]);
  useEffect(() => {
    function dismiss(event: KeyboardEvent) {
      if (event.key === 'Escape') onSelect(undefined);
    }
    document.addEventListener('keydown', dismiss);
    return () => document.removeEventListener('keydown', dismiss);
  }, [onSelect]);
  return null;
}
