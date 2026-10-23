// @vitest-environment jsdom
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import type { Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { setLocale } from '../i18n';
import { usageDayWindows } from '../lib/usage';
import { UsageChart } from './UsageChart';

let container: HTMLDivElement;
let root: Root;
const days = usageDayWindows(7, new Date(2026, 9, 22));

beforeEach(() => {
  vi.stubGlobal('IS_REACT_ACT_ENVIRONMENT', true);
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  // jsdom has no layout engine; keep only measurements deterministic, not chart behavior.
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
    x: 0,
    y: 0,
    left: 0,
    top: 0,
    right: 688,
    bottom: 230,
    width: 688,
    height: 230,
    toJSON: () => ({}),
  });
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(() => root.unmount());
  container.remove();
  setLocale('en');
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
function element<T extends Element>(selector: string): T {
  const result = container.querySelector<T>(selector);
  if (!result) throw new Error(`Missing element: ${selector}`);
  return result;
}
async function pressKey(key: string) {
  await act(() =>
    element<SVGSVGElement>('.recharts-surface').dispatchEvent(
      new KeyboardEvent('keydown', { key, bubbles: true }),
    ),
  );
}
async function click(selector: string) {
  await act(() => element<HTMLButtonElement>(selector).click());
}

describe('usage chart interactions', () => {
  it('stacks actual values and announces compact and exact daily totals with arrow navigation', async () => {
    await act(() =>
      root.render(
        <UsageChart
          days={days}
          kind="tokens"
          series={[
            {
              id: 'provider.model[0]',
              label: 'Provider / model.a[0]',
              values: days.map((_, i) => 1_234_000 + i),
            },
            { id: 'other', label: 'Other model', values: days.map(() => 1000) },
          ]}
        />,
      ),
    );
    expect(container.querySelectorAll('.recharts-bar-rectangle')).toHaveLength(14);
    const bars = container.querySelectorAll('.recharts-bar');
    const lower = bars[0]?.querySelector('path');
    const upper = bars[1]?.querySelector('path');
    expect(Number(lower?.getAttribute('height'))).toBeGreaterThan(0);
    expect(Number(upper?.getAttribute('y')) + Number(upper?.getAttribute('height'))).toBeCloseTo(
      Number(lower?.getAttribute('y')),
    );
    expect(element('.recharts-surface').getAttribute('role')).toBe('application');
    expect(element('.recharts-surface title').textContent).toBe('Token usage');
    await act(() => element<SVGSVGElement>('.recharts-surface').focus());
    await pressKey('ArrowRight');
    expect(element('.usage-tooltip').textContent).toContain('1.23M');
    expect(element('.usage-tooltip').textContent).toContain('1K');
    expect(element('.usage-tooltip b').getAttribute('title')).toBe('1,234,001');
    await pressKey('ArrowRight');
    expect(element('.usage-tooltip b').getAttribute('title')).toBe('1,234,002');
    expect(element('.usage-tooltip').closest('.usage-day-details')).not.toBeNull();
    await pressKey('Escape');
    expect(container.querySelector('.usage-tooltip')).toBeNull();
    await pressKey('ArrowLeft');
    expect(element('.usage-tooltip b').getAttribute('title')).toBe('1,234,001');
  });

  it('toggles series with native buttons and restores all hidden series', async () => {
    await act(() =>
      root.render(
        <UsageChart
          days={days}
          kind="skills"
          series={[{ id: 'skill', label: 'Review', values: days.map(() => 3) }]}
        />,
      ),
    );
    expect(container.querySelectorAll('.recharts-line-curve')).toHaveLength(1);
    await click('.usage-legend button');
    expect(element('.usage-legend button').getAttribute('aria-pressed')).toBe('false');
    expect(container.querySelectorAll('.recharts-line-curve')).toHaveLength(0);
    expect(element('.usage-chart-empty').textContent).toContain('Select a series');
    await click('.usage-show-all');
    expect(container.querySelectorAll('.recharts-line-curve')).toHaveLength(1);
    expect(element('.usage-legend button').getAttribute('aria-pressed')).toBe('true');
  });

  it.each([7, 14, 30] as const)(
    'keeps every day and all skill details available for %i days',
    async (range) => {
      const rangeDays = usageDayWindows(range, new Date(2026, 9, 22));
      await act(() =>
        root.render(
          <UsageChart
            days={rangeDays}
            kind="skills"
            series={Array.from({ length: 40 }, (_, i) => ({
              id: `skill.${i}`,
              label: `Skill ${i}`,
              values: rangeDays.map((_day, day) => i + day),
            }))}
          />,
        ),
      );
      expect(container.querySelectorAll('.recharts-line-curve')).toHaveLength(40);
      expect(container.querySelectorAll('.recharts-line-dot')).toHaveLength(40 * range);
      await act(async () => {
        element('.recharts-wrapper').dispatchEvent(
          new MouseEvent('mousemove', { bubbles: true, clientX: 120, clientY: 80 }),
        );
        await new Promise((resolve) => requestAnimationFrame(resolve));
      });
      expect(container.querySelector('.usage-tooltip')).not.toBeNull();
      const values = element<HTMLDivElement>('.usage-tooltip > div');
      await act(() => {
        element('.recharts-wrapper').dispatchEvent(
          new MouseEvent('mouseout', { bubbles: true, relatedTarget: values }),
        );
      });
      expect(container.querySelector('.usage-tooltip')).not.toBeNull();
      expect(values.closest('.usage-day-details')).not.toBeNull();

      await act(() => element<SVGSVGElement>('.recharts-surface').focus());
      await act(() => {
        for (let i = 1; i < range; i++) {
          element<SVGSVGElement>('.recharts-surface').dispatchEvent(
            new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }),
          );
        }
      });
      expect(container.querySelectorAll('.usage-tooltip b')).toHaveLength(40);
      expect(element('.usage-tooltip b').textContent).toBe(String(range - 1));
      expect(element('.usage-tooltip strong').textContent).toContain('Oct 22');
      await act(() => setLocale('zh-CN'));
      expect(element('.usage-chart-hint').textContent).toContain('左右方向键');
      expect(element('.usage-tooltip strong').textContent).toContain('10月22日');
    },
  );
});
