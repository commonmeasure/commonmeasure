//! Cost of spool state changes with thousands of retained batches (EGR-01).
//!
//! Ignored by default: it measures, it does not assert. Run with
//! `cargo test -p commonmeasure-relay --test spool_measure -- --ignored --nocapture`.
//! `SPOOL_MEASURE_RETAINED` (delivered), `SPOOL_MEASURE_DEAD` and
//! `SPOOL_MEASURE_DUE` override the sizes. The fixture is written in the
//! layout that preceded the journal, so the run also times the first open of
//! an existing spool directory. Its sessions are pinned, as the relay pins
//! every session it delivers, so the close prunes whatever the retention rule
//! releases: above `RETAIN_DELIVERED_BATCHES` delivered, `close` includes
//! that one-off rewrite of the queue file.
//!
//! The code before the journal (`a98a428`) has neither `snapshot()` nor the
//! third argument of `record_acceptance`, and never prunes. To measure it,
//! copy this file into a checkout of that revision, apply the change each
//! `BASELINE` comment states, and run the same command there with its own
//! `CARGO_TARGET_DIR`: two checkouts of this crate collide in a shared one.
//! The baseline's final read is two calls (`delivery_states()` and
//! `pending()`), the nearest equivalent there of one `snapshot()`.

use commonmeasure_relay::spool::{DeliveryState, DeliveryStatus, Spool, SpoolEntry};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

fn size(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// One batch of five events, about the size the projection produces.
fn line(queued_at: chrono::DateTime<chrono::Utc>, session: uuid::Uuid) -> String {
    let events: Vec<_> = (0..5)
        .map(|n| {
            json!({
                "id": uuid::Uuid::new_v4(),
                "type": "content_retrieved",
                "timestamp": queued_at,
                "content_url": format!("https://publisher.example/articles/{n}"),
                "content_hash": "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
                "tokens_ingested": 1200,
                "token_basis": "characters/4",
            })
        })
        .collect();
    json!({
        "origin": "measurement fixture",
        "queued_at": queued_at,
        "directory_selection": false,
        "document": {
            "session_id": session,
            "agent_id": "commonmeasure",
            "refused": 0,
            "events": events,
        },
    })
    .to_string()
}

/// The status read: every state, the undelivered payloads, the pruned count.
///
/// BASELINE: the body becomes
/// `let reader = Spool::read_only(home);`
/// `(reader.delivery_states().unwrap(), reader.pending().unwrap(), 0)`.
fn read_back(home: &Path) -> (BTreeMap<u64, DeliveryState>, Vec<(u64, SpoolEntry)>, u64) {
    let snapshot = Spool::read_only(home).snapshot().unwrap();
    let pruned = snapshot.pruned_delivered.expect("the journal exists");
    (snapshot.states, snapshot.undelivered, pruned)
}

#[test]
#[ignore = "measurement; run explicitly with --ignored --nocapture"]
fn state_changes_with_thousands_of_retained_batches() {
    let retained = size("SPOOL_MEASURE_RETAINED", 5000);
    let dead = size("SPOOL_MEASURE_DEAD", 0);
    let due = size("SPOOL_MEASURE_DUE", 50);
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("relay/spool");
    std::fs::create_dir_all(&dir).unwrap();
    let now = chrono::Utc::now();
    // Recent deliveries: the retention period releases none of them, so
    // only the retained-count rule prunes at close.
    let delivered_at = now - chrono::Duration::hours(1);
    let mut queue = String::new();
    let mut states = serde_json::Map::new();
    let mut pins = serde_json::Map::new();
    for index in 0..retained + dead + due {
        let session = uuid::Uuid::new_v4();
        pins.insert(session.to_string(), json!("commonmeasure"));
        queue.push_str(&line(delivered_at, session));
        queue.push('\n');
        let (status, attempts) = match index {
            i if i < retained => ("delivered", 1),
            i if i < retained + dead => ("dead", 10),
            _ => ("queued", 0),
        };
        states.insert(
            index.to_string(),
            json!({
                "status": status,
                "attempts": attempts,
                "next_attempt_at": null,
                "last_attempt_at": if attempts > 0 { json!(delivered_at) } else { json!(null) },
                "last_error": if status == "dead" { json!("receiver answered 503") } else { json!(null) },
                "hold_reason": null,
            }),
        );
    }
    std::fs::write(
        home.path().join("relay/session-agents.json"),
        serde_json::to_vec(&pins).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("outbound.ndjson"), &queue).unwrap();
    std::fs::write(
        dir.join("outbound.delivery.json"),
        serde_json::to_vec_pretty(&states).unwrap(),
    )
    .unwrap();

    let started = Instant::now();
    let spool = Spool::open(home.path()).unwrap();
    let pending: Vec<_> = spool.pending().unwrap().split_off(dead);
    assert_eq!(pending.len(), due);
    let opened = started.elapsed();

    let started = Instant::now();
    for (index, entry) in &pending {
        assert!(spool.claim(*index, now).unwrap());
        let ids: Vec<uuid::Uuid> = entry.document["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["id"].as_str().unwrap().parse().unwrap())
            .collect();
        // BASELINE: drop the third argument.
        spool.record_acceptance(*index, &ids, 0).unwrap();
    }
    let changed = started.elapsed();

    let started = Instant::now();
    drop(spool);
    let closed = started.elapsed();

    let started = Instant::now();
    let (states, undelivered, pruned) = read_back(home.path());
    let read = started.elapsed();
    let delivered = states
        .values()
        .filter(|state| state.status == DeliveryStatus::Delivered)
        .count();
    assert_eq!(delivered as u64 + pruned, (retained + due) as u64);
    assert_eq!(undelivered.len(), dead);

    println!(
        "delivered={retained} dead={dead} due={due} queue_bytes={} open_and_pending={opened:?} \
         claim_and_accept_total={changed:?} per_state_change={:?} close={closed:?} \
         pruned_at_close={} read_only_snapshot={read:?}",
        queue.len(),
        changed / (2 * due as u32),
        pruned,
    );
}
