import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';

import type { FontSize } from '../lib/types';
import { FontSizeSettings } from './FontSizeSettings';

describe('font size settings', () => {
  it.each(['small', 'medium', 'large'] satisfies FontSize[])(
    'shows the saved %s size and all three choices',
    (fontSize) => {
      const markup = renderToStaticMarkup(
        <FontSizeSettings fontSize={fontSize} onSaved={async () => undefined} />,
      );
      expect(markup).toContain(`<option value="${fontSize}" selected="">`);
      expect(markup).toContain('Small');
      expect(markup).toContain('Medium (default)');
      expect(markup).toContain('Large');
      expect(markup.match(/<option /g)).toHaveLength(3);
      expect(markup).toContain('Applies immediately');
    },
  );
});
