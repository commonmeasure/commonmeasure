//! Host observations without content text, joined to durable acquisitions.

use super::observation_index::ValidationIndex;
use super::{SessionLog, TURN_PRIVACY_LEVEL};
use chrono::{SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum HostEvent {
    ObservationsStarted {},
    ContextEntered {
        acquisition_id: Uuid,
        generation_id: Uuid,
        representation_hash: String,
    },
    OutputAssociated {
        generation_id: Uuid,
        output_id: Uuid,
        output_hash: String,
        acquisition_ids: Vec<Uuid>,
    },
    ActionDecided {
        action_id: Uuid,
        generation_id: Uuid,
        acquisition_id: Uuid,
        action: Action,
        decision: ActionDecision,
        rule_id: String,
        target: ActionTarget,
        data_hash: String,
        host_policy_hash: String,
    },
    ObservationUnavailable {
        observation: Unavailable,
    },
}

#[derive(Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    SendAcquiredText,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum ActionDecision {
    Allowed,
    Refused,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ActionTarget {
    url: String,
    recipient: String,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Unavailable {
    ProviderContext,
    ProviderCache,
    SupplierDeliveryIdentity,
    SemanticSupport,
    ProcessEgress,
    RecipientUse,
}

fn invalid(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, reason)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// Whether a string identifies bytes by SHA-256, without carrying those bytes.
pub fn valid_hash(hash: &str) -> bool {
    hash.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

impl SessionLog {
    /// Validate and fsync an observation from the host on this MCP session.
    /// The caller supplies no source policy, source URL, observer identity or text.
    /// A completion acknowledges both its output record and minimal boundary.
    /// Failure is unavailable even if the first append already succeeded.
    /// An identical retry can finish an output whose boundary was not written.
    ///
    /// A request is checked against the validation index
    /// (`observation_index.rs`), which reads the lines the log gained since
    /// the last request, strictly: a malformed or incomplete record anywhere
    /// in them makes the request unavailable.
    pub fn record_host_observation(
        &mut self,
        params: Value,
        host: &str,
        cwd: Option<&str>,
    ) -> io::Result<u64> {
        let observation: HostEvent =
            serde_json::from_value(params).map_err(|error| invalid(&error.to_string()))?;
        let mut index = self
            .validation
            .take()
            .unwrap_or_else(|| ValidationIndex::load(&self.home, &self.session_id, &self.path));
        let recorded = self.record_validated(observation, &mut index, host, cwd);
        self.validation = Some(index);
        recorded
    }

    fn record_validated(
        &mut self,
        observation: HostEvent,
        index: &mut ValidationIndex,
        host: &str,
        cwd: Option<&str>,
    ) -> io::Result<u64> {
        match index.catch_up() {
            Ok(_) => {}
            // No log yet: nothing has been recorded, and the index is empty.
            Err(error) if error.kind() == io::ErrorKind::NotFound && !self.host_observations => {}
            Err(error) => return Err(error),
        }
        let mut payload = json!({
            "session_id": self.session_id(), "host": host,
            "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            "observer": "host", "grade": "observed",
        });
        if !self.host_observations && !matches!(observation, HostEvent::ObservationsStarted {}) {
            return Err(invalid("host observations have not been enabled"));
        }
        let event = match observation {
            HostEvent::ObservationsStarted {} => {
                if self.host_observations || index.crossing_recorded() {
                    return Err(invalid(
                        "enable host observations once, before any mediated crossing",
                    ));
                }
                let seq = self.append("observations_started", payload)?;
                self.host_observations = true;
                return Ok(seq);
            }
            HostEvent::ContextEntered {
                acquisition_id,
                generation_id,
                representation_hash,
            } => {
                if !valid_hash(&representation_hash) {
                    return Err(invalid("representation_hash must be SHA-256"));
                }
                let (acquisition, generation) =
                    (acquisition_id.to_string(), generation_id.to_string());
                if !index.admitted(&acquisition) {
                    return Err(invalid("unknown acquisition handle in this session"));
                }
                if index.entered_as(&acquisition, &generation, &representation_hash) {
                    return Err(invalid("duplicate context-entry observation"));
                }
                if index.generation_has_output(&generation) {
                    return Err(invalid("generation already completed"));
                }
                payload["acquisition_id"] = json!(acquisition_id);
                payload["generation_id"] = json!(generation_id);
                payload["representation_hash"] = json!(representation_hash);
                "context_entered"
            }
            HostEvent::OutputAssociated {
                generation_id,
                output_id,
                output_hash,
                acquisition_ids,
            } => {
                if !valid_hash(&output_hash) || acquisition_ids.len() > 1024 {
                    return Err(invalid(
                        "output requires SHA-256 and at most 1024 associations",
                    ));
                }
                payload["generation_id"] = json!(generation_id);
                payload["output_id"] = json!(output_id);
                payload["output_hash"] = json!(output_hash);
                payload["acquisition_ids"] = json!(acquisition_ids);
                let generation = generation_id.to_string();
                if let Some(original) = index.first_output(&generation, &output_id.to_string())? {
                    let identical = [
                        "generation_id",
                        "output_id",
                        "output_hash",
                        "acquisition_ids",
                        "host",
                    ]
                    .iter()
                    .all(|key| original[key] == payload[key]);
                    if !identical || index.completed(&generation) {
                        return Err(invalid("duplicate output or completed generation"));
                    }
                    // Reuse the durable observation; a retry cannot change its
                    // associations or create a second output record.
                    payload = original;
                } else {
                    self.append("output_associated", payload.clone())?;
                }
                payload
                    .as_object_mut()
                    .expect("object")
                    .remove("output_hash");
                payload
                    .as_object_mut()
                    .expect("object")
                    .remove("acquisition_ids");
                payload["turn_id"] = json!(generation_id);
                payload["privacy_level"] = json!(TURN_PRIVACY_LEVEL);
                payload["privacy_basis"] =
                    json!("the generation boundary carries identifiers and no conversation text");
                payload["detail"] = json!({"cwd": cwd});
                if let Some(identity) = &self.policy_identity {
                    payload["policy_identity"] = json!(identity);
                }
                return self.append("turn_completed", payload);
            }
            HostEvent::ActionDecided {
                action_id,
                generation_id,
                acquisition_id,
                action,
                decision,
                rule_id,
                target,
                data_hash,
                host_policy_hash,
            } => {
                let destination = url::Url::parse(&target.url)
                    .map_err(|_| invalid("action target requires an absolute HTTP(S) URL"))?;
                if target.url.len() > 2048
                    || !matches!(destination.scheme(), "http" | "https")
                    || destination.host_str().is_none()
                    || !destination.username().is_empty()
                    || destination.password().is_some()
                    || destination.query().is_some()
                    || destination.fragment().is_some()
                    || !valid_identifier(&target.recipient)
                    || !valid_identifier(&rule_id)
                    || !valid_hash(&data_hash)
                    || !valid_hash(&host_policy_hash)
                {
                    return Err(invalid("invalid action target, rule identifier or hash"));
                }
                let acquisition = acquisition_id.to_string();
                if !index.admitted_with(&acquisition, &data_hash)
                    || !index.entered(&acquisition, &generation_id.to_string())
                {
                    return Err(invalid(
                        "action requires matching acquisition bytes and context entry",
                    ));
                }
                if index.action_recorded(&action_id.to_string()) {
                    return Err(invalid("duplicate action decision"));
                }
                payload["action_id"] = json!(action_id);
                payload["generation_id"] = json!(generation_id);
                payload["acquisition_id"] = json!(acquisition_id);
                payload["action"] = json!(action);
                payload["decision"] = json!(decision);
                payload["rule_id"] = json!(rule_id);
                payload["target"] = json!(target);
                payload["data_hash"] = json!(data_hash);
                payload["host_policy_hash"] = json!(host_policy_hash);
                "action_decided"
            }
            HostEvent::ObservationUnavailable { observation } => {
                payload["grade"] = json!("unavailable");
                payload["observation"] = json!(observation);
                payload["reason"] = json!(match observation {
                    Unavailable::ProviderContext =>
                        "the host exposes no provider-internal context event",
                    Unavailable::ProviderCache => "the host exposes no provider cache-use event",
                    Unavailable::SupplierDeliveryIdentity =>
                        "no supplier-issued delivery identity is exposed",
                    Unavailable::SemanticSupport =>
                        "explicit source markers do not measure semantic support",
                    Unavailable::ProcessEgress =>
                        "independent process egress restrictions have not been verified",
                    Unavailable::RecipientUse =>
                        "the host cannot observe subsequent recipient access, retention or use",
                });
                "observation_unavailable"
            }
        };
        self.append(event, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crossing(mode: &str, status: Value) -> super::super::Crossing {
        serde_json::from_value(json!({
            "session_id":"s", "timestamp":"2026-09-16T10:00:00Z", "mode":mode,
            "host":"pi", "url":"https://publisher.example/source", "host_name":"publisher.example",
            "grounded":true, "licence":{"state":"unknown"}, "http_status":status,
            "content_hash":format!("sha256:{}", "a".repeat(64))
        }))
        .unwrap()
    }

    #[test]
    fn opening_tolerates_partial_and_damaged_lines_but_observations_remain_strict() {
        use std::io::Write;
        for tail in ["{\"partial\":", "{broken}\n{\"event\":\"other\"}\n"] {
            let home = tempfile::tempdir().unwrap();
            let mut log = SessionLog::open(home.path(), "s").unwrap();
            log.record_host_observation(json!({"event":"observations_started"}), "pi", None)
                .unwrap();
            std::fs::OpenOptions::new()
                .append(true)
                .open(log.path())
                .unwrap()
                .write_all(tail.as_bytes())
                .unwrap();
            let before = std::fs::read(log.path()).unwrap();
            let mut reopened = SessionLog::open(home.path(), "s").unwrap();
            assert!(reopened.host_observations);
            assert!(
                reopened
                    .record_host_observation(
                        json!({"event":"observation_unavailable",
                "observation":"provider_context"}),
                        "pi",
                        None
                    )
                    .is_err()
            );
            reopened
                .record_crossing(&crossing("observed", json!(200)))
                .unwrap();
            let after = std::fs::read(log.path()).unwrap();
            assert!(
                after.starts_with(&before),
                "opening must not repair a concurrent writer's bytes"
            );
            let appended: Value = serde_json::from_slice(&after[before.len()..]).unwrap();
            assert_eq!(appended["event"], "crossing_observed");
            assert!(
                SessionLog::read(log.path()).is_err(),
                "damaged evidence stays unavailable"
            );
        }
    }

    #[test]
    fn opt_in_discovery_skips_damage_before_a_complete_marker() {
        let home = tempfile::tempdir().unwrap();
        let log = SessionLog::open(home.path(), "s").unwrap();
        std::fs::write(
            log.path(),
            b"{broken}\n{\"event\":\"observations_started\"}\n{partial",
        )
        .unwrap();
        assert!(
            SessionLog::open(home.path(), "s")
                .unwrap()
                .host_observations
        );
        std::fs::write(log.path(), b"{\"event\":\"observations_started\"}").unwrap();
        assert!(
            !SessionLog::open(home.path(), "s")
                .unwrap()
                .host_observations
        );
    }

    #[test]
    fn opt_in_allows_prior_hook_crossings_and_search_keeps_its_semantics() {
        let home = tempfile::tempdir().unwrap();
        let mut log = SessionLog::open(home.path(), "s").unwrap();
        log.record_crossing(&crossing("observed", json!(200)))
            .unwrap();
        log.record_host_observation(json!({"event":"observations_started"}), "pi", None)
            .unwrap();
        log.record_crossing(&crossing("mediated", Value::Null))
            .unwrap();
        let records = SessionLog::read(log.path()).unwrap();
        let search = &records.last().unwrap()["payload"];
        assert!(search.get("context_observation").is_none());
        assert!(search.get("acquisition_id").is_none());
        assert_eq!(search["grounded"], true);

        let mut other = SessionLog::open(home.path(), "late").unwrap();
        other
            .record_crossing(&crossing("mediated", json!(200)))
            .unwrap();
        assert!(
            other
                .record_host_observation(json!({"event":"observations_started"}), "pi", None)
                .is_err()
        );
    }

    #[test]
    fn identical_output_retry_finishes_only_a_missing_boundary() {
        let home = tempfile::tempdir().unwrap();
        let mut log = SessionLog::open(home.path(), "s").unwrap();
        log.record_host_observation(json!({"event":"observations_started"}), "pi", None)
            .unwrap();
        let generation = Uuid::new_v4();
        let output = json!({"event":"output_associated", "generation_id":generation,
            "output_id":Uuid::new_v4(), "output_hash":format!("sha256:{}", "c".repeat(64)),
            "acquisition_ids":[]});
        // Materialise the durable state after the first append succeeds and
        // before the completion append. This is a recovery unit fixture.
        let mut durable = output.clone();
        durable.as_object_mut().unwrap().remove("event");
        durable["host"] = json!("pi");
        durable["session_id"] = json!("s");
        durable["observer"] = json!("host");
        durable["grade"] = json!("observed");
        durable["timestamp"] = json!("2026-09-16T10:00:00Z");
        log.log
            .append("output_associated", durable.clone())
            .unwrap();
        let mut log = SessionLog::open(home.path(), "s").unwrap();
        for field in [
            "output_hash",
            "output_id",
            "generation_id",
            "acquisition_ids",
        ] {
            let mut changed = output.clone();
            changed[field] = match field {
                "output_hash" => json!(format!("sha256:{}", "d".repeat(64))),
                "acquisition_ids" => json!([Uuid::new_v4()]),
                _ => json!(Uuid::new_v4()),
            };
            assert!(
                log.record_host_observation(changed, "pi", None).is_err(),
                "{field}"
            );
        }
        assert!(
            log.record_host_observation(output.clone(), "other", None)
                .is_err()
        );
        log.record_host_observation(output.clone(), "pi", Some("/project"))
            .unwrap();
        let records = SessionLog::read(log.path()).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[1]["payload"], durable);
        assert_eq!(records[2]["event"], "turn_completed");
        assert_eq!(records[2]["payload"]["turn_id"], json!(generation));
        assert_eq!(records[2]["payload"]["detail"]["cwd"], "/project");
        assert!(records[2]["payload"].get("output_hash").is_none());
        assert!(log.record_host_observation(output, "pi", None).is_err());
    }

    #[test]
    fn handles_survive_reopening_and_observations_reject_duplicates_and_failed_writes() {
        let home = tempfile::tempdir().unwrap();
        let mut log = SessionLog::open(home.path(), "durable").unwrap();
        log.record_host_observation(json!({"event":"observations_started"}), "pi", None)
            .unwrap();
        let crossing: super::super::Crossing = serde_json::from_value(json!({
            "session_id":"durable", "timestamp":"2026-09-16T10:00:00Z", "mode":"mediated",
            "host":"pi", "url":"https://publisher.example/source", "host_name":"publisher.example",
            "grounded":true, "licence":{"state":"unknown"}, "http_status":200,
            "content_hash":format!("sha256:{}", "a".repeat(64))
        }))
        .unwrap();
        log.record_crossing(&crossing).unwrap();
        let handle = log.acquisition_handle().unwrap().unwrap();
        let generation = Uuid::new_v4();
        let entry = json!({"event":"context_entered", "acquisition_id":handle,
            "generation_id":generation, "representation_hash":format!("sha256:{}", "b".repeat(64))});
        let mut log = SessionLog::open(home.path(), "durable").unwrap();
        log.record_host_observation(entry.clone(), "pi", None)
            .unwrap();
        assert!(log.record_host_observation(entry, "pi", None).is_err());
        let output = json!({"event":"output_associated", "generation_id":generation,
            "output_id":Uuid::new_v4(), "output_hash":format!("sha256:{}", "c".repeat(64)),
            "acquisition_ids":[handle]});
        log.record_host_observation(output.clone(), "pi", Some("/project"))
            .unwrap();
        assert!(log.record_host_observation(output, "pi", None).is_err());
        let records = SessionLog::read(log.path()).unwrap();
        assert_eq!(records[1]["payload"]["grounded"], false);
        assert_eq!(records.last().unwrap()["event"], "turn_completed");
        assert_eq!(
            records.last().unwrap()["payload"]["privacy_level"],
            "minimal"
        );
        std::fs::remove_file(log.path()).unwrap();
        std::fs::create_dir(log.path()).unwrap();
        assert!(log.record_crossing(&crossing).is_err());
        assert!(log.acquisition_handle().is_err());
    }

    /// The action arm's refusals, each against a session in which the same
    /// request with the one field restored is accepted. Nothing refused may
    /// reach the file, and the accepted record carries CM's stamps only.
    #[test]
    fn action_decisions_bind_to_admitted_bytes_and_reject_host_supplied_claims() {
        let home = tempfile::tempdir().unwrap();
        let hash = |digit: &str| format!("sha256:{}", digit.repeat(64));
        let admit = |log: &mut SessionLog| {
            log.record_crossing(&crossing("mediated", json!(200)))
                .unwrap();
            log.acquisition_handle().unwrap().unwrap()
        };
        let enter = |log: &mut SessionLog, handle: Uuid, generation: Uuid| {
            log.record_host_observation(
                json!({"event":"context_entered", "acquisition_id":handle,
                    "generation_id":generation, "representation_hash":hash("b")}),
                "pi",
                None,
            )
            .unwrap();
        };

        // Another session on the same home holds an admitted, entered handle.
        let mut other = SessionLog::open(home.path(), "other").unwrap();
        other
            .record_host_observation(json!({"event":"observations_started"}), "pi", None)
            .unwrap();
        let (foreign, foreign_generation) = (admit(&mut other), Uuid::new_v4());
        enter(&mut other, foreign, foreign_generation);

        let mut log = SessionLog::open(home.path(), "s").unwrap();
        log.set_policy_identity(&crate::policy::PolicyIdentity {
            schema: "test".to_owned(),
            resolver: "test".to_owned(),
            digest: hash("9"),
        });
        let request = |handle: Uuid, generation: Uuid| {
            json!({"event":"action_decided", "action_id":Uuid::new_v4(),
                "generation_id":generation, "acquisition_id":handle,
                "action":"send_acquired_text", "decision":"refused",
                "rule_id":"recipient_not_authorised",
                "target":{"url":"https://shared.example/inbox/other", "recipient":"other"},
                "data_hash":hash("a"), "host_policy_hash":hash("e")})
        };
        assert!(
            log.record_host_observation(request(Uuid::new_v4(), Uuid::new_v4()), "pi", None)
                .is_err(),
            "observations not enabled"
        );
        log.record_host_observation(json!({"event":"observations_started"}), "pi", None)
            .unwrap();
        let (handle, generation) = (admit(&mut log), Uuid::new_v4());
        // Admitted but never entered into a generation.
        let unentered = admit(&mut log);
        // Refused by policy: CM issues no handle, so the host has none to cite.
        let mut refused = crossing("mediated", json!(200));
        refused.refusal = Some("policy".to_owned());
        log.record_crossing(&refused).unwrap();
        assert!(log.acquisition_handle().is_err());
        // A refusal row that did carry a handle still could not be cited. CM
        // never writes one; this pins the event check in the arm.
        let forged = Uuid::new_v4();
        log.log
            .append(
                "crossing_refused",
                json!({"session_id":"s", "acquisition_id":forged, "content_hash":hash("a")}),
            )
            .unwrap();
        for (handle, generation) in [(forged, generation), (forged, Uuid::new_v4())] {
            log.log
                .append(
                    "context_entered",
                    json!({"session_id":"s", "acquisition_id":handle, "generation_id":generation,
                        "representation_hash":hash("b")}),
                )
                .unwrap();
        }
        enter(&mut log, handle, generation);
        // CM does not constrain the form of a crossing's content hash, so the
        // action arm checks the form of `data_hash` itself.
        let mut unhashed = crossing("mediated", json!(200));
        unhashed.content_hash = Some("md5:abc".to_owned());
        log.record_crossing(&unhashed).unwrap();
        let unhashed = log.acquisition_handle().unwrap().unwrap();
        enter(&mut log, unhashed, generation);
        let before = SessionLog::read(log.path()).unwrap().len();

        let valid = request(handle, generation);
        let with = |pointer: &str, value: Value| {
            let mut changed = valid.clone();
            *changed.pointer_mut(pointer).unwrap() = value;
            changed
        };
        let adding = |key: &str, value: Value| {
            let mut changed = valid.clone();
            changed[key] = value;
            changed
        };
        let mut refusals = vec![
            ("mismatched data_hash", with("/data_hash", json!(hash("f")))),
            (
                "unknown handle",
                with("/acquisition_id", json!(Uuid::new_v4())),
            ),
            ("foreign handle", request(foreign, foreign_generation)),
            ("refused crossing", with("/acquisition_id", json!(forged))),
            (
                "no context entry",
                with("/acquisition_id", json!(unentered)),
            ),
            (
                "other generation",
                with("/generation_id", json!(Uuid::new_v4())),
            ),
            (
                "another action kind",
                with("/action", json!("send_working_directory")),
            ),
            ("another decision", with("/decision", json!("deferred"))),
            ("data_hash not SHA-256", {
                let mut matching = with("/data_hash", json!("md5:abc"));
                matching["acquisition_id"] = json!(unhashed);
                matching
            }),
            (
                "policy hash not SHA-256",
                with("/host_policy_hash", json!("A".repeat(71))),
            ),
            ("action_id not a UUID", with("/action_id", json!("first"))),
            (
                "target field",
                with(
                    "/target",
                    json!({"url":"https://shared.example/inbox/other", "recipient":"other",
                        "body":"acquired text"}),
                ),
            ),
            (
                "missing recipient",
                with("/target", json!({"url":"https://shared.example/inbox"})),
            ),
            (
                "oversized URL",
                with(
                    "/target/url",
                    json!(format!("https://shared.example/{}", "a".repeat(2048))),
                ),
            ),
        ];
        for (field, value) in [
            ("observer", json!("cm")),
            ("grade", json!("mediated")),
            ("timestamp", json!("2020-01-01T00:00:00Z")),
            ("policy_identity", json!(hash("9"))),
            ("session_id", json!("other")),
            ("host", json!("cm")),
            ("text", json!("acquired text")),
            ("arguments", json!({"path":"/private/project"})),
        ] {
            refusals.push((field, adding(field, value)));
        }
        for url in [
            "https://user@shared.example/inbox/other",
            "https://user:secret@shared.example/inbox/other",
            "https://:secret@shared.example/inbox/other",
            "https://shared.example/inbox/other?token=secret",
            "https://shared.example/inbox/other#private",
            "ftp://shared.example/inbox/other",
            "file:///private/project",
            "/inbox/other",
            "",
        ] {
            refusals.push((url, with("/target/url", json!(url))));
        }
        let overlong = "r".repeat(129);
        for identifier in ["", "two words", "a/b", "ünicode", &overlong] {
            refusals.push((identifier, with("/rule_id", json!(identifier))));
            refusals.push((identifier, with("/target/recipient", json!(identifier))));
        }
        for (case, refusal) in refusals {
            assert!(
                log.record_host_observation(refusal, "pi", None).is_err(),
                "{case:?} was accepted"
            );
        }
        assert_eq!(
            SessionLog::read(log.path()).unwrap().len(),
            before,
            "a refused observation leaves no record"
        );

        log.record_host_observation(valid.clone(), "pi", Some("/private/project"))
            .unwrap();
        assert!(
            log.record_host_observation(valid.clone(), "pi", None)
                .is_err(),
            "duplicate action_id"
        );
        // A decision for the same generation and handle under a new action_id
        // is a separate action, recorded after the completion boundary.
        log.record_host_observation(
            json!({"event":"output_associated", "generation_id":generation,
                "output_id":Uuid::new_v4(), "output_hash":hash("c"), "acquisition_ids":[handle]}),
            "pi",
            None,
        )
        .unwrap();
        let mut allowed = request(handle, generation);
        allowed["decision"] = json!("allowed");
        allowed["rule_id"] = json!("grant.exact");
        log.record_host_observation(allowed, "pi", None).unwrap();

        let records = SessionLog::read(log.path()).unwrap();
        let actions: Vec<_> = records
            .iter()
            .filter(|r| r["event"] == "action_decided")
            .collect();
        assert_eq!(actions.len(), 2);
        assert_eq!(records.last().unwrap()["event"], "action_decided");
        let recorded = actions[0]["payload"].as_object().unwrap();
        let mut keys: Vec<_> = recorded.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "acquisition_id",
                "action",
                "action_id",
                "data_hash",
                "decision",
                "generation_id",
                "grade",
                "host",
                "host_policy_hash",
                "observer",
                "rule_id",
                "session_id",
                "target",
                "timestamp"
            ],
            "no policy identity, working directory or other field"
        );
        for key in [
            "action_id",
            "generation_id",
            "acquisition_id",
            "action",
            "decision",
            "rule_id",
            "target",
            "data_hash",
            "host_policy_hash",
        ] {
            assert_eq!(recorded[key], valid[key], "{key}");
        }
        assert_eq!(recorded["observer"], "host");
        assert_eq!(recorded["grade"], "observed");
        assert_eq!(recorded["session_id"], "s");
        assert_eq!(recorded["host"], "pi");
        let stamped = chrono::DateTime::parse_from_rfc3339(recorded["timestamp"].as_str().unwrap())
            .expect("CM's timestamp");
        assert!(
            (Utc::now() - stamped.with_timezone(&Utc))
                .num_seconds()
                .abs()
                < 60
        );
        assert_eq!(actions[1]["payload"]["decision"], "allowed");
        // CM's admission stays on the crossing, with the policy that ruled.
        let admission = records
            .iter()
            .find(|r| r["payload"]["acquisition_id"] == json!(handle))
            .unwrap();
        assert_eq!(admission["event"], "crossing_mediated");
        assert_eq!(admission["payload"]["observer"], "cm");
        assert_eq!(admission["payload"]["policy_identity"], hash("9"));
    }

    /// NET-03's long-session cost, printed and never asserted: opening a
    /// session that never opted in, and validating one observation request
    /// against an opted-in session of the same length. The request is refused
    /// for an unknown handle, so the figure is validation with no append.
    /// `CM_LONG_SESSION_RECORDS` sets the length (default 50,000 records).
    ///
    /// The restarts after that are with a journal of one entry per accepted
    /// request: `COMPACT_AFTER` entries, the longest a restart leaves as it
    /// is, and `CM_LONG_JOURNAL_ENTRIES` (default 10,000), which the restart
    /// parses once and replaces. The entries are journalled here through the
    /// index without the requests, whose fsyncs would dominate the run.
    ///
    /// `cargo test -p commonmeasure-harness --lib long_session_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, run by hand"]
    fn long_session_cost() {
        use std::io::Write;
        use std::time::Instant;
        let records: usize = std::env::var("CM_LONG_SESSION_RECORDS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(50_000);
        let home = tempfile::tempdir().unwrap();
        let sessions = home.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let observed = crossing("observed", json!(200)).to_record();
        let write = |session: &str, opted_in: bool| {
            let file = std::fs::File::create(sessions.join(format!("{session}.ndjson"))).unwrap();
            let mut file = std::io::BufWriter::new(file);
            for seq in 1..=records {
                let record = if opted_in && seq == 1 {
                    json!({"seq":seq, "timestamp":"2026-09-16T10:00:00.000Z",
                        "event":"observations_started", "payload":{"session_id":session}})
                } else {
                    json!({"seq":seq, "timestamp":"2026-09-16T10:00:00.000Z",
                        "event":"crossing_observed", "payload":observed})
                };
                serde_json::to_writer(&mut file, &record).unwrap();
                file.write_all(b"\n").unwrap();
            }
            file.flush().unwrap();
        };
        write("never", false);
        write("opted", true);
        let bytes = std::fs::metadata(sessions.join("never.ndjson"))
            .unwrap()
            .len();
        println!("records per session: {records}; log bytes: {bytes}");

        let started = Instant::now();
        let never = SessionLog::open(home.path(), "never").unwrap();
        println!("open, never opted in: {:?}", started.elapsed());
        assert!(!never.host_observations);

        let mut opted = SessionLog::open(home.path(), "opted").unwrap();
        assert!(opted.host_observations);
        let mut request = || {
            let started = Instant::now();
            let refused = opted.record_host_observation(
                json!({"event":"context_entered", "acquisition_id":Uuid::new_v4(),
                    "generation_id":Uuid::new_v4(),
                    "representation_hash":format!("sha256:{}", "b".repeat(64))}),
                "pi",
                None,
            );
            assert_eq!(refused.unwrap_err().kind(), io::ErrorKind::InvalidInput);
            started.elapsed()
        };
        println!("first observation request: {:?}", request());
        let later: Vec<_> = (0..10).map(|_| request()).collect();
        println!(
            "next 10 requests, mean: {:?}",
            later.iter().sum::<std::time::Duration>() / 10
        );
        drop(opted);
        let mut reopened = SessionLog::open(home.path(), "opted").unwrap();
        let started = Instant::now();
        let _ = reopened.record_host_observation(
            json!({"event":"context_entered", "acquisition_id":Uuid::new_v4(),
                "generation_id":Uuid::new_v4(),
                "representation_hash":format!("sha256:{}", "b".repeat(64))}),
            "pi",
            None,
        );
        println!("first request after a restart: {:?}", started.elapsed());
        drop(reopened);

        let long: usize = std::env::var("CM_LONG_JOURNAL_ENTRIES")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(10_000);
        let log = sessions.join("opted.ndjson");
        let journal = home.path().join("observation-index/opted.ndjson");
        for entries in [super::super::observation_index::COMPACT_AFTER, long] {
            std::fs::remove_file(&journal).unwrap();
            let mut index = ValidationIndex::load(home.path(), "opted", &log);
            index.catch_up().unwrap();
            let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
            for _ in 1..entries {
                let mut record = entered(Uuid::new_v4(), Uuid::new_v4(), "b");
                record.as_object_mut().unwrap().remove("event");
                let line = json!({"event":"context_entered", "payload":record});
                file.write_all(format!("{line}\n").as_bytes()).unwrap();
                index.catch_up().unwrap();
            }
            drop(index);
            let journalled = std::fs::read_to_string(&journal).unwrap().lines().count();
            for restart in ["first", "second"] {
                let mut reopened = SessionLog::open(home.path(), "opted").unwrap();
                let started = Instant::now();
                let _ = reopened.record_host_observation(
                    entered(Uuid::new_v4(), Uuid::new_v4(), "b"),
                    "pi",
                    None,
                );
                println!(
                    "journal of {journalled} entries, {restart} restart, first request: {:?}",
                    started.elapsed()
                );
            }
        }
    }

    fn hash(digit: &str) -> String {
        format!("sha256:{}", digit.repeat(64))
    }

    fn entered(handle: Uuid, generation: Uuid, representation: &str) -> Value {
        json!({"event":"context_entered", "acquisition_id":handle,
            "generation_id":generation, "representation_hash":hash(representation)})
    }

    fn write_raw(log: &SessionLog, bytes: &[u8]) {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(log.path())
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }

    /// Hook processes append while the server validates, and a second writer
    /// of observations holds its own index: each sees what the other wrote,
    /// through the log, and refuses what conflicts with it.
    #[test]
    fn validation_follows_other_writers_and_refuses_their_conflicts() {
        let home = tempfile::tempdir().unwrap();
        let mut server = SessionLog::open(home.path(), "s").unwrap();
        server
            .record_host_observation(json!({"event":"observations_started"}), "pi", None)
            .unwrap();
        // The contract lets a concurrent incomplete append make a request
        // unavailable; a caller that meets one asks again. One that persists
        // is a fault, not a writer still writing.
        fn observe(log: &mut SessionLog, request: &Value) -> io::Result<u64> {
            for _ in 0..100 {
                match log.record_host_observation(request.clone(), "pi", None) {
                    Err(error) if error.to_string().contains("incomplete record") => {}
                    result => return result,
                }
            }
            panic!("the session log still ended in an incomplete record after 100 requests");
        }
        // A writer of mediated crossings and their context entries.
        fn enter_eight(log: &mut SessionLog) -> Vec<(Uuid, Uuid)> {
            (0..8)
                .map(|_| {
                    log.record_crossing(&crossing("mediated", json!(200)))
                        .unwrap();
                    let handle = log.acquisition_handle().unwrap().unwrap();
                    let generation = Uuid::new_v4();
                    observe(log, &entered(handle, generation, "b")).unwrap();
                    (handle, generation)
                })
                .collect()
        }
        let hooks: Vec<_> = (0..4)
            .map(|_| {
                let home = home.path().to_owned();
                std::thread::spawn(move || {
                    let mut hook = SessionLog::open(&home, "s").unwrap();
                    for _ in 0..8 {
                        hook.record_crossing(&crossing("observed", json!(200)))
                            .unwrap();
                    }
                })
            })
            .collect();
        // The second writer validates and journals while the first does.
        let other = {
            let home = home.path().to_owned();
            std::thread::spawn(move || {
                let mut other = SessionLog::open(&home, "s").unwrap();
                let entries = enter_eight(&mut other);
                (other, entries)
            })
        };
        let generations = enter_eight(&mut server);
        for hook in hooks {
            hook.join().unwrap();
        }
        let (mut other, theirs) = other.join().unwrap();
        assert_eq!(
            SessionLog::read(server.path()).unwrap().len(),
            1 + 32 + 16 + 16
        );

        let mut restarted = SessionLog::open(home.path(), "s").unwrap();
        for (handle, generation) in generations.iter().chain(&theirs) {
            for log in [&mut server, &mut other, &mut restarted] {
                let duplicate = observe(log, &entered(*handle, *generation, "b"));
                assert_eq!(duplicate.unwrap_err().kind(), io::ErrorKind::InvalidInput);
            }
        }
        let (handle, generation) = generations[0];
        let output = |output_id: Uuid| {
            json!({"event":"output_associated", "generation_id":generation,
                "output_id":output_id, "output_hash":hash("c"), "acquisition_ids":[handle]})
        };
        let output_id = Uuid::new_v4();
        observe(&mut server, &output(output_id)).unwrap();
        for conflict in [
            output(Uuid::new_v4()),
            output(output_id),
            entered(handle, generation, "d"),
        ] {
            let refused = observe(&mut other, &conflict);
            assert_eq!(refused.unwrap_err().kind(), io::ErrorKind::InvalidInput);
        }
        other
            .record_crossing(&crossing("mediated", json!(200)))
            .unwrap();
        let theirs = other.acquisition_handle().unwrap().unwrap();
        observe(&mut server, &entered(theirs, Uuid::new_v4(), "b")).unwrap();
    }

    /// A record half written is unavailable, is never indexed in part, and is
    /// indexed whole once its writer finishes. A malformed record stays
    /// unavailable across retries and a restart, nothing after it is used,
    /// and no refused request reaches the file.
    #[test]
    fn a_partial_tail_waits_for_its_writer_and_a_malformed_record_stays_unavailable() {
        let home = tempfile::tempdir().unwrap();
        let mut log = SessionLog::open(home.path(), "s").unwrap();
        log.record_host_observation(json!({"event":"observations_started"}), "pi", None)
            .unwrap();
        log.record_crossing(&crossing("mediated", json!(200)))
            .unwrap();
        let handle = log.acquisition_handle().unwrap().unwrap();
        let generation = Uuid::new_v4();
        let unavailable = |log: &mut SessionLog, request: Value| {
            let before = std::fs::read(log.path()).unwrap();
            let error = log
                .record_host_observation(request, "pi", None)
                .unwrap_err();
            assert_ne!(error.kind(), io::ErrorKind::InvalidInput, "{error}");
            assert_eq!(std::fs::read(log.path()).unwrap(), before);
        };

        let mut record = entered(handle, generation, "b");
        record.as_object_mut().unwrap().remove("event");
        let line = format!("{}\n", json!({"event":"context_entered", "payload":record}));
        let (head, rest) = line.split_at(line.len() / 2);
        write_raw(&log, head.as_bytes());
        unavailable(&mut log, entered(handle, generation, "b"));
        unavailable(
            &mut SessionLog::open(home.path(), "s").unwrap(),
            entered(handle, generation, "b"),
        );
        write_raw(&log, rest.as_bytes());
        let duplicate = log.record_host_observation(entered(handle, generation, "b"), "pi", None);
        assert_eq!(duplicate.unwrap_err().kind(), io::ErrorKind::InvalidInput);
        log.record_host_observation(entered(handle, generation, "d"), "pi", None)
            .unwrap();

        let later = Uuid::new_v4();
        write_raw(&log, b"{broken}\n");
        write_raw(
            &log,
            format!(
                "{}\n",
                json!({"event":"crossing_mediated", "payload":{"acquisition_id":later}})
            )
            .as_bytes(),
        );
        for _ in 0..2 {
            unavailable(&mut log, entered(later, Uuid::new_v4(), "b"));
            unavailable(&mut log, entered(handle, Uuid::new_v4(), "b"));
        }
        let mut restarted = SessionLog::open(home.path(), "s").unwrap();
        unavailable(&mut restarted, entered(handle, Uuid::new_v4(), "b"));
    }

    /// Output provenance after the writer is gone. Every append is synced
    /// before it returns, so dropping the writer here leaves what a killed
    /// process leaves. A reader with no writer joins the output to the
    /// acquisition it cites and checks the output's bytes against the
    /// recorded hash. Then the final append is lost (the file is cut inside
    /// its last line): a strict read fails, a restarted writer's observation
    /// is unavailable and changes nothing, and the complete lines still hold
    /// the output's association. The missing boundary is a gap in the record,
    /// not a record that the generation completed.
    #[test]
    fn a_killed_writers_output_still_joins_its_acquisition_and_a_lost_final_append_is_explicit() {
        use sha2::{Digest, Sha256};

        let answer = b"Registered instances fetch under policy.";
        let output_hash = format!("sha256:{:x}", Sha256::digest(answer));
        let home = tempfile::tempdir().unwrap();
        let generation = Uuid::new_v4();
        let handle = {
            let mut log = SessionLog::open(home.path(), "s").unwrap();
            log.record_host_observation(json!({"event":"observations_started"}), "pi", None)
                .unwrap();
            log.record_crossing(&crossing("mediated", json!(200)))
                .unwrap();
            let handle = log.acquisition_handle().unwrap().unwrap();
            log.record_host_observation(entered(handle, generation, "b"), "pi", None)
                .unwrap();
            log.record_host_observation(
                json!({"event":"output_associated", "generation_id":generation,
                    "output_id":Uuid::new_v4(), "output_hash":output_hash,
                    "acquisition_ids":[handle]}),
                "pi",
                Some("/project"),
            )
            .unwrap();
            handle
        };
        let path = home.path().join("sessions/s.ndjson");

        let records = SessionLog::read(&path).unwrap();
        let summary = crate::session::host_observations(&records, None);
        assert_eq!(
            (
                summary.acquisitions,
                summary.context_entries,
                summary.outputs,
                summary.output_associations,
                summary.unresolved_associations
            ),
            (1, 1, 1, 1, 0)
        );
        let output = records
            .iter()
            .find(|record| record["event"] == "output_associated")
            .unwrap();
        assert_eq!(
            output["payload"]["output_hash"],
            json!(format!("sha256:{:x}", Sha256::digest(answer))),
            "the bytes the session produced are the bytes the record names"
        );
        let cited = records
            .iter()
            .find(|record| record["payload"]["acquisition_id"] == json!(handle))
            .unwrap();
        assert_eq!(cited["event"], "crossing_mediated");
        assert_eq!(
            cited["payload"]["content_hash"],
            json!(format!("sha256:{}", "a".repeat(64)))
        );
        assert_eq!(records.last().unwrap()["event"], "turn_completed");

        // Lose the final append: cut the file inside its last line.
        let bytes = std::fs::read(&path).unwrap();
        let last_line = bytes[..bytes.len() - 1]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .unwrap()
            + 1;
        let cut = last_line + (bytes.len() - last_line) / 2;
        std::fs::write(&path, &bytes[..cut]).unwrap();
        assert!(SessionLog::read(&path).is_err(), "a strict read fails");
        let mut restarted = SessionLog::open(home.path(), "s").unwrap();
        let error = restarted
            .record_host_observation(entered(handle, Uuid::new_v4(), "d"), "pi", None)
            .unwrap_err();
        assert_ne!(error.kind(), io::ErrorKind::InvalidInput, "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), &bytes[..cut]);

        let complete: Vec<Value> = std::str::from_utf8(&bytes[..last_line])
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let summary = crate::session::host_observations(&complete, None);
        assert_eq!((summary.outputs, summary.output_associations), (1, 1));
        assert!(
            complete
                .iter()
                .all(|record| record["event"] != "turn_completed"),
            "the lost boundary is absent, and nothing stands in for it"
        );
    }

    /// An output whose completion boundary was never written, met after a
    /// restart with the index journal in each state it can be found in. The
    /// outcome is the same in all of them: a changed retry and a conflicting
    /// output are refused, the identical retry writes the one boundary, and
    /// the generation is then complete.
    #[test]
    fn pending_completion_recovers_whatever_state_the_index_journal_is_in() {
        for state in ["intact", "missing", "malformed", "torn", "another log's"] {
            let home = tempfile::tempdir().unwrap();
            let mut log = SessionLog::open(home.path(), "s").unwrap();
            log.record_host_observation(json!({"event":"observations_started"}), "pi", None)
                .unwrap();
            let generation = Uuid::new_v4();
            let output = json!({"event":"output_associated", "generation_id":generation,
                "output_id":Uuid::new_v4(), "output_hash":hash("c"), "acquisition_ids":[]});
            // The durable state after the first append and before the second.
            let mut durable = output.clone();
            durable.as_object_mut().unwrap().remove("event");
            durable["host"] = json!("pi");
            log.log.append("output_associated", durable).unwrap();
            log.record_host_observation(
                json!({"event":"observation_unavailable", "observation":"provider_cache"}),
                "pi",
                None,
            )
            .unwrap();
            drop(log);

            let journal = home.path().join("observation-index").join("s.ndjson");
            let indexed = std::fs::read_to_string(&journal).unwrap();
            assert!(indexed.contains(&generation.to_string()), "{state}");
            match state {
                "missing" => std::fs::remove_file(&journal).unwrap(),
                "malformed" => std::fs::write(&journal, "{broken}\n").unwrap(),
                "torn" => std::fs::write(&journal, &indexed[..indexed.len() - 9]).unwrap(),
                "another log's" => {
                    let mut entry: Value =
                        serde_json::from_str(indexed.lines().last().unwrap()).unwrap();
                    entry["from"] = json!(0);
                    entry["tail"] = json!("0".repeat(64));
                    entry["facts"] = json!([{"fact":"completed", "turn":generation}]);
                    std::fs::write(&journal, format!("{entry}\n")).unwrap();
                }
                _ => {}
            }

            let mut log = SessionLog::open(home.path(), "s").unwrap();
            let mut changed = output.clone();
            changed["output_hash"] = json!(hash("d"));
            let mut conflicting = output.clone();
            conflicting["output_id"] = json!(Uuid::new_v4());
            for refused in [changed, conflicting] {
                let refused = log.record_host_observation(refused, "pi", None);
                assert_eq!(
                    refused.unwrap_err().kind(),
                    io::ErrorKind::InvalidInput,
                    "{state}"
                );
            }
            log.record_host_observation(output.clone(), "pi", None)
                .unwrap_or_else(|error| panic!("{state}: {error}"));
            let mut restarted = SessionLog::open(home.path(), "s").unwrap();
            for log in [&mut log, &mut restarted] {
                let completed = log.record_host_observation(output.clone(), "pi", None);
                assert_eq!(
                    completed.unwrap_err().kind(),
                    io::ErrorKind::InvalidInput,
                    "{state}"
                );
            }
            let records = SessionLog::read(log.path()).unwrap();
            let count = |event: &str| records.iter().filter(|r| r["event"] == event).count();
            assert_eq!(count("output_associated"), 1, "{state}");
            assert_eq!(count("turn_completed"), 1, "{state}");
            assert_eq!(
                records.last().unwrap()["payload"]["turn_id"],
                json!(generation)
            );
        }
    }

    #[test]
    fn ingress_rejects_unbound_claims_and_text_and_keeps_unavailability_explicit() {
        let home = tempfile::tempdir().unwrap();
        let mut log = SessionLog::open(home.path(), "s").unwrap();
        assert!(
            log.record_host_observation(json!({"event":"context_entered"}), "pi", None)
                .is_err()
        );
        assert!(
            log.record_host_observation(
                json!({"event":"observations_started", "text":"private"}),
                "pi",
                None
            )
            .is_err()
        );
        log.record_host_observation(json!({"event":"observations_started"}), "pi", None)
            .unwrap();
        let oversized = json!({"event":"output_associated", "generation_id":Uuid::new_v4(),
            "output_id":Uuid::new_v4(), "output_hash":format!("sha256:{}", "a".repeat(64)),
            "acquisition_ids":vec![Uuid::new_v4(); 1025]});
        assert!(log.record_host_observation(oversized, "pi", None).is_err());
        let request = json!({"event":"context_entered", "acquisition_id":Uuid::new_v4(),
            "generation_id":Uuid::new_v4(), "representation_hash":format!("sha256:{}", "a".repeat(64))});
        assert!(log.record_host_observation(request, "pi", None).is_err());
        assert!(
            log.record_host_observation(
                json!({"event":"observation_unavailable",
            "observation":"provider_context", "text":"private"}),
                "pi",
                None
            )
            .is_err()
        );
        log.record_host_observation(
            json!({"event":"observation_unavailable", "observation":"provider_context"}),
            "pi",
            None,
        )
        .unwrap();
        let records = SessionLog::read(log.path()).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1]["payload"]["grade"], "unavailable");
        assert_eq!(records[1]["payload"]["observer"], "host");
    }
}
