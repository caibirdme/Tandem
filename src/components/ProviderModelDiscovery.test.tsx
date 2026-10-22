import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';

import { initialCredentialEditor } from '../lib/credentialEditor';
import { ProviderModelDiscovery } from './ProviderModelDiscovery';

function markup(saved = false, blank = false) {
  return renderToStaticMarkup(
    <ProviderModelDiscovery
      baseUrl="https://example.com"
      listModelsPath="/v1/models"
      protocol="anthropic"
      credential={{ ...initialCredentialEditor(saved && !blank), modified: blank }}
      hasSavedKey={saved}
      disabled={false}
      existingIds={[]}
      onAdd={() => undefined}
    />,
  );
}

describe('discovery controls', () => {
  it('requires an explicit fetch action and initially renders no selected or added models', () => {
    const rendered = markup();
    expect(rendered).toContain('Fetch models');
    expect(rendered).toContain('type="button"');
    expect(rendered).not.toContain('type="checkbox"');
    expect(rendered).not.toContain('Add selected');
    expect(rendered).not.toContain('disabled');
  });
  it('requires explicit key intent after blanking a saved credential', () => {
    const rendered = markup(true, true);
    expect(rendered).toContain('disabled=""');
    expect(rendered).toContain('Enter an API key or choose Clear saved key before fetching');
  });
  it('disables fetching while a saved credential is loading', () => {
    const rendered = markup(true);
    expect(rendered).toContain('disabled=""');
    expect(rendered).toContain('Wait for the saved API key to load');
  });
});
