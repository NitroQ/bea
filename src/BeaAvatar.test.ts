import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import BeaAvatar, {
  BEA_GRID_HEIGHT,
  BEA_GRID_WIDTH,
  BEA_VARIANTS,
  beaLabel,
  beaRects,
} from './BeaAvatar';

describe('BeaAvatar pixel data', () => {
  it('exposes one variant per placement (logo + actions + emotions)', () => {
    expect(BEA_VARIANTS).toEqual([
      'logo',
      'presenting',
      'waving',
      'happy',
      'love',
      'curious',
      'celebrating',
    ]);
  });

  it('keeps every pixel inside the 48x56 grid', () => {
    for (const variant of BEA_VARIANTS) {
      for (const [x, y, w, h] of beaRects(variant)) {
        expect(x, `${variant} x`).toBeGreaterThanOrEqual(0);
        expect(y, `${variant} y`).toBeGreaterThanOrEqual(0);
        expect(w, `${variant} w`).toBeGreaterThanOrEqual(1);
        expect(h, `${variant} h`).toBeGreaterThanOrEqual(1);
        expect(x + w, `${variant} right edge`).toBeLessThanOrEqual(BEA_GRID_WIDTH);
        expect(y + h, `${variant} bottom edge`).toBeLessThanOrEqual(BEA_GRID_HEIGHT);
      }
    }
  });

  it('gives every variant a distinct silhouette', () => {
    const shapes = new Set(BEA_VARIANTS.map((variant) => JSON.stringify(beaRects(variant))));
    expect(shapes.size).toBe(BEA_VARIANTS.length);
  });

  it('labels every variant for screen readers', () => {
    for (const variant of BEA_VARIANTS) {
      expect(beaLabel(variant)).toMatch(/bea/i);
    }
  });
});

describe('BeaAvatar render', () => {
  it('renders a crisp svg per variant', () => {
    for (const variant of BEA_VARIANTS) {
      const html = renderToStaticMarkup(createElement(BeaAvatar, { variant }));
      expect(html).toContain('<svg');
      expect(html).toContain(`data-variant="${variant}"`);
      expect(html).toContain('crispEdges');
      expect(html).toContain('<rect');
    }
  });
});
