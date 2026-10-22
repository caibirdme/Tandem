import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import { usageDayWindows } from '../lib/usage';
import type { UsageDashboardData } from '../lib/usage';
import { UsageChart } from './UsageChart';
import { UsageDashboard, UsageDashboardContent } from './UsageDashboard';

const days = usageDayWindows(7, new Date(2026, 9, 22));
const empty: UsageDashboardData = {
  trackingStartedAt: '2026-10-20T00:00:00Z',
  dailyTokens: [],
  dailySkills: [],
  recentSessions: [],
  totalTokens: 0,
  skillInvocations: 0,
};
afterEach(() => setLocale('en'));

describe('usage dashboard', () => {
  it('offers the supported ranges and an accessible initial loading state', () => {
    const markup = renderToStaticMarkup(<UsageDashboard />);
    expect(markup).toContain('aria-busy="true"');
    expect(markup).toContain('<output');
    for (const range of [7, 14, 30]) expect(markup).toContain(`Last ${range} days`);
    expect(markup).toContain('aria-label="Refresh usage"');
  });
  it('shows honest empty states and the history boundary, not fake sample data', () => {
    const markup = renderToStaticMarkup(<UsageDashboardContent data={empty} days={days} />);
    expect(markup).toContain('No token usage recorded');
    expect(markup).toContain('No skill activations recorded');
    expect(markup).toContain('No recorded conversations');
    expect(markup).toContain('Earlier activity cannot be reconstructed');
    expect(markup).not.toContain('<svg viewBox=');
  });
  it('renders provider/model stacks, actual skill series and range-labelled conversation totals', () => {
    const data: UsageDashboardData = {
      ...empty,
      totalTokens: 1_234_000,
      skillInvocations: 3,
      dailyTokens: [
        {
          date: '2026-10-22',
          providerId: 'p',
          providerName: 'My provider',
          modelId: 'model-a',
          inputTokens: 1_000_000,
          outputTokens: 234_000,
          totalTokens: 1_234_000,
        },
      ],
      dailySkills: [{ date: '2026-10-22', skillName: 'code-review', count: 3 }],
      recentSessions: [
        {
          sessionId: 's',
          title: 'Build dashboard',
          updatedAt: '2026-10-22T12:00:00Z',
          totalTokens: 1_234_000,
        },
      ],
    };
    const markup = renderToStaticMarkup(<UsageDashboardContent data={data} days={days} />);
    expect(markup).toContain('1.23M');
    expect(markup).toContain('My provider / model-a');
    expect(markup).toContain('code-review');
    expect(markup).toContain('Build dashboard');
    expect(markup).toContain('Tokens in period');
    expect(markup).toContain('data-value="1234000"');
    expect(markup).toContain('<polyline');
    expect(markup.match(/class="usage-hit-target"/g)).toHaveLength(14);
  });
  it('translates the page and empty states into Chinese', () => {
    setLocale('zh-CN');
    const markup = renderToStaticMarkup(<UsageDashboardContent data={empty} days={days} />);
    expect(markup).toContain('Token 使用量');
    expect(markup).toContain('此时间范围内尚无 Skill 激活记录');
    expect(markup).not.toContain('No token usage');
  });
  it('provides keyboard-operable legend controls and accessible day values', () => {
    const markup = renderToStaticMarkup(
      <UsageChart
        days={days}
        kind="skills"
        series={[{ id: 'skill', label: 'Review', values: [0, 0, 0, 0, 0, 0, 3] }]}
      />,
    );
    expect(markup).toContain('aria-pressed="true"');
    expect(markup).toContain('aria-label="Show or hide Review"');
    expect(markup).toContain('Review: 3');
    expect(markup).toContain('aria-live="polite"');
  });
});
