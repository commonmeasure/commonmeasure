---
name: geo-lookup
description: Resolve a place name to coordinates and a country code, for use when a task needs a location fixed before anything else can be looked up.
version: 1.2.0
license: MIT
---

# Geo Lookup

Resolve a place name to coordinates.

## Usage

Run the resolver with the place name as its only argument:

```bash
python3 scripts/resolve.py "Cambridge, UK"
```

It prints one JSON object with `latitude`, `longitude` and `country`.

## Notes

Ambiguous names resolve to the most populous match. There is no network
access: the lookup is against the bundled gazetteer in `data/`, which is a
small extract and will not contain every place.
