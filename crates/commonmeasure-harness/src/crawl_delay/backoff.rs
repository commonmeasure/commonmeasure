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

impl Backoff {
    /// The reservation whose deadline outlasts the failure. Once another
    /// sender's failure runs to or past that deadline, `until` is the
    /// failure's end and the record holds no separate deadline, so a waiter
    /// reads and waits out the back-off alone.
    fn held(&self) -> Option<&Reservation> {
        self.reservation
            .as_ref()
            .filter(|held| held.failure_until < self.until)
    }
}

/// Why the back-off store could not be read or kept, by what the operator
/// repairs: the host's record, or the directory that holds it. Creating,
/// locking, writing and removing all need the directory; a write replaces
/// the record through a temporary file beside it.
#[derive(Debug)]
enum Fault {
    Record(String),
    Directory(String),
    /// Another process held the host's lock for the whole wait.
    Busy(String),
    /// The host's lock file exists and cannot be opened.
    LockFile(String),
}

impl Fault {
    fn into_reason(self) -> String {
        match self {
            Self::Record(reason)
            | Self::Directory(reason)
            | Self::Busy(reason)
            | Self::LockFile(reason) => reason,
        }
    }

    /// The host's lock refused, by what the operator can do about it: a
    /// writable directory helps neither a busy lock nor a lock file that
    /// refuses this user (EGR-119).
    fn of_lock(host: &str, lock: &Path, refused: crate::declaration::LockRefused) -> Self {
        use crate::declaration::LockRefused;
        let cause = format!("the back-off record for {host} could not be locked");
        match refused {
            LockRefused::Busy(_) => Self::Busy(format!(
                "{cause}: another process on this edge holds {} and did not let go within {}s",
                lock.display(),
                crate::declaration::LOCK_DEADLINE.as_secs()
            )),
            LockRefused::LockFile(reason) => Self::LockFile(format!("{cause}: {reason}")),
            LockRefused::Failed(reason) => Self::Directory(format!("{cause}: {reason}")),
        }
    }
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
            .map_err(Fault::into_reason)
    }

    fn answer(
        &self,
        host: &str,
        status: u16,
        retry_after: Option<&str>,
        now: DateTime<Utc>,
        reservation: Option<&str>,
    ) -> Result<Option<Backoff>, Fault> {
        let failure = is_failure(status);
        if !failure
            && self
                .backoff(host)
                .map_err(Fault::Record)?
                .is_none_or(|old| old.failures == 0 && old.reservation.is_none())
        {
            return Ok(None);
        }
        std::fs::create_dir_all(&self.dir).map_err(|error| {
            Fault::Directory(format!("the back-off store cannot be created: {error}"))
        })?;
        let lock = self.path_for(host, "lock");
        let _lock = crate::declaration::lock(&lock)
            .map_err(|refused| Fault::of_lock(host, &lock, refused))?;
        self.clear_temporaries(host);
        let previous = self.backoff(host).map_err(Fault::Record)?;
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
            std::fs::remove_file(self.backoff_path(host)).map_err(|error| {
                Fault::Directory(format!(
                    "the back-off record for {host} could not be removed: {error}"
                ))
            })?;
            backoff.reservation = None;
        } else {
            self.write_backoff(host, &backoff)?;
        }
        Ok(Some(backoff))
    }

    fn write_backoff(&self, host: &str, backoff: &Backoff) -> Result<(), Fault> {
        let bytes = serde_json::to_vec_pretty(backoff).expect("serialisable back-off");
        crate::declaration::replace(&self.backoff_path(host), &bytes).map_err(|error| {
            Fault::Directory(format!(
                "the back-off for {host} could not be kept: {error}"
            ))
        })
    }

    /// Give back reservation `id` for a request that ended without an HTTP
    /// answer. `until` returns to the failure's end; nothing is counted and
    /// no back-off is extended, because no answer says anything about the
    /// host. A reservation another sender has since taken is left alone.
    fn release(&self, host: &str, id: &str, now: DateTime<Utc>) -> Result<(), String> {
        let _lock = crate::declaration::lock(&self.path_for(host, "lock")).map_err(|error| {
            format!("the back-off record for {host} could not be locked: {error}")
        })?;
        self.clear_temporaries(host);
        let Some(mut backoff) = self.backoff(host)? else {
            return Ok(());
        };
        if backoff
            .reservation
            .as_ref()
            .is_none_or(|held| held.id != id)
        {
            return Ok(());
        }
        backoff.until = backoff
            .reservation
            .take()
            .expect("owned reservation")
            .failure_until;
        if backoff.until <= now && backoff.failures == 0 {
            std::fs::remove_file(self.backoff_path(host)).map_err(|error| error.to_string())
        } else {
            self.write_backoff(host, &backoff)
                .map_err(Fault::into_reason)
        }
    }
}

/// An answer that sets or extends back-off.
fn is_failure(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

/// The back-off reservation one send holds, from [`Pacing::before_send`]
/// until the send's answer is recorded. Dropped on any path that recorded no
/// answer (not sent after all, a transport failure), it releases the
/// reservation, so a connection refused in milliseconds does not hold every
/// request to the host until the request timeout. A release that fails is
/// ignored: the reservation then expires at its deadline, as a stopped
/// process's does.
#[must_use = "dropping it releases the send's back-off reservation"]
pub struct Reserved<'a> {
    pacing: &'a Pacing,
    held: Option<(String, String)>,
}

impl std::fmt::Debug for Reserved<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Reserved")
            .field("held", &self.held)
            .finish_non_exhaustive()
    }
}

impl Drop for Reserved<'_> {
    fn drop(&mut self) {
        if let Some((host, id)) = self.held.take() {
            self.pacing.release(&host, &id);
        }
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

    fn backoff_error(&self, url: &str, fault: Fault) -> String {
        let host = crate::grounding::host_of(url);
        let host = if host.is_empty() { url } else { &host };
        // A hosted tenant can act on neither the store nor its directory, so
        // it is given the operator's remedy, as a crawl-delay fault gives it.
        let reason = match fault {
            Fault::Record(reason) | Fault::Directory(reason) if !self.pace.names_the_edge() => {
                format!(
                    "{}. The back-off record for each host is kept on the edge that serves \
                     this session; its operator can make that store writable or clear it.",
                    self.store.cause(&reason, self.pace)
                )
            }
            Fault::Record(reason) => format!(
                "{reason}. Repair access to or remove that host's back-off file, {}, and try \
                 again.",
                self.store.backoff_path(host).display()
            ),
            Fault::Directory(reason) => format!(
                "{reason}. Make the back-off directory, {}, writable, and try again.",
                self.store.dir().display()
            ),
            Fault::Busy(reason) => format!(
                "{}. Try again once it has finished.",
                self.store.cause(&reason, self.pace)
            ),
            Fault::LockFile(reason) if self.pace.names_the_edge() => format!(
                "{reason}; {}, and try again.",
                crate::declaration::LOCK_FILE_REMEDY
            ),
            Fault::LockFile(reason) => format!(
                "{}. The back-off record for each host is kept on the edge that serves this \
                 session; its operator can repair that lock file.",
                self.store.cause(&reason, self.pace)
            ),
        };
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
    /// Hold the result until the send's answer is recorded with
    /// [`Pacing::answered`]; dropping it before then releases any reservation
    /// this send made.
    pub fn before_send(&self, url: &str, timeout: Duration) -> Result<Reserved<'_>, String> {
        let host = crate::grounding::host_of(url);
        let id = self.before_host_send(
            &host,
            url,
            self.spendable().saturating_sub(self.probe_reserve.get()),
            Some(timeout),
        )?;
        Ok(Reserved {
            pacing: self,
            held: id.map(|id| (host, id)),
        })
    }

    /// What is left of the wait budget. Only a sleep charges it, so a caller
    /// compares it before and after [`Pacing::before_turn`] to tell whether
    /// back-off slept.
    pub fn wait_left(&self) -> Duration {
        self.budget.left()
    }

    /// Check back-off before taking a crawl-delay turn, without reserving a send.
    pub fn before_turn(&self, host: &str, allowance: Duration) -> Result<(), String> {
        self.before_host_send(host, host, allowance, None)
            .map(|_| ())
    }

    /// Release reservation `id` unless an answer has already settled it.
    fn release(&self, host: &str, id: &str) {
        let mut reservations = self.reservations.borrow_mut();
        if reservations.get(host).map(String::as_str) != Some(id) {
            return;
        }
        reservations.remove(host);
        drop(reservations);
        let _ = self.store.release(host, id, Utc::now());
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
                .filter(|date| *date == backoff.until && backoff.held().is_none())
            {
                Some(until) => serde_json::json!({"until": until}),
                None => serde_json::json!({}),
            }
        } else {
            let mut value = serde_json::to_value(backoff).expect("serialisable back-off");
            value.as_object_mut().unwrap().remove("reservation");
            if let Some(reservation) = backoff.held() {
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
    ) -> Result<Option<String>, String> {
        loop {
            let Some(mut backoff) = self
                .store
                .backoff(host)
                .map_err(|reason| self.backoff_error(url, Fault::Record(reason)))?
            else {
                return Ok(None);
            };
            let now = Utc::now();
            let wait = (backoff.until - now).to_std().unwrap_or_default();
            if wait.is_zero() {
                // A healthy host never needs a writable store merely to send.
                if backoff.failures == 0 {
                    return Ok(None);
                }
                let Some(timeout) = timeout else {
                    return Ok(None);
                };
                let lock = self.store.path_for(host, "lock");
                let _lock = crate::declaration::lock(&lock).map_err(|refused| {
                    self.backoff_error(url, Fault::of_lock(host, &lock, refused))
                })?;
                self.store.clear_temporaries(host);
                let Some(current) = self
                    .store
                    .backoff(host)
                    .map_err(|reason| self.backoff_error(url, Fault::Record(reason)))?
                else {
                    return Ok(None);
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
                    .map_err(|fault| self.backoff_error(url, fault))?;
                self.reservations
                    .borrow_mut()
                    .insert(host.to_owned(), id.clone());
                return Ok(Some(id));
            }
            let budget = self.spendable().min(allowance);
            // A reservation may clear early when its answer arrives. Poll it
            // within this caller's budget rather than sleeping its whole timeout.
            let reservation = backoff.held();
            let reserved = reservation.is_some();
            let refused = if reserved {
                budget.is_zero()
            } else {
                wait > budget
            };
            let reason = refused.then(|| {
                if self.pace == Pace::Hosted {
                    match backoff.http_date.filter(|date| *date == backoff.until && !reserved) {
                        Some(until) => format!("{host} is in back-off until {}", until.to_rfc3339()),
                        None => format!("{host} is in back-off"),
                    }
                } else if let Some(held) = reservation {
                    // `until` is the reservation's deadline here. The back-off
                    // and its status are named only while that period lasts:
                    // a reservation taken after it ended cites no failure.
                    let period = if held.failure_until > now {
                        format!(
                            ", and {host} is in back-off until {} after HTTP {}",
                            held.failure_until.to_rfc3339(),
                            backoff.status
                        )
                    } else {
                        String::new()
                    };
                    format!(
                        "another request to {host} is under way and holds the host's turn until \
                         {}{period}; this fetch may spend no more time waiting",
                        backoff.until.to_rfc3339()
                    )
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
        let reservation = self.reservations.borrow().get(&host).cloned();
        let recorded = self.store.answer(
            &host,
            response.status,
            response.headers.get("Retry-After"),
            Utc::now(),
            reservation.as_deref(),
        );
        // An answer that could not be recorded leaves the reservation to
        // this send's `Reserved`, which releases it, only where the answer
        // would have released it anyway. A failure the store could not
        // record keeps the reservation to its deadline: the host asked for
        // a pause, and the reservation is the only pause left to give it.
        if recorded.is_ok() || is_failure(response.status) {
            self.reservations.borrow_mut().remove(&host);
        }
        let backoff = recorded.map_err(|fault| self.backoff_error(url, fault))?;
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
        let file = store.backoff_path("publisher.example");
        let pacing = Pacing::new(store, Pace::Own);
        let reason = pacing
            .before_send("https://publisher.example/page", Duration::from_secs(5))
            .unwrap_err();
        assert!(reason.contains("unreadable"), "{reason}");
        assert!(
            reason.contains(&format!(
                "Repair access to or remove that host's back-off file, {}",
                file.display()
            )),
            "{reason}"
        );
        assert_eq!(pacing.backoff_events()[0].outcome, "unavailable");
    }

    /// The remedy a hosted tenant is given when the back-off store fails:
    /// the operator's, with no file for the tenant to repair.
    const HOSTED_REMEDY: &str = "its operator can make that store writable or clear it.";

    #[test]
    fn a_hosted_unreadable_backoff_names_the_operators_remedy() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        std::fs::create_dir_all(store.dir()).unwrap();
        std::fs::write(store.backoff_path("publisher.example"), b"broken").unwrap();
        let pacing = Pacing::new(store, Pace::Hosted);
        let reason = pacing
            .before_send("https://publisher.example/page", Duration::from_secs(5))
            .unwrap_err();
        assert!(reason.contains("unreadable"), "{reason}");
        assert!(reason.ends_with(HOSTED_REMEDY), "{reason}");
        assert!(!reason.contains("Repair access"), "{reason}");
        assert!(
            !reason.contains(&home.path().display().to_string()),
            "{reason}"
        );
    }

    /// A failure answered into a store whose directory cannot be written:
    /// the lock file cannot be created, so an own edge's remedy names the
    /// directory by path, and a hosted one's names the operator. The host's
    /// back-off file does not exist and is not named.
    #[cfg(unix)]
    #[test]
    fn a_directory_fault_names_the_directory() {
        use std::os::unix::fs::PermissionsExt as _;
        for (pace, named) in [(Pace::Own, true), (Pace::Hosted, false)] {
            let home = tempfile::tempdir().unwrap();
            let store = CrawlDelayStore::open(home.path());
            std::fs::create_dir_all(store.dir()).unwrap();
            std::fs::set_permissions(store.dir(), std::fs::Permissions::from_mode(0o555)).unwrap();
            let pacing = Pacing::new(store.clone(), pace);
            let reason = pacing
                .answered(
                    "https://publisher.example/page",
                    &commonmeasure_http::Response::text(503, "busy"),
                )
                .unwrap_err();
            std::fs::set_permissions(store.dir(), std::fs::Permissions::from_mode(0o755)).unwrap();
            // A hosted tenant can act on neither the store nor its
            // directory, so its remedy is the operator's, as the crawl-delay
            // store's is.
            let remedy = if named {
                format!(
                    "Make the back-off directory, {}, writable, and try again.",
                    store.dir().display()
                )
            } else {
                HOSTED_REMEDY.to_owned()
            };
            assert!(reason.contains("could not be locked"), "{reason}");
            assert!(reason.ends_with(&remedy), "{reason}");
            assert_eq!(reason.contains("Make the back-off"), named, "{reason}");
            assert!(!reason.contains("backoff.json"), "{reason}");
            // The cause names the lock file too: by path on an own edge, and
            // under the directory's name alone on a hosted one.
            let home_path = home.path().display().to_string();
            assert_eq!(reason.contains(&home_path), named, "{reason}");
            if !named {
                assert!(
                    reason.contains("open lock crawl-delay/publisher.example.lock"),
                    "{reason}"
                );
            }
            let events = pacing.backoff_events();
            assert_eq!(events[0].outcome, "unavailable");
            assert_eq!(events[0].reason.as_deref(), Some(reason.as_str()));
        }
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

    /// A waiter refused while another sender holds the host's turn reads
    /// that the turn is held and until when; the back-off period and its
    /// status are named only while that period lasts. Hosted text withholds
    /// both.
    #[test]
    fn a_refusal_during_a_reservation_names_the_reservation() {
        let now = Utc::now().with_nanosecond(0).unwrap();
        let reserved_until = now + chrono::Duration::hours(1);
        for (failure_until, own) in [
            (
                now - chrono::Duration::minutes(1),
                format!(
                    "another request to example.com is under way and holds the host's turn \
                     until {}; this fetch may spend no more time waiting",
                    reserved_until.to_rfc3339()
                ),
            ),
            (
                now + chrono::Duration::minutes(30),
                format!(
                    "another request to example.com is under way and holds the host's turn \
                     until {}, and example.com is in back-off until {} after HTTP 503; this \
                     fetch may spend no more time waiting",
                    reserved_until.to_rfc3339(),
                    (now + chrono::Duration::minutes(30)).to_rfc3339()
                ),
            ),
        ] {
            let home = tempfile::tempdir().unwrap();
            let store = CrawlDelayStore::open(home.path());
            std::fs::create_dir_all(store.dir()).unwrap();
            let held = Backoff {
                failures: 1,
                status: 503,
                until: reserved_until,
                retry_after: false,
                capped: false,
                http_date: None,
                reservation: Some(Reservation {
                    id: "another-sender".to_owned(),
                    failure_until,
                }),
            };
            store.write_backoff("example.com", &held).unwrap();
            for (pace, expected) in [
                (Pace::Own, own.as_str()),
                (Pace::Hosted, "example.com is in back-off"),
            ] {
                let pacing = Pacing::with_budget(store.clone(), Duration::ZERO, pace);
                assert_eq!(
                    pacing
                        .before_send("https://example.com/page", Duration::from_secs(5))
                        .unwrap_err(),
                    expected
                );
            }
        }

        // A reservation for the next minute, then another sender's 503 asking
        // for ten: the failure's end is past the reservation's deadline, so
        // the record holds no separate deadline to name or to poll for. The
        // waiter is refused at once with the plain back-off text.
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        std::fs::create_dir_all(store.dir()).unwrap();
        let reserved = Backoff {
            failures: 1,
            status: 503,
            until: now + chrono::Duration::seconds(60),
            retry_after: false,
            capped: false,
            http_date: None,
            reservation: Some(Reservation {
                id: "another-sender".to_owned(),
                failure_until: now - chrono::Duration::minutes(1),
            }),
        };
        store.write_backoff("example.com", &reserved).unwrap();
        let backoff = store
            .answered("example.com", 503, Some("600"), now)
            .unwrap()
            .expect("a back-off");
        assert_eq!(backoff.until, now + chrono::Duration::seconds(600));
        for (pace, expected) in [
            (
                Pace::Own,
                format!(
                    "example.com is in back-off until {} after HTTP 503; the wait exceeds",
                    backoff.until.to_rfc3339()
                ),
            ),
            (Pace::Hosted, "example.com is in back-off".to_owned()),
        ] {
            let pacing = Pacing::with_budget(store.clone(), Duration::from_secs(5), pace);
            let started = std::time::Instant::now();
            let reason = pacing
                .before_send("https://example.com/page", Duration::from_secs(5))
                .unwrap_err();
            assert!(reason.starts_with(&expected), "{reason}");
            assert!(!reason.contains("under way"), "{reason}");
            assert!(started.elapsed() < Duration::from_secs(1));
            let events = pacing.backoff_events();
            assert_eq!(events.len(), 1, "{events:?}");
            assert_eq!(events[0].outcome, "refused");
            let evidence = events[0].backoff.as_ref().expect("evidence");
            assert!(evidence.get("reserved_until").is_none(), "{evidence}");
        }

        // The same, with the 503 stating an HTTP-date. That date set the
        // end, so once it runs past the reservation's deadline the hosted
        // tenant is told it, in the evidence and in the refusal.
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        std::fs::create_dir_all(store.dir()).unwrap();
        store.write_backoff("example.com", &reserved).unwrap();
        let date = now + chrono::Duration::minutes(10);
        let backoff = store
            .answered(
                "example.com",
                503,
                Some(&date.format("%a, %d %b %Y %H:%M:%S GMT").to_string()),
                now,
            )
            .unwrap()
            .expect("a back-off");
        assert_eq!(backoff.until, date);
        assert!(backoff.reservation.is_some());
        let pacing = Pacing::with_budget(store, Duration::ZERO, Pace::Hosted);
        assert_eq!(
            pacing
                .before_send("https://example.com/page", Duration::from_secs(5))
                .unwrap_err(),
            format!("example.com is in back-off until {}", date.to_rfc3339())
        );
        let events = pacing.backoff_events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].outcome, "refused");
        assert_eq!(events[0].backoff, Some(serde_json::json!({"until": date})));
    }

    /// A record whose back-off period has ended and whose host's turn is
    /// held by `holder` for the next hour.
    fn held_by(store: &CrawlDelayStore, holder: &str) -> Backoff {
        let now = Utc::now().with_nanosecond(0).unwrap();
        let held = Backoff {
            failures: 1,
            status: 503,
            until: now + chrono::Duration::hours(1),
            retry_after: false,
            capped: false,
            http_date: None,
            reservation: Some(Reservation {
                id: holder.to_owned(),
                failure_until: now - chrono::Duration::minutes(1),
            }),
        };
        std::fs::create_dir_all(store.dir()).unwrap();
        store.write_backoff("example.com", &held).unwrap();
        held
    }

    /// Only the holder's own answer releases its reservation. An answer
    /// carrying another sender's reservation, success or failure, leaves the
    /// holder's reservation and its deadline in place; a failure moves only
    /// the end of the back-off period it starts.
    #[test]
    fn another_senders_answer_leaves_the_reservation_in_place() {
        for status in [200, 503] {
            let home = tempfile::tempdir().unwrap();
            let store = CrawlDelayStore::open(home.path());
            let held = held_by(&store, "holder");
            store
                .answer("example.com", status, None, Utc::now(), Some("another"))
                .unwrap();
            let kept = store.backoff("example.com").unwrap().unwrap();
            let holder = kept.reservation.map(|reservation| reservation.id);
            assert_eq!(holder.as_deref(), Some("holder"), "HTTP {status}");
            assert_eq!(kept.until, held.until, "HTTP {status}");
        }
    }

    /// Every poll of another sender's reservation is charged to the wait
    /// budget and added to the one `waited` event, so the event's `wait_ms`
    /// is what the waiter spent.
    #[test]
    fn every_poll_of_a_reservation_is_added_to_the_wait() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        held_by(&store, "holder");
        let budget = Duration::from_millis(200);
        let waiter = Pacing::with_budget(store, budget, Pace::Own);
        assert!(
            waiter
                .before_send("https://example.com/page", Duration::from_secs(5))
                .is_err()
        );
        let events = waiter.backoff_events();
        let outcomes: Vec<_> = events.iter().map(|event| event.outcome.as_str()).collect();
        assert_eq!(outcomes, ["waited", "refused"]);
        assert_eq!(waiter.wait_left(), Duration::ZERO);
        assert_eq!(events[0].wait_ms, Some(millis(budget)));
    }

    /// A waiter polls a reservation rather than sleeping to its deadline, so
    /// it sends soon after the holder's answer releases it, with most of its
    /// budget left. The holder answers from this thread once the waiter,
    /// on its own thread, has started; the waiter's budget is far longer
    /// than that.
    #[test]
    fn a_waiter_sends_once_the_reservation_is_released() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        held_by(&store, "holder");
        let budget = Duration::from_secs(30);
        let (started, waiting) = std::sync::mpsc::channel();
        let waiter = {
            let store = store.clone();
            std::thread::spawn(move || {
                let pacing = Pacing::with_budget(store, budget, Pace::Own);
                started.send(()).unwrap();
                let sent = pacing
                    .before_send("https://example.com/page", Duration::from_secs(5))
                    .map(drop);
                (sent, pacing.backoff_events(), pacing.wait_left())
            })
        };
        waiting.recv().unwrap();
        std::thread::sleep(Duration::from_millis(500));
        store
            .answer("example.com", 200, None, Utc::now(), Some("holder"))
            .unwrap();
        let (sent, events, left) = waiter.join().unwrap();
        assert!(sent.is_ok(), "{sent:?}");
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].outcome, "waited");
        assert!(left > Duration::ZERO, "the waiter slept its whole budget");
        assert_eq!(events[0].wait_ms, Some(millis(budget - left)));
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
        let _sent = first
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
        let _sent = waiter
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

    /// Scratch S7 from the second back-off review: a request that reserved
    /// the host and ended with no answer held the same call's next request
    /// for the whole of its timeout. Released, the next request proceeds
    /// without a `waited` event.
    #[test]
    fn a_request_with_no_answer_releases_its_reservation() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        store
            .answered("example.com", 503, Some("0"), Utc::now())
            .unwrap();
        let pacing = Pacing::new(store.clone(), Pace::Own);
        let licence = pacing
            .before_send("https://example.com/licence", Duration::from_secs(2))
            .unwrap();
        drop(licence);
        let _page = pacing
            .before_send("https://example.com/page", Duration::from_secs(2))
            .unwrap();
        assert!(
            pacing
                .backoff_events()
                .iter()
                .all(|event| event.outcome != "waited"),
            "{:?}",
            pacing.backoff_events()
        );
    }

    /// Ownership: a holder whose reservation expired and was taken by a newer
    /// sender neither releases the newer reservation by ending without an
    /// answer nor by answering late.
    #[test]
    fn an_expired_holder_leaves_the_newer_reservation_in_place() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        store
            .answered("example.com", 503, Some("0"), Utc::now())
            .unwrap();
        let url = "https://example.com/page";
        let unanswered = Pacing::new(store.clone(), Pace::Own);
        let expired = unanswered
            .before_send(url, Duration::from_millis(1))
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let late = Pacing::new(store.clone(), Pace::Own);
        let _late = late.before_send(url, Duration::from_millis(1)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let newer = Pacing::new(store.clone(), Pace::Own);
        let _held = newer.before_send(url, Duration::from_secs(30)).unwrap();
        let reserved = store.backoff("example.com").unwrap().unwrap().reservation;
        assert!(reserved.is_some());

        drop(expired);
        assert_eq!(
            store.backoff("example.com").unwrap().unwrap().reservation,
            reserved,
            "a release by an expired holder"
        );
        late.answered(url, &commonmeasure_http::Response::text(200, "ok"))
            .unwrap();
        assert_eq!(
            store
                .backoff("example.com")
                .unwrap()
                .and_then(|backoff| backoff.reservation),
            reserved,
            "a late answer from an expired holder"
        );
    }

    /// A release gives back only the reservation: back-off another request's
    /// failure set while it was held stays in force, with its count.
    #[test]
    fn a_release_keeps_the_backoff_in_force() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        store
            .answered("example.com", 503, Some("0"), Utc::now())
            .unwrap();
        let pacing = Pacing::new(store.clone(), Pace::Own);
        let sent = pacing
            .before_send("https://example.com/page", Duration::from_secs(5))
            .unwrap();
        store
            .answered("example.com", 503, Some("600"), Utc::now())
            .unwrap();
        let held = store.backoff("example.com").unwrap().unwrap();
        let failure_until = held.reservation.as_ref().unwrap().failure_until;

        drop(sent);
        let released = store.backoff("example.com").unwrap().unwrap();
        assert_eq!(released.reservation, None);
        assert_eq!(released.until, failure_until);
        assert_eq!(released.failures, held.failures);
        let waiter = Pacing::with_budget(store, Duration::from_millis(20), Pace::Own);
        assert!(
            waiter
                .before_send("https://example.com/page", Duration::from_secs(5))
                .is_err()
        );
    }

    /// EGR-119. An answer that cannot be recorded because another process
    /// holds the host's lock says so and names the lock; one refused because
    /// the lock file exists and cannot be opened says what to do with that
    /// file. Neither asks for a writable back-off directory, which it is.
    #[cfg(unix)]
    #[test]
    fn a_busy_lock_and_an_unopenable_lock_file_each_name_their_own_remedy() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        std::fs::create_dir_all(store.dir()).unwrap();
        let lock_path = store.path_for("publisher.example", "lock");
        let pacing = Pacing::new(store.clone(), Pace::Own);
        let answer = || {
            pacing
                .answered(
                    "https://publisher.example/page",
                    &commonmeasure_http::Response::text(503, "busy"),
                )
                .unwrap_err()
        };

        let held = crate::declaration::lock(&lock_path).unwrap();
        let busy = answer();
        drop(held);
        assert!(busy.contains(&lock_path.display().to_string()), "{busy}");
        assert!(
            busy.contains("another process on this edge holds"),
            "{busy}"
        );
        assert!(!busy.contains("writable"), "{busy}");
        assert!(!busy.contains("no edit was made"), "{busy}");

        if std::fs::metadata(home.path()).unwrap().uid() == 0 {
            return;
        }
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let unopenable = answer();
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            unopenable.contains(&lock_path.display().to_string()),
            "{unopenable}"
        );
        assert!(
            unopenable.contains("make that lock file readable and writable by this user"),
            "{unopenable}"
        );
        assert!(
            !unopenable.contains("Make the back-off directory"),
            "{unopenable}"
        );
    }

    use chrono::Timelike;
}
