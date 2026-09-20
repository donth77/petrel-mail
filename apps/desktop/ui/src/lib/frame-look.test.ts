import { describe, expect, it } from 'vitest';
import { frameLook, frameUrl } from './frame-look';

const TEAL = { dark: false, forceLight: false, accent: '#0E7C86' };

describe('frameLook', () => {
  it('says which palette the app is wearing', () => {
    expect(frameLook(TEAL)).toContain('theme=light');
    expect(frameLook({ ...TEAL, dark: true })).toContain('theme=dark');
  });

  it('asks for light only when this message was forced light', () => {
    expect(frameLook(TEAL)).not.toContain('force=');
    expect(frameLook({ ...TEAL, forceLight: true })).toContain('force=light');
  });

  it('drops the hash, which would start a fragment', () => {
    expect(frameLook(TEAL)).toContain('accent=0E7C86');
    expect(frameLook(TEAL)).not.toContain('#');
    expect(frameLook({ ...TEAL, accent: '0E7C86' })).toContain('accent=0E7C86');
  });

  /// The regression. The accent was baked into the frame at birth but left out
  /// of what the app watched, so changing it in Settings repainted the whole
  /// window and left the open message on the old ground.
  it('changes when only the accent changes', () => {
    expect(frameLook({ ...TEAL, dark: true })).not.toBe(
      frameLook({ ...TEAL, dark: true, accent: '#9A6B1F' }),
    );
  });
});

describe('frameUrl', () => {
  it('opens a query, or joins the one the body URL already has', () => {
    expect(frameUrl('petrel-msg://m/7', 'theme=dark')).toBe('petrel-msg://m/7?theme=dark');
    expect(frameUrl('./msg.html?blocked=0', 'theme=dark')).toBe('./msg.html?blocked=0&theme=dark');
  });
});
