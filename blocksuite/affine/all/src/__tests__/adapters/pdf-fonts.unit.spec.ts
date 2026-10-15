import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

import { DefaultTheme, NoteDisplayMode } from '@blocksuite/affine-model';
import {
  configurePdfFontBaseUrl,
  ensurePdfFontsReady,
  PDF_FONT_CJK,
  PDF_FONT_INTER,
  PdfAdapter,
  PdfFontsUnavailableError,
  resetPdfFontsForTesting,
} from '@blocksuite/affine-shared/adapters';
import type { BlockSnapshot } from '@blocksuite/store';
import { AssetsManager, MemoryBlobCRUD } from '@blocksuite/store';
import pdfMake from 'pdfmake/build/pdfmake';
import { afterEach, describe, expect, test, vi } from 'vitest';

import { createJob } from '../utils/create-job.js';
import { getProvider } from '../utils/get-provider.js';

const CDN_HOST = 'cdn.affine.pro';

const FONT_FILES = [
  'Inter-Regular.woff2',
  'Inter-SemiBold.woff2',
  'Inter-Italic.woff2',
  'Inter-SemiBoldItalic.woff2',
  'SarasaGothicCL-Regular.ttf',
];

/**
 * The real faces the web app serves from its own origin. Vitest may run from
 * the repository root or from the package, so walk up to the checkout.
 */
function findPublicFontDir(): string {
  let directory = process.cwd();
  for (let depth = 0; depth < 6; depth++) {
    const candidate = join(
      directory,
      'packages',
      'frontend',
      'core',
      'public',
      'fonts'
    );
    if (existsSync(candidate)) {
      return candidate;
    }
    directory = dirname(directory);
  }
  throw new Error('packages/frontend/core/public/fonts not found');
}

const publicFontDir = findPublicFontDir();

function fakeFont(extension: string): ArrayBuffer {
  const magic =
    extension === '.woff2'
      ? [0x77, 0x4f, 0x46, 0x32]
      : [0x00, 0x01, 0x00, 0x00];
  return new Uint8Array([...magic, 0, 0, 0, 0]).buffer;
}

function realFont(name: string): ArrayBuffer {
  const file = readFileSync(join(publicFontDir, name));
  return file.buffer.slice(
    file.byteOffset,
    file.byteOffset + file.byteLength
  ) as ArrayBuffer;
}

/**
 * Stand in for the browser: answer the paths the deployment serves, and let
 * everything else fail the way a blocked cross-origin request does.
 */
function stubFetch(server: (url: string) => ArrayBuffer | 'html' | null) {
  const fetchMock = vi.fn(async (input: unknown) => {
    const url = String(input);
    const body = server(url);
    if (!body) {
      throw new TypeError('Failed to fetch');
    }
    if (body === 'html') {
      // A SPA fallback answers 200 text/html for a path that does not exist.
      return {
        ok: true,
        arrayBuffer: async () =>
          new TextEncoder().encode('<!doctype html>').buffer,
      };
    }
    return { ok: true, arrayBuffer: async () => body };
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

function fileNameOf(url: string): string {
  return url.split('/').pop() ?? '';
}

afterEach(() => {
  vi.unstubAllGlobals();
  resetPdfFontsForTesting();
  pdfMake.vfs = {};
  pdfMake.fonts = {};
});

describe('pdf font resolution', () => {
  test('prefers the fonts the deployment serves itself', async () => {
    const requests: string[] = [];
    stubFetch(url => {
      requests.push(url);
      if (new URL(url).host === CDN_HOST) {
        return null; // cross-origin, no Access-Control-Allow-Origin
      }
      return fileNameOf(url).endsWith('.woff2')
        ? fakeFont('.woff2')
        : fakeFont('.ttf');
    });
    configurePdfFontBaseUrl('/');

    const resolution = await ensurePdfFontsReady();

    expect(resolution.defaultFont).toBe(PDF_FONT_CJK);
    expect(resolution.monoFont).toBe(PDF_FONT_INTER);
    expect(resolution.missing).toEqual([]);
    expect(requests.filter(url => new URL(url).host === CDN_HOST)).toHaveLength(
      0
    );
    expect(Object.keys(pdfMake.vfs)).toHaveLength(FONT_FILES.length);
    for (const file of FONT_FILES) {
      expect(Object.keys(pdfMake.vfs).some(url => url.endsWith(file))).toBe(
        true
      );
    }
  });

  test('falls back to the CDN when the deployment has no font files', async () => {
    stubFetch(url =>
      new URL(url).host === CDN_HOST ? fakeFont('.ttf') : 'html'
    );
    configurePdfFontBaseUrl('/');

    const resolution = await ensurePdfFontsReady();

    expect(resolution.defaultFont).toBe(PDF_FONT_CJK);
    expect(resolution.missing).toEqual([]);
    expect(
      Object.keys(pdfMake.vfs).every(url => new URL(url).host === CDN_HOST)
    ).toBe(true);
  });

  test('degrades to Inter when the CJK face is missing', async () => {
    stubFetch(url => {
      if (new URL(url).host === CDN_HOST) {
        return null;
      }
      return fileNameOf(url) === 'SarasaGothicCL-Regular.ttf'
        ? 'html'
        : fakeFont('.woff2');
    });
    configurePdfFontBaseUrl('/');

    const resolution = await ensurePdfFontsReady();

    expect(resolution.defaultFont).toBe(PDF_FONT_INTER);
    expect(resolution.monoFont).toBe(PDF_FONT_INTER);
    expect(resolution.missing).toEqual([PDF_FONT_CJK]);
    expect(Object.keys(pdfMake.fonts)).toEqual([PDF_FONT_INTER]);
  });

  test('fails with the probed urls when nothing can be fetched', async () => {
    // Both the local path (SPA fallback) and the CDN (CORS) are unusable.
    stubFetch(url => (new URL(url).host === CDN_HOST ? null : 'html'));
    configurePdfFontBaseUrl('/');

    const error = await ensurePdfFontsReady().catch(error => error);

    expect(error).toBeInstanceOf(PdfFontsUnavailableError);
    expect(error.candidates).toContain(
      'http://localhost:3000/fonts/SarasaGothicCL-Regular.ttf'
    );
    expect(error.candidates).toContain(
      'https://cdn.affine.pro/fonts/Inter-Regular.woff'
    );
  });

  test('resolves once per base url', async () => {
    const fetchMock = stubFetch(() => fakeFont('.woff2'));
    configurePdfFontBaseUrl('/');

    const first = await ensurePdfFontsReady();
    const calls = fetchMock.mock.calls.length;
    const second = await ensurePdfFontsReady();

    expect(second).toBe(first);
    expect(fetchMock).toHaveBeenCalledTimes(calls);
    expect(calls).toBe(FONT_FILES.length);
  });
});

function createSnapshot(): BlockSnapshot {
  return {
    type: 'block',
    id: 'block:test',
    flavour: 'affine:page',
    props: {
      title: {
        '$blocksuite:internal:text$': true,
        delta: [{ insert: '字体 Fonts' }],
      },
    },
    children: [
      {
        type: 'block',
        id: 'block:surface',
        flavour: 'affine:surface',
        props: { elements: {} },
        children: [],
      },
      {
        type: 'block',
        id: 'block:note',
        flavour: 'affine:note',
        props: {
          xywh: '[0,0,800,95]',
          background: DefaultTheme.noteBackgrounColor,
          index: 'a0',
          hidden: false,
          displayMode: NoteDisplayMode.DocAndEdgeless,
        },
        children: [
          {
            type: 'block',
            id: 'block:paragraph',
            flavour: 'affine:paragraph',
            props: {
              type: 'text',
              text: {
                '$blocksuite:internal:text$': true,
                delta: [{ insert: '中文段落，with Latin text. ' }],
              },
            },
            children: [],
          },
        ],
      },
    ],
  };
}

describe('pdf export with real font files', () => {
  test('embeds the fonts served from our own origin', async () => {
    const requested: string[] = [];
    stubFetch(url => {
      requested.push(url);
      if (new URL(url).host === CDN_HOST) {
        return null;
      }
      return realFont(fileNameOf(url));
    });
    configurePdfFontBaseUrl('/');
    const job = createJob();
    const assets = new AssetsManager({ blob: new MemoryBlobCRUD() });
    const adapter = new PdfAdapter(job, getProvider());

    const { file } = await adapter.fromDocSnapshot({
      snapshot: {
        type: 'page',
        meta: {
          id: 'doc:test',
          title: 'Fonts',
          tags: [],
          createDate: 0,
        },
        blocks: createSnapshot(),
      },
      assets,
    });

    expect(requested.some(url => new URL(url).host === CDN_HOST)).toBe(false);
    const bytes = new Uint8Array(await file.blob.arrayBuffer());
    expect(new TextDecoder().decode(bytes.subarray(0, 5))).toBe('%PDF-');
    expect(file.fileName).toBe('Fonts.pdf');
  }, 30_000);
});
