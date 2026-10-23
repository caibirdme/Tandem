import { useState } from 'react';

import { useI18n } from '../i18n';
import { formatTokens, seriesColor, usageDateLabel } from '../lib/usage';
import type { UsageDayWindow, UsageSeries } from '../lib/usage';

const plot = { left: 52, top: 16, width: 620, height: 180 };

export function UsageChart({
  days,
  series,
  kind,
}: {
  days: UsageDayWindow[];
  series: UsageSeries[];
  kind: 'tokens' | 'skills';
}) {
  const { t, locale } = useI18n();
  const [hidden, setHidden] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<number | null>(null);
  const visible = series
    .map((item, index) => ({ ...item, color: seriesColor(index), index }))
    .filter((item) => !hidden.has(item.id));
  const format = kind === 'tokens' ? formatTokens : (count: number) => count.toLocaleString(locale);
  const totals = days.map((_, index) =>
    visible.reduce((sum, item) => sum + (item.values[index] ?? 0), 0),
  );
  const peak = Math.max(
    1,
    ...(kind === 'tokens' ? totals : visible.flatMap((item) => item.values)),
  );
  const step = Math.max(1, 10 ** Math.floor(Math.log10(peak)));
  const max = Math.max(4, Math.ceil(peak / (step * 4)) * step * 4);
  const slot = plot.width / days.length;
  const x = (index: number) => plot.left + slot * (index + 0.5);
  const y = (value: number) => plot.top + plot.height * (1 - value / max);
  const activeDay = selected === null ? null : days[selected];
  return (
    <div className="usage-chart">
      <p className="usage-chart-hint">{t('usage.chartHint')}</p>
      {visible.length === 0 && <p className="usage-chart-empty">{t('usage.noSeries')}</p>}
      <div className="usage-chart-scroll">
        <div className="usage-plot">
          <svg viewBox="0 0 688 230" aria-hidden="true">
            {[0, 1, 2, 3, 4].map((tick) => {
              const value = (max * tick) / 4;
              return (
                <g key={tick} className="usage-grid" aria-hidden="true">
                  <line x1={plot.left} x2={plot.left + plot.width} y1={y(value)} y2={y(value)} />
                  <text x={plot.left - 10} y={y(value) + 4} textAnchor="end">
                    {format(Math.round(value))}
                  </text>
                </g>
              );
            })}
            {kind === 'tokens' &&
              days.map((day, index) => {
                let base = 0;
                return (
                  <g key={day.date} aria-hidden="true">
                    {visible.map((item) => {
                      const value = item.values[index] ?? 0;
                      const bottom = base;
                      base += value;
                      return (
                        <rect
                          key={item.id}
                          x={x(index) - slot * 0.31}
                          y={y(base)}
                          width={slot * 0.62}
                          height={(plot.height * value) / max}
                          fill={item.color}
                          rx={2}
                          opacity={selected === null || selected === index ? 1 : 0.5}
                          data-series={item.id}
                          data-value={value}
                          data-base={bottom}
                        />
                      );
                    })}
                  </g>
                );
              })}
            {kind === 'skills' &&
              visible.map((item) => (
                <g key={item.id} aria-hidden="true">
                  <polyline
                    points={item.values.map((value, index) => `${x(index)},${y(value)}`).join(' ')}
                    fill="none"
                    stroke={item.color}
                    strokeWidth={2.5}
                    strokeLinejoin="round"
                    strokeLinecap="round"
                    strokeDasharray={item.index >= 8 ? `${3 + (item.index % 4) * 2} 3` : undefined}
                  />
                  {item.values.map((value, index) => (
                    <circle
                      key={days[index]?.date}
                      cx={x(index)}
                      cy={y(value)}
                      r={selected === index ? 4 : 2.5}
                      fill={item.color}
                      stroke="var(--surface)"
                      strokeWidth={1.5}
                    />
                  ))}
                </g>
              ))}
            {activeDay && (
              <line
                className="usage-cursor"
                x1={x(selected ?? 0)}
                x2={x(selected ?? 0)}
                y1={plot.top}
                y2={plot.top + plot.height}
                aria-hidden="true"
              />
            )}
            {days.map(
              (day, index) =>
                (index === 0 ||
                  index === days.length - 1 ||
                  index % Math.ceil(days.length / 7) === 0) && (
                  <text
                    key={day.date}
                    className="usage-axis-label"
                    x={x(index)}
                    y={219}
                    textAnchor="middle"
                  >
                    {usageDateLabel(day.date, locale)}
                  </text>
                ),
            )}
          </svg>
          <div className="usage-plot-interaction">
            {days.map((day, index) => (
              <button
                key={day.date}
                type="button"
                className="usage-hit-target"
                aria-label={`${usageDateLabel(day.date, locale, true)}. ${visible.map((item) => `${item.label}: ${(item.values[index] ?? 0).toLocaleString(locale)}`).join(', ')}`}
                onMouseEnter={() => setSelected(index)}
                onFocus={() => setSelected(index)}
                onClick={() => setSelected(index)}
                onKeyDown={(event) => {
                  if (event.key === 'Escape') setSelected(null);
                }}
              />
            ))}
          </div>
        </div>
      </div>
      <div className={`usage-tooltip${activeDay ? ' visible' : ''}`} aria-live="polite">
        {activeDay ? (
          <>
            <strong>{usageDateLabel(activeDay.date, locale, true)}</strong>
            <div>
              {visible.map((item) => (
                <span key={item.id}>
                  <i style={{ background: item.color }} />
                  {item.label}
                  <b title={(item.values[selected ?? 0] ?? 0).toLocaleString(locale)}>
                    {format(item.values[selected ?? 0] ?? 0)}
                  </b>
                </span>
              ))}
            </div>
          </>
        ) : (
          <span>{t('usage.dayDetails')}</span>
        )}
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
