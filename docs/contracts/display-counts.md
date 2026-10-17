---
title: Display counts
domain: extensions
audience: contributor
---

# Display counts

This document is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

The counts one display of a signed release produces, as the
`integrity-checker` processor records them
([`processor.md`](processor.md) §Status). This is a working rule that
Common Measure follows and does not define: the definitions belong to the
open standards bodies they are offered to, and the outcome names are those
of the trusted news demo's display rule.

One display is one release fetched and one reply shown against it. The
counts are eleven small non-negative integers in this order:

| # | Count | Incremented when |
|---|---|---|
| 0 | `fetch` | Always 1: the display shows one fetched release. |
| 1 | `quote_shown` | A quote was rendered from the signed text, by passage id or as words that occur whole in it. |
| 2 | `quote_expanded_to_kept` | A quote cut a kept span, and the whole span was shown instead. |
| 3 | `quote_refused` | A quote named a passage or sentence the release does not have; nothing was shown for it. |
| 4 | `shown_confirmed` | A sentence drawn from the source passed the support check (the whole sentence verbatim in the text, or a model stage at its confident-yes threshold against the passages it cites) and was shown plain with those passages. |
| 5 | `shown_own` | The assistant's own sentence was shown labelled "not from the article" (labelled display only). |
| 6 | `shown_unconfirmed` | A sentence drawn from the source was not confirmed and was shown labelled "not from the article" (labelled display only). |
| 7 | `withheld_contradiction` | A model stage found a sentence, drawn from the source or the assistant's own, to contradict the passages it was checked against at the contradiction threshold; the source's passage was shown instead. |
| 8 | `withheld_attribution` | A sentence attributed a quotation to someone the text does not; the source's passage was shown instead. |
| 9 | `withheld_kept` | A sentence reproduced part of a kept span; the whole span was shown instead. |
| 10 | `withheld_unconfirmed` | Under strict display, a sentence not confirmed, or the assistant's own, was not shown; the source's passage was shown instead. |

Each reply item adds one to exactly one of counts 1 to 10, so their sum is
the number of items. The rule that decides which is the processor's rule
text, which its configuration digest covers.

The counts say what the display rule did. A `shown_confirmed` count says a
check passed, not that the sentence was true.

The counts carry outcomes only: no sentence, passage, probability or
reader. The integrity checker's record holds each question's verdict and
probability beside the outcome, names each sentence by its hash and the
words shown by byte span, and stays in the local evidence log.
