//! Response-driven pacing shares the crawl-delay directory and host lock.
//! A separate file preserves the existing crawl-delay turn and evidence.

use super::*;

const RETRY_AFTER_CAP: u64 = 3_600;
const EXPONENTIAL_CAP: u64 = 900;

/// The last failure and the earliest permitted send after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backoff {
    pub failures: u32,
    pub status: u16,
    pub until: DateTime<Utc>,
    /// Whether a usable Retry-After supplied the interval.
    pub retry_after: bool,
    /// Whether Retry-After exceeded the one-hour limit.
    pub capped: bool,
    /// The host's absolute Retry-After statement, only when it set the end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_date: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reservation: Option<Reservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Reservation {
    // A late answer must not release a newer sender's reservation.
    id: String,
    // While `until` holds the request timeout, this keeps the failure's end
    // for counting periods and for evidence about the response that set it.
    failure_until: DateTime<Utc>,
}

/// One response or send decision, separate from the crawl-delay ruling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackoffEvent {
    /// Request URL, or host for a crawl-delay turn taken before transport.
    pub target: String,
    /// `set`, `reset`, `waited`, `refused` or `unavailable`.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Own edges retain the failure details; hosted evidence may contain
    /// only the host's absolute Retry-After date, or an empty object.
    pub backoff: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl CrawlDelayStore {
    fn backoff_path(&self, host: &str) -> PathBuf {
        self.path_for(host, "backoff.json")
    }

    /// Read the response-driven pace. An unreadable store cannot grant a send.
    pub fn backoff(&self, host: &str) -> Result<Option<Backoff>, String> {
        match std::fs::read(self.backoff_path(host)) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| format!("the back-off record for {host} is unreadable: {error}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!(
                "the back-off record for {host} cannot be read: {error}"
            )),
        }
    }

    /// Record an actual HTTP answer under the same lock as crawl-delay turns.
    /// `now` is the response time; callers may inject it in deterministic tests.
    /// A successful answer resets the consecutive failure count but does not
    /// revoke a wait already imposed by a concurrent request's failure.
    pub fn answered(
        &self,
        host: &str,
        status: u16,
        retry_after: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Option<Backoff>, String> {
        self.answer(host, status, retry_after, now, None)
    }

    fn answer(
        &self,
        host: &str,
        status: u16,
        retry_after: Option<&str>,
        now: DateTime<Utc>,
        reservation: Option<&str>,
    ) -> Result<Option<Backoff>, String> {
        let failure = status == 429 || (500..600).contains(&status);
        if !failure
            && self
                .backoff(host)?
                .is_none_or(|old| old.failures == 0 && old.reservation.is_none())
        {
            return Ok(None);
        }
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| format!("the back-off store cannot be created: {error}"))?;
        let _lock = crate::declaration::lock(&self.path_for(host, "lock")).map_err(|error| {
            format!("the back-off record for {host} could not be locked: {error}")
        })?;
        self.clear_temporaries(host);
        let previous = self.backoff(host)?;
        let owns = previous
            .as_ref()
            .and_then(|old| old.reservation.as_ref())
            .is_some_and(|held| Some(held.id.as_str()) == reservation);
        let mut backoff = if failure {
            let failures = previous.as_ref().map_or(1, |old| {
                if now
                    >= old
                        .reservation
                        .as_ref()
                        .map_or(old.until, |held| held.failure_until)
                    || old.failures == 0
                {
                    old.failures.saturating_add(1)
                } else {
                    old.failures
                }
            });
            let supplied = matches!(status, 429 | 503)
                .then(|| retry_after.and_then(|value| retry_after_seconds(value, now)))
                .flatten();
            let seconds = supplied.unwrap_or_else(|| {
                10_u64
                    .saturating_mul(1_u64 << failures.saturating_sub(1).min(7))
                    .min(EXPONENTIAL_CAP)
            });
            let http_date = supplied
                .filter(|seconds| *seconds <= RETRY_AFTER_CAP)
                .and_then(|_| retry_after.and_then(retry_after_date));
            let until = http_date
                .unwrap_or_else(|| {
                    now + chrono::Duration::seconds(seconds.min(RETRY_AFTER_CAP) as i64)
                })
                .max(now);
            let mut next = Backoff {
                failures,
                status,
                until,
                retry_after: supplied.is_some(),
                capped: supplied.is_some_and(|seconds| seconds > RETRY_AFTER_CAP),
                http_date: http_date.filter(|date| *date == until),
                reservation: None,
            };
            if let Some(old) = previous {
                let old_until = old
                    .reservation
                    .as_ref()
                    .map_or(old.until, |held| held.failure_until);
                if old_until > next.until {
                    next.status = old.status;
                    next.until = old_until;
                    next.retry_after = old.retry_after;
                    next.capped = old.capped;
                    next.http_date = old.http_date;
                }
                if !owns && let Some(held) = old.reservation {
                    let failure_until = next.until;
                    next.until = next.until.max(old.until);
                    next.reservation = Some(Reservation {
                        failure_until,
                        ..held
                    });
                }
            }
            next
        } else {
            let Some(mut old) = previous else {
                return Ok(None);
            };
            if old.failures == 0 && !owns {
                return Ok(None);
            }
            old.failures = 0;
            if owns {
                old.until = old
                    .reservation
                    .take()
                    .expect("owned reservation")
                    .failure_until;
            }
            old
        };
        if backoff.until <= now && backoff.failures == 0 {
            std::fs::remove_file(self.backoff_path(host)).map_err(|error| error.to_string())?;
            backoff.reservation = None;
        } else {
            self.write_backoff(host, &backoff)?;
        }
        Ok(Some(backoff))
    }

    fn write_backoff(&self, host: &str, backoff: &Backoff) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(backoff).map_err(|error| error.to_string())?;
        crate::declaration::replace(&self.backoff_path(host), &bytes)
            .map_err(|error| format!("the back-off for {host} could not be kept: {error}"))
    }
}

fn retry_after_seconds(value: &str, now: DateTime<Utc>) -> Option<u64> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(value.parse().unwrap_or(u64::MAX));
    }
    let at = retry_after_date(value)?;
    let millis = (at - now).num_milliseconds().max(0) as u64;
    Some(millis.div_ceil(1_000))
}

fn retry_after_date(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(value.trim())
        .ok()
        .map(|date| date.with_timezone(&Utc))
        .or_else(|| {
            ["%A, %d-%b-%y %H:%M:%S GMT", "%a %b %e %H:%M:%S %Y"]
                .iter()
                .find_map(|format| {
                    chrono::NaiveDateTime::parse_from_str(value.trim(), format)
                        .ok()
                        .map(|date| date.and_utc())
                })
        })
}

impl Pacing {
    /// Back-off decisions made during this crossing, including probe requests.
    pub fn backoff_events(&self) -> Vec<BackoffEvent> {
        self.backoff_events.borrow().clone()
    }

    fn backoff_error(&self, url: &str, reason: String) -> String {
        let host = crate::grounding::host_of(url);
        let host = if host.is_empty() { url } else { &host };
        let path = self.store.backoff_path(host);
        let file = if self.pace.names_the_edge() {
            path.display().to_string()
        } else {
            path.file_name().unwrap().to_string_lossy().into_owned()
        };
        let reason = format!(
            "{reason}. Repair access to or remove that host's back-off file, {file}, and try again."
        );
        self.backoff_events.borrow_mut().push(BackoffEvent {
            target: url.to_owned(),
            outcome: "unavailable".to_owned(),
            backoff: None,
            wait_ms: None,
            budget_ms: None,
            reason: Some(reason.clone()),
        });
        reason
    }

    /// Check immediately before every transport send, after any crawl-delay
    /// turn. Only the remaining back-off is slept, so the later limit wins.
    /// Recheck after sleeping because another process may have extended it.
    pub fn before_send(&self, url: &str, timeout: Duration) -> Result<(), String> {
        self.before_host_send(
            &crate::grounding::host_of(url),
            url,
            self.spendable().saturating_sub(self.probe_reserve.get()),
            Some(timeout),
        )
    }

    /// Check back-off before taking a crawl-delay turn, without reserving a send.
    pub fn before_turn(&self, host: &str, allowance: Duration) -> Result<(), String> {
        self.before_host_send(host, host, allowance, None)
    }

    /// The most recent back-off refusal, for the crossing's explanation.
    pub fn backoff_refusal(&self) -> Option<String> {
        self.backoff_events
            .borrow()
            .iter()
            .rev()
            .find(|event| matches!(event.outcome.as_str(), "refused" | "unavailable"))
            .and_then(|event| event.reason.clone())
    }

    fn evidence(&self, backoff: &Backoff) -> serde_json::Value {
        if self.pace == Pace::Hosted {
            match backoff
                .http_date
                .filter(|date| *date == backoff.until && backoff.reservation.is_none())
            {
                Some(until) => serde_json::json!({"until": until}),
                None => serde_json::json!({}),
            }
        } else {
            let mut value = serde_json::to_value(backoff).expect("serialisable back-off");
            value.as_object_mut().unwrap().remove("reservation");
            if let Some(reservation) = &backoff.reservation {
                value["until"] = serde_json::json!(reservation.failure_until);
                value["reserved_until"] = serde_json::json!(backoff.until);
            }
            value
        }
    }

    fn before_host_send(
        &self,
        host: &str,
        url: &str,
        mut allowance: Duration,
        timeout: Option<Duration>,
    ) -> Result<(), String> {
        loop {
            let Some(mut backoff) = self
                .store
                .backoff(host)
                .map_err(|reason| self.backoff_error(url, reason))?
            else {
                return Ok(());
            };
            let now = Utc::now();
            let wait = (backoff.until - now).to_std().unwrap_or_default();
            if wait.is_zero() {
                // A healthy host never needs a writable store merely to send.
                if backoff.failures == 0 {
                    return Ok(());
                }
                let Some(timeout) = timeout else {
                    return Ok(());
                };
                let _lock = crate::declaration::lock(&self.store.path_for(host, "lock"))
                    .map_err(|error| self.backoff_error(url, format!("back-off lock: {error}")))?;
                self.store.clear_temporaries(host);
                let Some(current) = self
                    .store
                    .backoff(host)
                    .map_err(|reason| self.backoff_error(url, reason))?
                else {
                    return Ok(());
                };
                if current != backoff {
                    continue;
                }
                let id = uuid::Uuid::new_v4().to_string();
                let failure_until = backoff
                    .reservation
                    .as_ref()
                    .map_or(backoff.until, |r| r.failure_until);
                backoff.reservation = Some(Reservation {
                    id: id.clone(),
                    failure_until,
                });
                backoff.until =
                    Utc::now() + chrono::Duration::from_std(timeout.min(self.left())).unwrap();
                self.store
                    .write_backoff(host, &backoff)
                    .map_err(|reason| self.backoff_error(url, reason))?;
                self.reservations.borrow_mut().insert(host.to_owned(), id);
                return Ok(());
            }
            let budget = self.spendable().min(allowance);
            // A reservation may clear early when its answer arrives. Poll it
            // within this caller's budget rather than sleeping its whole timeout.
            let reserved = backoff.reservation.is_some();
            let refused = if reserved {
                budget.is_zero()
            } else {
                wait > budget
            };
            let reason = refused.then(|| {
                if self.pace == Pace::Hosted {
                    match backoff.http_date.filter(|date| *date == backoff.until && backoff.reservation.is_none()) {
                        Some(until) => format!("{host} is in back-off until {}", until.to_rfc3339()),
                        None => format!("{host} is in back-off"),
                    }
                } else {
                    format!("{host} is in back-off until {} after HTTP {}; the wait exceeds the {}s this fetch may still spend waiting",
                        backoff.until.to_rfc3339(), backoff.status, seconds(millis(budget)))
                }
            });
            let sleep = if reserved {
                wait.min(budget).min(Duration::from_millis(50))
            } else {
                wait
            };
            let evidence = Some(self.evidence(&backoff));
            let mut events = self.backoff_events.borrow_mut();
            if reserved
                && !refused
                && let Some(last) = events.last_mut()
                && last.target == url
                && last.outcome == "waited"
                && last.backoff == evidence
            {
                if let Some(wait_ms) = &mut last.wait_ms {
                    *wait_ms += millis(sleep);
                }
            } else {
                events.push(BackoffEvent {
                    target: url.to_owned(),
                    outcome: if refused { "refused" } else { "waited" }.to_owned(),
                    backoff: evidence,
                    wait_ms: (self.pace == Pace::Own)
                        .then(|| millis(if refused { wait } else { sleep })),
                    budget_ms: Some(millis(budget)),
                    reason: reason.clone(),
                });
            }
            drop(events);
            if let Some(reason) = reason {
                return Err(reason);
            }
            self.budget.sleep(sleep);
            allowance = allowance.saturating_sub(sleep);
        }
    }

    /// Observe every response, including redirects and declaration probes.
    pub fn answered(
        &self,
        url: &str,
        response: &commonmeasure_http::Response,
    ) -> Result<(), String> {
        let host = crate::grounding::host_of(url);
        let backoff = self
            .store
            .answer(
                &host,
                response.status,
                response.headers.get("Retry-After"),
                Utc::now(),
                self.reservations.borrow_mut().remove(&host).as_deref(),
            )
            .map_err(|reason| self.backoff_error(url, reason))?;
        if let Some(backoff) = backoff {
            self.backoff_events.borrow_mut().push(BackoffEvent {
                target: url.to_owned(),
                outcome: if backoff.failures == 0 {
                    "reset"
                } else {
                    "set"
                }
                .to_owned(),
                backoff: Some(self.evidence(&backoff)),
                wait_ms: None,
                budget_ms: None,
                reason: None,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        "2026-09-24T04:00:00Z".parse().unwrap()
    }

    #[test]
    fn failures_double_cap_and_reset_using_response_times() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        let mut at = now();
        for expected in [10, 20, 40, 80, 160, 320, 640, 900, 900] {
            let answer = store
                .answered("publisher.example", 502, Some("999"), at)
                .unwrap()
                .unwrap();
            assert_eq!(answer.until, at + chrono::Duration::seconds(expected));
            assert!(!answer.retry_after);
            at = answer.until;
        }
        let reset = store
            .answered("publisher.example", 404, None, at)
            .unwrap()
            .unwrap();
        assert_eq!(reset.failures, 0);
        let again = store
            .answered("publisher.example", 429, None, at)
            .unwrap()
            .unwrap();
        assert_eq!(again.failures, 1);
        assert_eq!(again.until, at + chrono::Duration::seconds(10));
    }

    #[test]
    fn retry_after_seconds_dates_invalid_values_and_cap() {
        for (status, value, seconds, supplied, capped) in [
            (429, "25", 25, true, false),
            (503, "Thu, 24 Sep 2026 04:01:00 GMT", 60, true, false),
            (503, "Thursday, 24-Sep-26 04:01:00 GMT", 60, true, false),
            (503, "Thu Sep 24 04:01:00 2026", 60, true, false),
            (429, "7200", 3600, true, true),
            (503, "9999999999999999999999999999999", 3600, true, true),
            (429, "-1", 10, false, false),
            (503, "bad date", 10, false, false),
            (429, "0", 0, true, false),
            (503, "Wed, 23 Sep 2026 04:00:00 GMT", 0, true, false),
        ] {
            let home = tempfile::tempdir().unwrap();
            let store = CrawlDelayStore::open(home.path());
            let answer = store
                .answered("publisher.example", status, Some(value), now())
                .unwrap()
                .unwrap();
            assert_eq!(
                answer.until,
                now() + chrono::Duration::seconds(seconds),
                "{value}"
            );
            assert_eq!(answer.retry_after, supplied, "{value}");
            assert_eq!(answer.capped, capped, "{value}");
        }
    }

    #[test]
    fn success_resets_count_without_revoking_an_existing_wait() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        let first = store
            .answered("publisher.example", 429, Some("3600"), now())
            .unwrap()
            .unwrap();
        let second = store
            .answered("publisher.example", 200, None, now())
            .unwrap()
            .unwrap();
        assert_eq!(second.failures, 0);
        assert_eq!(second.until, first.until);
        let third = store
            .answered("publisher.example", 500, None, now())
            .unwrap()
            .unwrap();
        assert_eq!(third.failures, 1);
        assert_eq!(
            third.until, first.until,
            "a later answer must not shorten an existing wait"
        );
    }

    #[test]
    fn unreadable_backoff_refuses_without_sending() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        std::fs::create_dir_all(store.dir()).unwrap();
        std::fs::write(store.backoff_path("publisher.example"), b"broken").unwrap();
        let pacing = Pacing::new(store, Pace::Own);
        assert!(
            pacing
                .before_send("https://publisher.example/page", Duration::from_secs(5))
                .unwrap_err()
                .contains("unreadable")
        );
        assert_eq!(pacing.backoff_events()[0].outcome, "unavailable");
    }
    #[test]
    fn backoff_does_not_spend_a_probes_reserved_budget_or_rewrite_a_crawl_turn() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        let at = Utc::now();
        store.take_turn(
            "publisher.example",
            Duration::from_secs(2),
            WAIT_BUDGET,
            at,
            Pace::Own,
        );
        let original = std::fs::read(store.path_of("publisher.example")).unwrap();
        store
            .answered("publisher.example", 429, Some("10"), at)
            .unwrap();
        let pacing = Pacing::new(store.clone(), Pace::Own);
        pacing.learn("publisher.example", 2_000);
        assert!(
            pacing
                .free_probe("https://publisher.example/manifest", at)
                .is_err()
        );
        assert_eq!(pacing.backoff_events()[0].budget_ms, Some(0));
        assert_eq!(
            std::fs::read(store.path_of("publisher.example")).unwrap(),
            original
        );
        assert_eq!(pacing.spendable_now(), WAIT_BUDGET);
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;

    #[test]
    fn concurrent_failures_count_once_and_keep_the_failure_that_set_the_end() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        let now = Utc::now();
        let first = store
            .answered("example.com", 503, Some("3600"), now)
            .unwrap()
            .unwrap();
        let later = store
            .answered("example.com", 500, None, now + chrono::Duration::seconds(1))
            .unwrap()
            .unwrap();
        assert_eq!(later.failures, 1);
        assert_eq!(later.status, first.status);
        assert_eq!(later.until, first.until);
        assert_eq!(later.retry_after, first.retry_after);
        assert_eq!(later.capped, first.capped);
    }

    #[test]
    fn hosted_refusals_only_disclose_a_host_stated_absolute_date() {
        let capped_date = (Utc::now() + chrono::Duration::hours(2))
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        for header in [None, Some("120"), Some(capped_date.as_str())] {
            let home = tempfile::tempdir().unwrap();
            let store = CrawlDelayStore::open(home.path());
            let now = Utc::now();
            let held = store
                .answered("example.com", 503, header, now)
                .unwrap()
                .unwrap();
            assert_eq!(held.capped, header == Some(capped_date.as_str()));
            let pacing = Pacing::with_budget(store, Duration::ZERO, Pace::Hosted);
            assert_eq!(
                pacing
                    .before_send("https://example.com/page", Duration::from_secs(5))
                    .unwrap_err(),
                "example.com is in back-off"
            );
            let event = serde_json::to_value(&pacing.backoff_events()[0]).unwrap();
            assert_eq!(event["backoff"], serde_json::json!({}));
            assert!(event.get("wait_ms").is_none());
        }
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        let now = Utc::now();
        let until = (now + chrono::Duration::minutes(5))
            .with_nanosecond(0)
            .unwrap();
        store
            .answered(
                "example.com",
                503,
                Some(&until.format("%a, %d %b %Y %H:%M:%S GMT").to_string()),
                now,
            )
            .unwrap();
        let pacing = Pacing::with_budget(store, Duration::ZERO, Pace::Hosted);
        let reason = pacing
            .before_send("https://example.com/page", Duration::from_secs(5))
            .unwrap_err();
        assert_eq!(
            reason,
            format!("example.com is in back-off until {}", until.to_rfc3339())
        );
        assert_eq!(
            pacing.backoff_events()[0].backoff,
            Some(serde_json::json!({"until":until}))
        );
    }

    #[test]
    fn final_send_check_keeps_the_probes_reserve() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        let pacing = Pacing::new(store.clone(), Pace::Own);
        pacing
            .probe_reserving("https://example.com/licence", Utc::now(), WAIT_BUDGET)
            .unwrap();
        store
            .answered("example.com", 503, None, Utc::now())
            .unwrap();
        assert!(
            pacing
                .before_send("https://example.com/licence", Duration::from_secs(5))
                .is_err()
        );
        assert_eq!(pacing.backoff_events()[0].budget_ms, Some(0));
        assert_eq!(pacing.spendable_now(), WAIT_BUDGET);
    }

    #[test]
    fn an_answer_releases_the_reservation_and_waiters_read_the_result() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        store
            .answered("example.com", 503, Some("0"), Utc::now())
            .unwrap();
        let first = Pacing::new(store.clone(), Pace::Own);
        first
            .before_send("https://example.com/page", Duration::from_secs(5))
            .unwrap();
        let waiter = Pacing::with_budget(store.clone(), Duration::from_millis(20), Pace::Own);
        assert!(
            waiter
                .before_send("https://example.com/page", Duration::from_secs(5))
                .is_err()
        );
        first
            .answered(
                "https://example.com/page",
                &commonmeasure_http::Response::text(200, "ok"),
            )
            .unwrap();
        assert!(store.backoff("example.com").unwrap().is_none());
        waiter
            .before_send("https://example.com/page", Duration::from_secs(5))
            .unwrap();
    }

    #[test]
    fn backoff_writes_sweep_both_kinds_of_interrupted_writes() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        std::fs::create_dir_all(store.dir()).unwrap();
        for name in ["example.com.json.1.tmp", "example.com.backoff.json.2.tmp"] {
            std::fs::write(store.dir().join(name), "interrupted").unwrap();
        }
        store
            .answered("example.com", 503, None, Utc::now())
            .unwrap();
        for entry in std::fs::read_dir(store.dir()).unwrap() {
            assert!(
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            );
        }
    }

    #[test]
    fn historical_delay_rulings_round_trip_without_backoff_fields() {
        for old in [
            serde_json::json!({"host":"example.com","delay_ms":2000,"outcome":"waited","wait_ms":1500,"budget_ms":60000}),
            serde_json::json!({"host":"example.com","delay_ms":2000,"outcome":"clear","wait_ms":0,"budget_ms":60000}),
            serde_json::json!({"host":"example.com","delay_ms":2000,"outcome":"refused","wait_ms":3000,"budget_ms":1000,"next_at":"2026-09-24T04:00:00Z"}),
            serde_json::json!({"host":"example.com","delay_ms":2000,"outcome":"refused","budget_ms":1000}),
            serde_json::json!({"host":"example.com","delay_ms":2000,"outcome":"unavailable","wait_ms":0,"budget_ms":60000,"unavailable":"store unavailable"}),
            serde_json::json!({"host":"example.com","delay_ms":2000,"outcome":"waited","wait_ms":2000,"budget_ms":60000,"recovered":"discarded unreadable turn","licence_first":"https://example.com/licence"}),
        ] {
            let ruling: DelayRuling = serde_json::from_value(old.clone()).unwrap();
            assert_eq!(serde_json::to_value(ruling).unwrap(), old);
        }
    }

    use chrono::Timelike;
}
