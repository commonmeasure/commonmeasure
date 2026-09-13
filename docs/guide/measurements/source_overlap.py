#!/usr/bin/env python3
"""Measure how much three open-web search providers agree with each other.

Answers a practical question for anyone choosing providers: how many distinct
hosts does a search return, how much do two providers overlap, and how much
does a second provider actually add. Results are compared by host rather than
URL, because two providers returning different pages from the same site are
not independent evidence.

Search only. No `contents` on Exa, no `scrapeOptions` on Firecrawl, so no
page-content or scrape charges are incurred — the same reasoning as the
comment in `crates/commonmeasure-supply/src/firecrawl.rs`. Request shapes follow the
live-verified adapters in `crates/commonmeasure-supply/`, so this measures the same
provider behaviour the product sees.

    python3 docs/guide/measurements/source_overlap.py --pilot
    python3 docs/guide/measurements/source_overlap.py --out results.json

Reads EXA_API_KEY, TAVILY_API_KEY and FIRECRAWL_API_KEY from the environment
or from `.env` at the repository root. A provider whose key is absent is
reported as unavailable and excluded — it is never silently skipped, and a
failed request is never counted as an empty result set.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
import time
import urllib.error
import urllib.request
from collections import Counter
from pathlib import Path
from urllib.parse import urlsplit

REPO = Path(__file__).resolve().parents[3]
LIMIT = 10

# Twenty-four jobs an agent might plausibly be given, spread across the kinds
# of query that stress retrieval differently. Frozen here so a rerun measures
# providers rather than a new question set.
QUERIES: list[tuple[str, str]] = [
    ("factual", "what is the maximum context window of Claude Opus 4.5"),
    ("factual", "how does rotary position embedding work"),
    ("factual", "UK corporation tax rate for small profits 2026"),
    ("factual", "who wrote the paper introducing the transformer architecture"),
    ("fresh", "EU AI Act general purpose model obligations latest guidance"),
    ("fresh", "Bank of England base rate decision this month"),
    ("fresh", "OpenAI model releases 2026"),
    ("fresh", "latest developments in AI content licensing deals"),
    ("technical", "vLLM automatic prefix caching configuration"),
    ("technical", "how to reduce MCP tool definition token usage"),
    ("technical", "Rust async trait object safety workarounds"),
    ("technical", "postgres logical replication slot disk usage growth"),
    ("commercial", "best reranking API pricing comparison"),
    ("commercial", "vector database managed hosting pricing"),
    ("commercial", "enterprise search vendors for regulated industries"),
    ("commercial", "cost of licensed news content for AI training"),
    ("vague", "why is my agent getting worse over long conversations"),
    ("vague", "how much context is too much"),
    ("vague", "making retrieval better"),
    ("vague", "context engineering best practices"),
    ("analytical", "evidence that retrieval augmented generation reduces hallucination"),
    ("analytical", "compare long context models against retrieval on cost"),
    ("analytical", "criticism of needle in a haystack benchmarks"),
    ("analytical", "does prompt caching change optimal context ordering"),
]


def load_env() -> None:
    """Populate os.environ from the repository `.env`, without overriding."""
    env_file = REPO / ".env"
    if not env_file.is_file():
        return
    for line in env_file.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        os.environ.setdefault(key.strip(), value.strip().strip('"').strip("'"))


def post(url: str, body: dict, headers: dict, timeout: int = 45) -> dict:
    payload = json.dumps(body).encode()
    request = urllib.request.Request(url, data=payload, method="POST")
    request.add_header("Content-Type", "application/json")
    request.add_header("Accept", "application/json")
    for name, value in headers.items():
        request.add_header(name, value)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.loads(response.read().decode())


def exa(query: str, key: str) -> list[str]:
    # No `contents` block: URLs only, so no per-page content charge.
    data = post(
        "https://api.exa.ai/search",
        {"query": query, "numResults": LIMIT},
        {"x-api-key": key},
    )
    return [r["url"] for r in data.get("results", []) if r.get("url")]


def tavily(query: str, key: str) -> list[str]:
    data = post(
        "https://api.tavily.com/search",
        {"query": query, "max_results": LIMIT, "search_depth": "basic"},
        {"Authorization": f"Bearer {key}"},
    )
    return [r["url"] for r in data.get("results", []) if r.get("url")]


def firecrawl(query: str, key: str) -> list[str]:
    # No `scrapeOptions`: search only, so no scrape credits are spent.
    data = post(
        "https://api.firecrawl.dev/v2/search",
        {"query": query, "limit": LIMIT, "sources": ["web"]},
        {"Authorization": f"Bearer {key}"},
    )
    payload = data.get("data", data)
    web = payload.get("web", payload) if isinstance(payload, dict) else payload
    return [r["url"] for r in web if isinstance(r, dict) and r.get("url")]


PROVIDERS = {
    "exa": (exa, "EXA_API_KEY"),
    "tavily": (tavily, "TAVILY_API_KEY"),
    "firecrawl": (firecrawl, "FIRECRAWL_API_KEY"),
}


def registrable(url: str) -> str:
    """Host, minus a leading `www.`. Not a public-suffix parse — good enough
    to count distinct sites, and stated as such rather than overclaimed."""
    host = (urlsplit(url).hostname or "").lower()
    return host[4:] if host.startswith("www.") else host


def jaccard(a: set, b: set) -> float | None:
    union = a | b
    return len(a & b) / len(union) if union else None


def herfindahl(hosts: list[str]) -> float | None:
    """Concentration on 0..1. One host holding everything scores 1."""
    if not hosts:
        return None
    counts = Counter(hosts)
    total = len(hosts)
    return sum((n / total) ** 2 for n in counts.values())


def run(queries: list[tuple[str, str]], available: dict) -> list[dict]:
    rows = []
    for index, (kind, query) in enumerate(queries, 1):
        row = {"kind": kind, "query": query, "providers": {}}
        for name, (fn, _) in available.items():
            try:
                urls = fn(query, os.environ[available[name][1]])
                row["providers"][name] = {"urls": urls, "error": None}
            except urllib.error.HTTPError as exc:
                detail = exc.read().decode()[:200]
                row["providers"][name] = {
                    "urls": None,
                    "error": f"HTTP {exc.code}: {detail}",
                }
            except Exception as exc:  # noqa: BLE001 - reported, never swallowed
                row["providers"][name] = {"urls": None, "error": str(exc)}
            time.sleep(0.4)
        done = sum(1 for p in row["providers"].values() if p["urls"] is not None)
        print(f"  [{index:>2}/{len(queries)}] {kind:<10} {done}/{len(available)} ok"
              f"  {query[:52]}", file=sys.stderr)
        rows.append(row)
    return rows


def summarise(rows: list[dict], names: list[str]) -> dict:
    per_provider = {n: {"results": [], "hosts": [], "hhi": [], "errors": 0} for n in names}
    pair_overlap: dict[str, list[float]] = {}
    union_sizes, unique_shares = [], {n: [] for n in names}
    all_hosts: Counter = Counter()

    for row in rows:
        sets = {}
        for name in names:
            entry = row["providers"].get(name, {})
            if entry.get("urls") is None:
                per_provider[name]["errors"] += 1
                continue
            urls = entry["urls"]
            hosts = [registrable(u) for u in urls if registrable(u)]
            sets[name] = set(hosts)
            all_hosts.update(hosts)
            per_provider[name]["results"].append(len(urls))
            per_provider[name]["hosts"].append(len(set(hosts)))
            h = herfindahl(hosts)
            if h is not None:
                per_provider[name]["hhi"].append(h)

        for i, a in enumerate(names):
            for b in names[i + 1:]:
                if a in sets and b in sets:
                    j = jaccard(sets[a], sets[b])
                    if j is not None:
                        pair_overlap.setdefault(f"{a}|{b}", []).append(j)

        if len(sets) >= 2:
            union = set().union(*sets.values())
            union_sizes.append(len(union))
            for name, s in sets.items():
                others = set().union(*(v for k, v in sets.items() if k != name))
                unique_shares[name].append(len(s - others) / len(s) if s else 0.0)

    def mean(xs):
        return round(statistics.fmean(xs), 4) if xs else None

    return {
        "queries": len(rows),
        "per_provider": {
            n: {
                "mean_results": mean(v["results"]),
                "mean_distinct_hosts": mean(v["hosts"]),
                "mean_host_concentration_hhi": mean(v["hhi"]),
                "failed_queries": v["errors"],
            }
            for n, v in per_provider.items()
        },
        "mean_pairwise_host_jaccard": {k: mean(v) for k, v in pair_overlap.items()},
        "mean_union_hosts_per_query": mean(union_sizes),
        "mean_unique_host_share": {n: mean(v) for n, v in unique_shares.items()},
        "top_hosts_overall": all_hosts.most_common(15),
        "distinct_hosts_overall": len(all_hosts),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pilot", action="store_true", help="one query only")
    parser.add_argument("--out", type=Path, help="write full results as JSON")
    args = parser.parse_args()

    load_env()
    available, missing = {}, []
    for name, (fn, env) in PROVIDERS.items():
        (available.__setitem__(name, (fn, env)) if os.environ.get(env)
         else missing.append(f"{name} ({env})"))
    if missing:
        print(f"unavailable, excluded: {', '.join(missing)}", file=sys.stderr)
    if len(available) < 2:
        sys.exit("need at least two providers with credentials to measure overlap")

    queries = QUERIES[:1] if args.pilot else QUERIES
    print(f"{len(queries)} queries x {len(available)} providers "
          f"= {len(queries) * len(available)} searches\n", file=sys.stderr)

    rows = run(queries, available)
    summary = summarise(rows, list(available))
    print(json.dumps(summary, indent=2))
    if args.out:
        args.out.write_text(json.dumps({"summary": summary, "rows": rows}, indent=2))
        print(f"\nfull results -> {args.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
