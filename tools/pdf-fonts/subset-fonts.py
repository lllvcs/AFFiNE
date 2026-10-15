#!/usr/bin/env python3
"""Build the CJK face the PDF exporter ships.

`pdfmake` embeds one font file per family/style, and the upstream face
(`https://cdn.affine.pro/fonts/SarasaGothicCL-Regular.ttf`, 12.45 MB) is fetched
over XHR — which a self-hosted origin is not allowed to do, see
`blocksuite/affine/shared/src/adapters/pdf/pdf-fonts.ts`. So a subset is served
from our own `/fonts/` instead.

Coverage: GB2312 (7,445 simplified Chinese characters) + Latin/Latin-1/IPA,
Greek, Cyrillic, general punctuation, currency, letterlike, arrows, math,
enclosed alphanumerics, geometric shapes, CJK radicals and strokes, enclosed
CJK, CJK compatibility forms, fullwidth forms, kana and CJK compatibility
ideographs. That is every character of an ordinary simplified-Chinese document;
it does *not* cover rare/variant ideographs or traditional-only characters —
pass `--coverage gbk` (21,791 characters, ~7.0 MB) or `--coverage cjk`
(all CJK unified ideographs, ~9.2 MB) when that matters more than size.

Usage:
    python tools/pdf-fonts/subset-fonts.py --coverage gb2312
    python tools/pdf-fonts/subset-fonts.py --coverage gbk --output /tmp/x.ttf

Requires `fonttools` (`pip install fonttools brotli`).

Output is a plain TTF on purpose: fontTools' WOFF2 writer applies the glyf/loca
transform, and the fontkit copy bundled in pdfmake cannot decode `loca` from
it — such a file embeds as `TypeError: Cannot read properties of undefined
(reading 'version')` at `TTFSubset.encode`. Verified against pdfmake 0.2.20.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import urllib.request

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DEFAULT_OUTPUT = os.path.join(
    REPO_ROOT, "packages", "frontend", "core", "public", "fonts",
    "SarasaGothicCL-Regular.ttf",
)
SOURCE_URL = "https://cdn.affine.pro/fonts/SarasaGothicCL-Regular.ttf"
USER_AGENT = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
)

# Latin + Latin-1 + IPA + Greek + Cyrillic, punctuation, symbols, CJK
# punctuation, halfwidth/fullwidth forms.
SYMBOLS = (
    "U+0020-007E,U+00A0-024F,U+0370-03FF,U+0400-04FF,U+2000-206F,"
    "U+20A0-20BF,U+2100-214F,U+2190-21FF,U+2200-22FF,U+2460-24FF,"
    "U+25A0-25FF,U+2600-26FF,U+3000-303F,U+FE10-FE1F,U+FE30-FE4F,U+FF00-FFEF"
)
KANA = "U+3040-30FF,U+31F0-31FF"
CJK_SYMBOLS = "U+2E80-2EFF,U+31C0-31EF,U+3200-32FF,U+3300-33FF,U+F900-FAFF"
CJK_IDEOGRAPHS = "U+3400-4DBF,U+4E00-9FFF"


def codec_characters(codec: str) -> list[str]:
    """Every character the codec can decode, i.e. a standard's coverage."""
    characters: set[str] = set()
    for lead in range(0x81, 0x100):
        for trail in range(0x40, 0x100):
            try:
                characters.add(bytes([lead, trail]).decode(codec))
            except (UnicodeDecodeError, LookupError):
                pass
    return sorted(character for character in characters if len(character) == 1)


def to_ranges(characters: list[str]) -> str:
    codepoints = sorted({ord(character) for character in characters})
    if not codepoints:
        return ""
    ranges: list[tuple[int, int]] = []
    start = previous = codepoints[0]
    for codepoint in codepoints[1:]:
        if codepoint == previous + 1:
            previous = codepoint
            continue
        ranges.append((start, previous))
        start = previous = codepoint
    ranges.append((start, previous))
    return ",".join(
        f"U+{low:04X}" if low == high else f"U+{low:04X}-{high:04X}"
        for low, high in ranges
    )


def coverage_unicodes(coverage: str) -> str:
    if coverage == "gb2312":
        ideographs = to_ranges(codec_characters("gb2312"))
    elif coverage == "gbk":
        ideographs = to_ranges(codec_characters("gbk"))
    elif coverage == "cjk":
        ideographs = CJK_IDEOGRAPHS
    else:  # pragma: no cover - argparse restricts the values
        raise ValueError(coverage)
    return ",".join([ideographs, SYMBOLS, KANA, CJK_SYMBOLS])


def download(source: str, destination: str) -> None:
    request = urllib.request.Request(source, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=300) as response:
        payload = response.read()
    with open(destination, "wb") as handle:
        handle.write(payload)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--coverage", choices=["gb2312", "gbk", "cjk"], default="gb2312")
    parser.add_argument("--output", default=DEFAULT_OUTPUT)
    parser.add_argument(
        "--source",
        default=SOURCE_URL,
        help="full face to subset; downloaded to a temporary file when it is a URL",
    )
    args = parser.parse_args()

    workdir = os.path.dirname(os.path.abspath(args.output))
    os.makedirs(workdir, exist_ok=True)

    source = args.source
    temporary = None
    if source.startswith(("http://", "https://")):
        temporary = os.path.join(workdir, ".SarasaGothicCL-Regular.full.ttf")
        if not os.path.exists(temporary):
            print(f"downloading {source}")
            download(source, temporary)
        source = temporary

    unicodes = coverage_unicodes(args.coverage)
    listing = os.path.join(workdir, ".subset-unicodes.txt")
    with open(listing, "w", encoding="utf-8") as handle:
        handle.write(unicodes.replace(",", "\n"))

    subprocess.run(
        [
            sys.executable, "-m", "fontTools.subset", source,
            f"--unicodes-file={listing}",
            "--layout-features=*",
            "--no-hinting",
            "--desubroutinize",
            "--drop-tables+=DSIG",
            f"--output-file={args.output}",
        ],
        check=True,
    )

    for path in (listing, temporary):
        if path and os.path.exists(path):
            os.remove(path)

    size = os.path.getsize(args.output) / 1048576
    print(f"wrote {args.output} ({size:.2f} MB) with --coverage {args.coverage}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
