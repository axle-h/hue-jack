import { describe, expect, it } from 'vitest';
import { render } from 'preact';
import { App } from './App';

describe('App', () => {
  it('renders the title', () => {
    const root = document.createElement('div');
    render(<App />, root);
    expect(root.textContent).toContain('hue-jack');
  });
});
