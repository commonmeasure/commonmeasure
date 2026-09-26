//! Relay configuration: who receives the projection, if anyone.
//!
//! The file's format and its one parser live in the harness
//! ([`commonmeasure_harness::relay_config`]), which rules on reporting
//! demands from the same file: a configuration the relay refuses must not
//! read there as a receiver that reports.

pub use commonmeasure_harness::relay_config::{
    RelayConfig, events_url, receiver_endpoint, receiver_origin, same_receiver,
};
