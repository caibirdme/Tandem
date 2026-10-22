import type { ComponentProps } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';

import { SkillPicker } from './SkillPicker';

const props: ComponentProps<typeof SkillPicker> = {
  id: 'skills',
  skills: [
    {
      name: 'review',
      description: 'Review project changes',
      path: '/project/.agents/skills/review/SKILL.md',
    },
  ],
  index: 0,
  loading: false,
  connected: true,
  filtered: false,
  onIndex: () => undefined,
  onSelect: () => undefined,
  onBlur: () => undefined,
};

describe('project skill picker', () => {
  it('exposes a named native listbox with selected skill details', () => {
    const markup = renderToStaticMarkup(<SkillPicker {...props} />);
    expect(markup).toContain('<select');
    expect(markup).toContain('aria-label="Project skills"');
    expect(markup).toContain('aria-selected="true"');
    expect(markup).toContain('$review');
    expect(markup).toContain('Review project changes');
    expect(markup).toContain('/project/.agents/skills/review/SKILL.md');
  });

  it.each([
    { override: { loading: true }, message: 'Loading project skills…' },
    { override: { connected: false }, message: 'Reconnect to load project skills.' },
    { override: { skills: [] }, message: 'No project skills found.' },
    {
      override: { skills: [], filtered: true },
      message: 'No skills match this prefix.',
    },
  ])('announces $message', ({ override, message }) => {
    const markup = renderToStaticMarkup(<SkillPicker {...props} {...override} />);
    expect(markup).toContain(`<output class="skill-picker-status">`);
    expect(markup).toContain(message);
    expect(markup).not.toContain('$review');
    expect(markup).not.toContain('Retry');
  });
});

it('explains deferred remote discovery instead of claiming the project has no skills', () => {
  const markup = renderToStaticMarkup(
    <SkillPicker {...props} skills={[]} remotePreparationPending />,
  );
  expect(markup).toContain(
    'Remote skills become available after your first message prepares the environment.',
  );
  expect(markup).not.toContain('No project skills found.');
});
