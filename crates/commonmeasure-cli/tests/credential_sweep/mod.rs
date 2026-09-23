//! The credential-shaped-string sweep over a published run directory.
//!
//! One implementation, compiled into each test binary that publishes a run,
//! so the run contract and the replay contract cannot drift into checking
//! different markers, and a marker added here reaches both.

// Each integration test binary compiles this module separately, so anything
// only one of them uses is dead code in the others.
#![allow(dead_code)]

use std::path::Path;

/// What a credential would look like if one ever reached an artefact: the
/// header names credentials travel in, and the key prefixes of the providers
/// this repository speaks to.
///
/// Firecrawl's `fc-` prefix is not here because it needs the rule below.
const MARKERS: &[&str] = &[
    "Bearer ",
    "x-api-key",
    "TollbitKey",
    "EXA_API_KEY=",
    "sk-",
    "tvly-",
];

/// Assert that none of `artefacts` under `output` carries a credential-shaped
/// string.
pub fn no_artefact_carries_a_credential(output: &Path, artefacts: &[&str]) {
    for artefact in artefacts {
        let path = output.join(artefact);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        for marker in MARKERS {
            assert!(
                !text.contains(marker),
                "{artefact} contains `{marker}`, which is credential-shaped"
            );
        }
        // Firecrawl's `fc-` prefix is pure hex, so it also occurs inside
        // random run and plan UUIDs ("…49fc-9060…"). It only counts as
        // credential-shaped when it does not continue a hex run.
        for (index, _) in text.match_indices("fc-") {
            let preceded_by_hex = text[..index]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_ascii_hexdigit());
            assert!(
                preceded_by_hex,
                "{artefact} contains `fc-` outside a hex identifier, which is credential-shaped"
            );
        }
    }
}
