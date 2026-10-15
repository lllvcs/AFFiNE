import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

const { loadFonts, setCompilerInitOptions, setRendererInitOptions, svg } =
  vi.hoisted(() => ({
    loadFonts: vi.fn((fontUrls: string[]) => ({ fontUrls })),
    setCompilerInitOptions: vi.fn(),
    setRendererInitOptions: vi.fn(),
    svg: vi.fn(),
  }));

vi.mock('@myriaddreamin/typst.ts', () => ({
  $typst: {
    setCompilerInitOptions,
    setRendererInitOptions,
    svg,
  },
  loadFonts,
}));

import {
  createTypstFontFetcher,
  ensureTypstReady,
  renderTypstSvgWithOptions,
} from './runtime';

describe('typst runtime', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    svg.mockResolvedValue('<svg />');
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  test('reconfigures typst when fontUrls change', async () => {
    await ensureTypstReady(['font-a']);
    await ensureTypstReady(['font-b']);

    expect(loadFonts).toHaveBeenNthCalledWith(
      1,
      ['font-a'],
      expect.any(Object)
    );
    expect(loadFonts).toHaveBeenNthCalledWith(
      2,
      ['font-b'],
      expect.any(Object)
    );
    expect(setCompilerInitOptions).toHaveBeenCalledTimes(2);
    expect(setRendererInitOptions).toHaveBeenCalledTimes(2);
  });

  test('serializes typst renders that need different configuration', async () => {
    const events: string[] = [];
    let releaseFirstRender!: () => void;

    svg.mockImplementationOnce(async () => {
      events.push('svg:first:start');
      await new Promise<void>(resolve => {
        releaseFirstRender = resolve;
      });
      events.push('svg:first:end');
      return '<svg>first</svg>';
    });
    svg.mockImplementationOnce(async () => {
      events.push('svg:second:start');
      return '<svg>second</svg>';
    });

    const first = renderTypstSvgWithOptions('= First', {
      fontUrls: ['font-a'],
    });
    const second = renderTypstSvgWithOptions('= Second', {
      fontUrls: ['font-b'],
    });

    await vi.waitFor(() => {
      expect(events).toEqual(['svg:first:start']);
    });

    releaseFirstRender();

    await expect(first).resolves.toEqual({ svg: '<svg>first</svg>' });
    await expect(second).resolves.toEqual({ svg: '<svg>second</svg>' });
    expect(events).toEqual([
      'svg:first:start',
      'svg:first:end',
      'svg:second:start',
    ]);
  });
});

describe('typst font fetcher', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  test('falls back to the fonts the deployment serves itself', async () => {
    vi.stubGlobal('location', {
      href: 'https://note.example.com/js/app.js',
      origin: 'https://note.example.com',
    });
    const requested: string[] = [];
    const baseFetcher = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      requested.push(url);
      if (new URL(url).host === 'cdn.affine.pro') {
        // A cross-origin font without Access-Control-Allow-Origin.
        throw new TypeError('Failed to fetch');
      }
      return { ok: true, url } as unknown as Response;
    });
    const fetcher = createTypstFontFetcher(baseFetcher as typeof fetch);

    const cjk = await fetcher(
      'https://cdn.affine.pro/fonts/SarasaGothicCL-Regular.ttf'
    );
    const inter = await fetcher(
      'https://cdn.affine.pro/fonts/Inter-Regular.woff'
    );

    // The CJK face keeps its own name: Inter has no CJK glyphs, so falling back
    // to it would render the source block as boxes.
    expect((cjk as unknown as { url: string }).url).toBe(
      'https://note.example.com/fonts/SarasaGothicCL-Regular.ttf'
    );
    expect((inter as unknown as { url: string }).url).toBe(
      'https://note.example.com/fonts/Inter-Regular.woff2'
    );
  });

  test('keeps the original response when it is served', async () => {
    vi.stubGlobal('location', {
      href: 'https://note.example.com/js/app.js',
      origin: 'https://note.example.com',
    });
    const served = {
      ok: true,
      url: 'https://cdn.affine.pro/fonts/Inter-Regular.woff',
    };
    const baseFetcher = vi.fn(async () => served as unknown as Response);
    const fetcher = createTypstFontFetcher(baseFetcher as typeof fetch);

    const response = await fetcher(
      'https://cdn.affine.pro/fonts/Inter-Regular.woff'
    );

    expect(response).toBe(served);
    expect(baseFetcher).toHaveBeenCalledTimes(1);
  });
});
