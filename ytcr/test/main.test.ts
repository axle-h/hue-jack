import { describe, expect, it } from 'vitest';
import { NAME } from '../src/main.js';

describe('ytcr', () => {
  it('has the device name', () => {
    expect(NAME).toBe('hue-jack');
  });
});
