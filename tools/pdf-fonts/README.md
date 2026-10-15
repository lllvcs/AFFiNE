# PDF fonts

`pdfmake` fetches every font in `pdfMake.fonts` over `XMLHttpRequest` **before**
it starts writing the document. A font URL is therefore only usable when the
response carries `Access-Control-Allow-Origin`, and `cdn.affine.pro` only sends
that header for AFFiNE's own origins:

```console
$ curl -sI -H 'Origin: https://app.affine.pro' https://cdn.affine.pro/fonts/Inter-Regular.woff | grep -i access-control
access-control-allow-origin: https://app.affine.pro

$ curl -sI -H 'Origin: https://<your-instance>' https://cdn.affine.pro/fonts/Inter-Regular.woff | grep -i access-control
# nothing
```

On a self-hosted instance the export therefore died with
`net::ERR_FAILED` / `blocked by CORS policy` and produced no file at all.

## What this directory is for

`subset-fonts.py` builds the CJK face that ships in
`packages/frontend/core/public/fonts/SarasaGothicCL-Regular.ttf`. That directory
is copied verbatim into the web app's output (`rspack.CopyRspackPlugin` on
`@affine/core/public`) and is served from the deployment's own origin, which is
where `blocksuite/affine/shared/src/adapters/pdf/pdf-fonts.ts` now looks first.
The canvas text fonts have always been resolved that way (`environment.publicPath
+ 'fonts/'`); the PDF exporter was the last place still pointing at the CDN.

## Rebuilding

```console
pip install fonttools brotli
python tools/pdf-fonts/subset-fonts.py --coverage gb2312   # default, 2.5 MB
python tools/pdf-fonts/subset-fonts.py --coverage gbk      # 7.0 MB, + traditional
python tools/pdf-fonts/subset-fonts.py --coverage cjk      # 9.2 MB, + rare ideographs
```

The source face is `https://cdn.affine.pro/fonts/SarasaGothicCL-Regular.ttf`
(12.45 MB, SIL OFL 1.1 — the licence ships next to the subset as
`SarasaGothicCL-LICENSE.txt`). Pass `--source <path>` to reuse an already
downloaded copy.

`--coverage gb2312` is the default: GB2312 (7,445 simplified Chinese
characters) plus Latin/Latin-1/IPA, Greek, Cyrillic, punctuation, currency,
letterlike, arrows, math, enclosed alphanumerics, geometric shapes, CJK radicals
and strokes, enclosed CJK, CJK compatibility forms, fullwidth forms, kana and
CJK compatibility ideographs. It does not cover rare or variant ideographs, nor
traditional-only characters — use `gbk` or `cjk` when that matters more than the
download size.

## Why TTF and not WOFF2

WOFF2 would be roughly half the size, but fontTools' WOFF2 writer applies the
`glyf`/`loca` transform and the fontkit copy bundled in pdfmake 0.2.20 cannot
decode `loca` from it. The failure is not a graceful "unsupported format": it
throws while embedding the font, after the document has been measured.

```console
$ python tools/pdf-fonts/subset-fonts.py --coverage gb2312 --output /tmp/x.ttf
$ python - <<'PY'   # with a .woff2 output instead
TypeError: Cannot read properties of undefined (reading 'version')
    at TTFSubset.encode (.../@foliojs-fork/fontkit/index.js)
```

A WOFF2 whose outlines are CFF (such as the `Inter-*.woff2` files the canvas
already loads) is fine — the problem is specific to transformed TrueType
outlines. Keep `.ttf` here.
