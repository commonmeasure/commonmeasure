#!/usr/bin/env python3
"""Measure tokens per unit of real content, by content type.

Answers how much of a context window different kinds of content actually
cost — prose against code against JSON against a lockfile — and how much of a
raw web page is markup rather than text. Measured against real files in this
repository plus live pages, with the tokeniser named, so it can be rerun.

Tokeniser: tiktoken o200k_base (GPT-4o/GPT-5 family). Anthropic and Google
publish no downloadable tokeniser, so absolute counts do not transfer to
those models. The *ratios between content types* are the transferable part.
"""

import json
import re
import sys
from pathlib import Path

import tiktoken

ENC = tiktoken.get_encoding("o200k_base")
REPO = Path(__file__).resolve().parents[3]


def measure(name, text, note=""):
    chars = len(text)
    words = len(text.split())
    tokens = len(ENC.encode(text))
    if chars == 0:
        return None
    return {
        "name": name,
        "chars": chars,
        "words": words,
        "tokens": tokens,
        "tokens_per_1k_chars": round(tokens / chars * 1000, 1),
        "chars_per_token": round(chars / tokens, 2),
        "tokens_per_word": round(tokens / words, 2) if words else None,
        "note": note,
    }


def strip_html(html):
    """Crude visible-text extraction: drop script/style, then all tags.

    Deliberately crude and labelled as such. A real extractor (trafilatura,
    Readability) also drops navigation and footers, so this UNDERSTATES the
    saving a proper extraction step gives you.
    """
    text = re.sub(r"(?is)<(script|style|noscript|svg)\b.*?</\1>", " ", html)
    text = re.sub(r"(?s)<!--.*?-->", " ", text)
    text = re.sub(r"(?s)<[^>]+>", " ", text)
    text = re.sub(r"&[a-zA-Z#0-9]+;", " ", text)
    text = re.sub(r"[ \t]+", " ", text)
    text = re.sub(r"\n\s*\n+", "\n\n", text)
    return text.strip()


def main():
    rows = []

    # ---- published repository files, readable by anyone with the repo.
    # The guide's table was measured on 4 August 2026 over an earlier set that
    # included unpublished files; these are published files of the same types.
    samples = [
        ("English prose (markdown)", "ARCHITECTURE.md", ""),
        ("English prose (markdown, long)", "docs/contracts/session-evidence.md", ""),
        ("Technical walkthrough (markdown + shell)", "docs/GETTING-STARTED.md", ""),
        ("Rust source", "crates/commonmeasure-http/src/message.rs", ""),
        ("Rust source (tests)", "crates/commonmeasure-http/tests/adversarial.rs", ""),
        ("JSON Schema", "schema/telemetry-event.v1.json", "tool-definition shaped"),
                ("JSON (test vectors)", "docs/contracts/source-policy-vectors.json", ""),
        ("Lockfile (TOML)", "Cargo.lock", "highly repetitive"),
    ]
    for label, rel, note in samples:
        path = REPO / rel
        if not path.exists():
            print(f"  skip (missing): {rel}", file=sys.stderr)
            continue
        text = path.read_text(errors="replace")
        row = measure(label, text, note)
        if row:
            row["source"] = rel
            rows.append(row)

    # ---- web pages fetched live, raw HTML vs crudely stripped text
    cache = Path(__file__).parent / "pages"
    if cache.is_dir():
        for f in sorted(cache.glob("*.html")):
            html = f.read_text(errors="replace")
            stripped = strip_html(html)
            raw = measure(f"Raw HTML — {f.stem}", html)
            txt = measure(f"Stripped text — {f.stem}", stripped)
            if raw and txt:
                raw["source"] = f.name
                txt["source"] = f.name
                txt["note"] = (
                    f"{100 - round(txt['tokens'] / raw['tokens'] * 100)}% fewer "
                    f"tokens than the raw HTML"
                )
                rows.append(raw)
                rows.append(txt)

    print(json.dumps(rows, indent=2))

    print("\n" + "=" * 78, file=sys.stderr)
    print(
        f"{'content':<44}{'tok/1k ch':>10}{'ch/tok':>8}{'tok/word':>10}",
        file=sys.stderr,
    )
    print("=" * 78, file=sys.stderr)
    for r in rows:
        print(
            f"{r['name'][:43]:<44}{r['tokens_per_1k_chars']:>10}"
            f"{r['chars_per_token']:>8}{str(r['tokens_per_word'] or '-'):>10}",
            file=sys.stderr,
        )


if __name__ == "__main__":
    main()
