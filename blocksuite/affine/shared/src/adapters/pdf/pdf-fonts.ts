import pdfMake from 'pdfmake/build/pdfmake';
import type { TFontDictionary } from 'pdfmake/interfaces';

/**
 * Font resolution for the PDF exporter.
 *
 * `pdfmake` fetches every font entry over `XMLHttpRequest` before it starts
 * writing the document, so a font URL is only usable when the response carries
 * `Access-Control-Allow-Origin`. `cdn.affine.pro` only sends that header for
 * AFFiNE's own origins: a request carrying `Origin: https://<self-hosted host>`
 * comes back without it, and the export dies with
 *
 *     Access to XMLHttpRequest at 'https://cdn.affine.pro/fonts/Inter-Regular.woff'
 *     from origin 'https://<host>' has been blocked by CORS policy
 *
 * So the fonts are resolved against the deployment's own asset root first
 * (`<publicPath>/fonts/<file>`, the same place the canvas text fonts already
 * come from) and only then against the CDN. A missing font is not fatal: the
 * family falls back to the next one, and finally to a pdfkit standard font that
 * needs no file at all, so an export always produces a document.
 */

export type PdfFontSlot = 'normal' | 'bold' | 'italics' | 'bolditalics';

type PdfFontFile = {
  /** file name below `<publicPath>/fonts/`, served by our own origin */
  readonly local: string;
  /** file name below `https://cdn.affine.pro/fonts/` */
  readonly remote: string;
};

const INTER_FILE: Record<PdfFontSlot, PdfFontFile> = {
  normal: { local: 'Inter-Regular.woff2', remote: 'Inter-Regular.woff' },
  bold: { local: 'Inter-SemiBold.woff2', remote: 'Inter-SemiBold.woff' },
  italics: { local: 'Inter-Italic.woff2', remote: 'Inter-Italic.woff' },
  bolditalics: {
    local: 'Inter-SemiBoldItalic.woff2',
    remote: 'Inter-SemiBoldItalic.woff',
  },
};

/**
 * One CJK face for all four slots, mirroring upstream. The file shipped in
 * `packages/frontend/core/public/fonts/` is a subset of Sarasa Gothic CL
 * covering GB2312 plus Latin, kana and CJK punctuation; see the README next to
 * it. Listed under exactly the CDN's file name so that a deployment which still
 * serves the full face from the CDN (or an operator who drops the full file in
 * place) resolves to the same URL without a code change.
 */
const SARASA_FILE: Record<PdfFontSlot, PdfFontFile> = {
  normal: {
    local: 'SarasaGothicCL-Regular.ttf',
    remote: 'SarasaGothicCL-Regular.ttf',
  },
  bold: {
    local: 'SarasaGothicCL-Regular.ttf',
    remote: 'SarasaGothicCL-Regular.ttf',
  },
  italics: {
    local: 'SarasaGothicCL-Regular.ttf',
    remote: 'SarasaGothicCL-Regular.ttf',
  },
  bolditalics: {
    local: 'SarasaGothicCL-Regular.ttf',
    remote: 'SarasaGothicCL-Regular.ttf',
  },
};

/** Family used for body text; falls back to {@link PDF_FONT_INTER}. */
export const PDF_FONT_CJK = 'SarasaGothicCL';
/** Family used for code blocks; also the fallback for body text. */
export const PDF_FONT_INTER = 'Inter';

const FONT_DIR = 'fonts/';
const CDN_FONT_ROOT = 'https://cdn.affine.pro/';

let configuredBaseUrl: string | null = null;
let resolvedFonts: Promise<PdfFontResolution> | null = null;
const filePromises = new Map<string, Promise<string | null>>();

export interface PdfFontResolution {
  /** family for `defaultStyle.font` */
  defaultFont: string;
  /** family for the `code` style */
  monoFont: string;
  /** families that could not be fetched from any candidate */
  missing: string[];
}

/** No family could be embedded; the message lists every URL that was tried. */
export class PdfFontsUnavailableError extends Error {
  readonly candidates: string[];

  constructor(candidates: string[]) {
    super(
      `PDF export needs a font file, and none of these could be loaded: ${candidates.join(', ')}. ` +
        `A self-hosted deployment serves them from its own origin — check that ` +
        `${FONT_DIR}* is present in the web app's output.`
    );
    this.name = 'PdfFontsUnavailableError';
    this.candidates = candidates;
  }
}

/**
 * Point the resolver at the deployment's asset root (`environment.publicPath`),
 * e.g. `/` for a self-hosted instance or `https://cdn.example.com/` for a build
 * that ships its assets elsewhere. `null` restores the origin-root default.
 */
export function configurePdfFontBaseUrl(baseUrl: string | null): void {
  if (baseUrl === configuredBaseUrl) {
    return;
  }
  configuredBaseUrl = baseUrl;
  resolvedFonts = null;
  filePromises.clear();
}

function documentBaseUri(): string | undefined {
  return typeof document === 'undefined' ? undefined : document.baseURI;
}

function toAbsolute(url: string, base: string | undefined): string | null {
  try {
    return new URL(url, base).href;
  } catch {
    return null;
  }
}

/**
 * The app's own asset root. A deep link such as `/workspace/x/doc/y` is served
 * the SPA shell, so the origin root is used rather than the document URL.
 */
function appRoot(): string | null {
  if (configuredBaseUrl) {
    return toAbsolute(configuredBaseUrl, documentBaseUri());
  }
  if (typeof location !== 'undefined' && location.origin) {
    return toAbsolute('/', location.origin);
  }
  return null;
}

function candidatesFor(file: PdfFontFile): string[] {
  const candidates: string[] = [];
  const root = appRoot();
  const local = root && toAbsolute(FONT_DIR + file.local, root);
  if (local) {
    candidates.push(local);
  }
  const remote = toAbsolute(FONT_DIR + file.remote, CDN_FONT_ROOT);
  if (remote && !candidates.includes(remote)) {
    candidates.push(remote);
  }
  return candidates;
}

/**
 * A SPA fallback answers `200 text/html` for a path that does not exist, so a
 * successful response is not enough — the payload has to be a font.
 */
function isFontBuffer(buffer: ArrayBuffer): boolean {
  if (buffer.byteLength < 4) {
    return false;
  }
  const head = new Uint8Array(buffer, 0, 4);
  const tag = String.fromCharCode(head[0], head[1], head[2], head[3]);
  return (
    tag === 'wOFF' || // WOFF
    tag === 'wOF2' || // WOFF2
    tag === 'true' || // TrueType
    tag === 'OTTO' || // CFF
    (head[0] === 0x00 &&
      head[1] === 0x01 &&
      head[2] === 0x00 &&
      head[3] === 0x00)
  );
}

function toBase64(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  const chunkSize = 0x8000;
  let binary = '';
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(
      ...bytes.subarray(offset, offset + chunkSize)
    );
  }
  return btoa(binary);
}

async function fetchIntoVfs(url: string): Promise<string | null> {
  if (typeof fetch !== 'function') {
    return null;
  }
  try {
    const response = await fetch(url, { credentials: 'omit' });
    if (!response.ok) {
      return null;
    }
    const buffer = await response.arrayBuffer();
    if (!isFontBuffer(buffer)) {
      return null;
    }
    // pdfmake reads a http(s) entry out of the virtual file system before it
    // falls back to the network, so the document is built from these bytes.
    // `vfs` only exists once something assigns it — the library ships it empty
    // unless `vfs_fonts` is loaded.
    const vfs: Record<string, string> = pdfMake.vfs ?? {};
    vfs[url] = toBase64(buffer);
    pdfMake.vfs = vfs;
    return url;
  } catch {
    return null;
  }
}

async function loadFontFile(file: PdfFontFile): Promise<string | null> {
  for (const url of candidatesFor(file)) {
    let pending = filePromises.get(url);
    if (!pending) {
      pending = fetchIntoVfs(url);
      filePromises.set(url, pending);
    }
    const resolved = await pending;
    if (resolved) {
      return resolved;
    }
  }
  return null;
}

async function resolveFamily(
  name: string,
  files: Record<PdfFontSlot, PdfFontFile>,
  missing: string[]
): Promise<Record<PdfFontSlot, string> | null> {
  const slots = Object.keys(files) as PdfFontSlot[];
  const entries = await Promise.all(
    slots.map(async slot => [slot, await loadFontFile(files[slot])] as const)
  );
  const resolved = Object.fromEntries(entries) as Record<
    PdfFontSlot,
    string | null
  >;
  if (slots.some(slot => !resolved[slot])) {
    missing.push(name);
    return null;
  }
  return resolved as Record<PdfFontSlot, string>;
}

async function resolveFonts(): Promise<PdfFontResolution> {
  const missing: string[] = [];
  const [inter, cjk] = await Promise.all([
    resolveFamily(PDF_FONT_INTER, INTER_FILE, missing),
    resolveFamily(PDF_FONT_CJK, SARASA_FILE, missing),
  ]);

  if (!inter && !cjk) {
    // pdfkit's standard fonts are not usable here: the browser build of
    // pdfmake looks up their AFM metrics in its virtual file system and they
    // are only shipped with the Node build. Nothing can be embedded.
    throw new PdfFontsUnavailableError(
      [...Object.values(INTER_FILE), ...Object.values(SARASA_FILE)].flatMap(
        file => candidatesFor(file)
      )
    );
  }

  const fonts: TFontDictionary = {};
  if (inter) {
    fonts[PDF_FONT_INTER] = inter;
  }
  if (cjk) {
    fonts[PDF_FONT_CJK] = cjk;
  }
  pdfMake.fonts = fonts;

  const resolution: PdfFontResolution = {
    defaultFont: cjk ? PDF_FONT_CJK : PDF_FONT_INTER,
    monoFont: inter ? PDF_FONT_INTER : PDF_FONT_CJK,
    missing,
  };
  if (missing.length) {
    console.warn(
      `[pdf] font files unavailable, falling back to ${resolution.defaultFont}: ${missing.join(', ')}`
    );
  }
  return resolution;
}

/**
 * Make `pdfMake.fonts` usable and report the family the document definition
 * should default to. Resolved once per base URL, then replayed from the virtual
 * file system.
 */
export function ensurePdfFontsReady(): Promise<PdfFontResolution> {
  if (!resolvedFonts) {
    resolvedFonts = resolveFonts().catch(error => {
      resolvedFonts = null;
      throw error;
    });
  }
  return resolvedFonts;
}

/** Exported for tests: drop every cached probe and resolution. */
export function resetPdfFontsForTesting(): void {
  configuredBaseUrl = null;
  resolvedFonts = null;
  filePromises.clear();
}
