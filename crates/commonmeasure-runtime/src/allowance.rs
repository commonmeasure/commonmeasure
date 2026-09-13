//! The cumulative allowance ledger: stateful, local, per principal.
//!
//! A `ContextJob`'s cost cap judges one purchase; an allowance judges a
//! period of them, so it needs state that survives the process — and state
//! shared by every process on this machine, because two concurrent buyers
//! must not each see the full remaining amount (`DECISIONS.md` §Delegated authority and fleet management). The
//! declaration lives in the operator's policy artefact; this module is the
//! enforcement side only, and it stays effective offline (`DECISIONS.md`
//! §2026-08-21).
//!
//! The ledger is an append-only NDJSON file under the operator home,
//! `allowance/ledger.ndjson`, every mutation taken under an exclusive lock on
//! `allowance/ledger.lock` (the same bounded [`crate::declaration::lock`] the
//! declared artefacts use). Append-only is the point, not an implementation
//! detail: every unit ever reserved, committed or released is one inspectable
//! line with its reason, so "the ledger explains every unit" is a property of
//! the file, not of a viewer.
//!
//! Three entry kinds. A `reserved` entry holds authority ahead of a purchase
//! and counts against the allowance until an entry with the same id resolves
//! it; a `committed` entry is spend that happened, counted in its own
//! currency for as long as the file exists; a `released` entry ends a
//! reservation and counts nothing. A reservation nobody resolves — a buyer
//! that crashed between reserve and receipt — is recovered by rule, not by
//! judgement: it carries an explicit expiry, and the next writer of any kind
//! releases expired reservations first, each with a line naming the rule that
//! did it.
//!
//! No exchange rates anywhere: sums only ever add amounts in one currency,
//! and a charge in another currency or a native provider unit is incomparable
//! with a declaration rather than convertible. Unknown stays unknown
//! (`docs/FAIL-POLICY.md` §7): an unreadable ledger is an error the caller
//! must treat as such, never an empty one.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use commonmeasure_types::{
    AcquisitionCharge, AllowanceDeclaration, AllowancePeriod, Gap, GapReason, Money, PolicyMode,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::declaration;
use crate::policy::Ruling;

/// How long a reservation holds authority before any writer may reclaim it.
/// A quote-to-receipt exchange completes in seconds; ten minutes is generous
/// enough that only a genuinely dead buyer is ever swept.
const RESERVATION_TTL: chrono::Duration = chrono::Duration::minutes(10);

/// Everything the run path needs to consult the allowance: who is spending,
/// what they declared, and where the ledger lives. Built by the caller that
/// holds the operator home (`commonmeasure-cli`) or the session policy (`commonmeasure-harness`);
/// the runtime itself resolves neither.
#[derive(Debug)]
pub struct AllowanceContext {
    pub principal: String,
    pub declarations: Vec<AllowanceDeclaration>,
    pub ledger: Ledger,
}

impl AllowanceContext {
    pub fn new(home: &Path, principal: String, declarations: Vec<AllowanceDeclaration>) -> Self {
        Self {
            principal,
            declarations,
            ledger: Ledger::in_home(home),
        }
    }

    /// This context's principal and declarations as one borrowed unit, the
    /// shape every [`Ledger`] operation is keyed by.
    pub fn account(&self) -> Account<'_> {
        Account {
            principal: &self.principal,
            declarations: &self.declarations,
        }
    }
}

/// Who is spending and what they declared: the key of every ledger
/// operation, borrowed together so a caller cannot pair one principal's name
/// with another's declarations.
#[derive(Debug, Clone, Copy)]
pub struct Account<'a> {
    pub principal: &'a str,
    pub declarations: &'a [AllowanceDeclaration],
}

/// The ledger file and its lock. Cheap to hold; every operation opens, locks,
/// reads and appends on its own, so two processes interleave at operation
/// granularity and never see a half-written line.
#[derive(Debug, Clone)]
pub struct Ledger {
    directory: PathBuf,
}

/// One line of the ledger. The tag is the entry's own word for itself, so
/// the file reads as what happened: reserved, committed, released.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
enum LedgerEntry {
    Reserved {
        id: Uuid,
        principal: String,
        amount: Money,
        /// Period name → the key of the period this reservation falls in,
        /// one per declared period, in the declaration's own zone.
        period_keys: BTreeMap<String, String>,
        at: String,
        expires_at: String,
        pid: u32,
        /// What was being bought, in the caller's words — run and plan, or
        /// session — so a reader of the raw file can place every line.
        context: String,
    },
    Committed {
        id: Uuid,
        /// The reservation this settles, when one was held. A commit with no
        /// reservation is spend that was only knowable after the fact.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reserved_id: Option<Uuid>,
        principal: String,
        amount: Money,
        period_keys: BTreeMap<String, String>,
        at: String,
        context: String,
        /// Why the committed amount is what it is — verbatim receipt facts,
        /// including where it differs from the quote it settles.
        note: String,
    },
    Released {
        id: Uuid,
        /// The reservation this ends.
        reserved_id: Uuid,
        at: String,
        reason: String,
    },
}

/// A held reservation, to be settled by [`Ledger::reconcile`] or ended by
/// [`Ledger::release`].
#[derive(Debug, Clone)]
pub struct Reservation {
    pub id: Uuid,
    pub amount: Money,
}

/// A settlement the ledger did not record after one retry under its lock.
/// The receipt is known and, where it carried money, the money has moved;
/// the ledger's committed total omits it, and a reservation left unresolved
/// is released by the expiry sweep as if its buyer had died. The caller
/// records this as a gap, so the evidence states what the ledger does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementFailure {
    pub reservation_id: Option<Uuid>,
    pub observed: Option<Money>,
    pub error: String,
}

impl SettlementFailure {
    /// The gate record's `settlement` for a settlement that was not
    /// recorded: what the ledger was asked to count, and why it did not.
    pub fn to_value(&self) -> Value {
        json!({
            "reconciled": false,
            "reservation_id": self.reservation_id,
            "observed": self.observed,
            "error": self.error,
        })
    }
}

impl std::fmt::Display for SettlementFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.observed, self.reservation_id) {
            (Some(observed), Some(id)) => write!(
                f,
                "Money moved and the allowance ledger did not count it: the receipt reported \
                 {} {} against reservation {id}, and the ledger did not record the commit ({}). \
                 The reservation stays unresolved until the expiry sweep releases it, and the \
                 period's committed total omits this charge.",
                observed.as_decimal_string(),
                observed.currency(),
                self.error
            ),
            (Some(observed), None) => write!(
                f,
                "Money moved and the allowance ledger did not count it: the receipt reported \
                 {} {} with nothing reserved, and the ledger did not record the commit ({}). \
                 The period's committed total omits this charge.",
                observed.as_decimal_string(),
                observed.currency(),
                self.error
            ),
            (None, Some(id)) => write!(
                f,
                "No charge in currency was reported against reservation {id}, and the ledger \
                 did not record its release ({}). The reservation stays held until the expiry \
                 sweep releases it.",
                self.error
            ),
            (None, None) => write!(
                f,
                "The allowance ledger did not record this settlement ({}).",
                self.error
            ),
        }
    }
}

/// One declaration's arithmetic at the moment of a check, in micros of the
/// declaration's own currency. `comparable` is false when the charge is in a
/// different currency or carries no money at all — the amounts then say what
/// was already spent, and nothing about this charge.
#[derive(Debug, Clone, Serialize)]
pub struct PeriodCheck {
    pub period: AllowancePeriod,
    pub period_key: String,
    pub declared: Money,
    pub spent_before: Money,
    pub comparable: bool,
    /// True when spending the charge would leave the period over its
    /// declaration. Meaningful only where `comparable`.
    pub would_exceed: bool,
}

/// What a reservation attempt found and did.
#[derive(Debug)]
pub enum ReserveOutcome {
    /// The principal declares no allowance; nothing was checked or written.
    NothingDeclared,
    /// Every declared period fits and the reservation is held.
    Reserved {
        reservation: Reservation,
        checks: Vec<PeriodCheck>,
    },
    /// The caller allowed the breach (observe and prefer modes), so the
    /// reservation is held despite a period that does not fit or a charge
    /// that cannot be compared; the checks say which.
    ReservedWithBreach {
        reservation: Reservation,
        checks: Vec<PeriodCheck>,
    },
    /// Nothing fits — or nothing is comparable — and the caller did not
    /// allow the breach. Nothing was written.
    Declined { checks: Vec<PeriodCheck> },
}

/// One declaration's standing, for the console and CLI.
#[derive(Debug, Clone, Serialize)]
pub struct PeriodState {
    pub period: AllowancePeriod,
    pub period_key: String,
    pub declared: Money,
    pub committed: Money,
    pub reserved: Money,
    /// Declared minus committed minus reserved, floored at zero; `exceeded`
    /// says when the floor is hiding an overdraft.
    pub remaining: Money,
    pub exceeded: bool,
}

impl Ledger {
    pub fn in_home(home: &Path) -> Self {
        Self {
            directory: home.join("allowance"),
        }
    }

    pub fn file(&self) -> PathBuf {
        self.directory.join("ledger.ndjson")
    }

    /// Hold authority for `amount` ahead of a purchase.
    ///
    /// One call checks every period the principal declares — a purchase must
    /// fit the day and the month, not one of them — and holds a single
    /// reservation counted against all of them. `allow_breach` is the
    /// observe/prefer stance: the reservation is written with the breach in
    /// the outcome rather than refused, because those modes record spending
    /// honestly instead of preventing it.
    pub fn reserve(
        &self,
        account: &Account<'_>,
        amount: &Money,
        now: DateTime<Utc>,
        context: &str,
        allow_breach: bool,
    ) -> Result<ReserveOutcome, String> {
        if account.declarations.is_empty() {
            return Ok(ReserveOutcome::NothingDeclared);
        }
        let _lock = self.lock()?;
        let entries = self.sweep_expired(now)?;
        let checks = checks_for(&entries, account, Some(amount), now)?;
        let fits = checks
            .iter()
            .all(|check| check.comparable && !check.would_exceed);
        if !fits && !allow_breach {
            return Ok(ReserveOutcome::Declined { checks });
        }
        let reservation = Reservation {
            id: Uuid::new_v4(),
            amount: amount.clone(),
        };
        self.append(&LedgerEntry::Reserved {
            id: reservation.id,
            principal: account.principal.to_owned(),
            amount: amount.clone(),
            period_keys: period_keys(account.declarations, now)?,
            at: stamp(now),
            expires_at: stamp(now + RESERVATION_TTL),
            pid: std::process::id(),
            context: context.to_owned(),
        })?;
        Ok(if fits {
            ReserveOutcome::Reserved {
                reservation,
                checks,
            }
        } else {
            ReserveOutcome::ReservedWithBreach {
                reservation,
                checks,
            }
        })
    }

    /// Settle a reservation against what the receipt actually said.
    ///
    /// The committed amount is the observed charge where the receipt reported
    /// money, whatever the quote said — the ledger records spend, not
    /// intentions — and the note states both figures where they differ. A
    /// receipt reporting no money at all releases the reservation instead:
    /// the spend, if any, was in a native unit a monetary allowance must not
    /// absorb, and the note says what was disclosed.
    pub fn reconcile(
        &self,
        account: &Account<'_>,
        reservation: &Reservation,
        receipt_money: Option<&Money>,
        now: DateTime<Utc>,
        context: &str,
        receipt_note: &str,
    ) -> Result<String, String> {
        let _lock = self.lock()?;
        self.sweep_expired(now)?;
        match receipt_money {
            Some(observed) => {
                let note = if observed == &reservation.amount {
                    format!("receipt matches the quote: {receipt_note}")
                } else {
                    format!(
                        "receipt differs from the quote ({} {} quoted, {} {} charged): \
                         {receipt_note}",
                        reservation.amount.as_decimal_string(),
                        reservation.amount.currency(),
                        observed.as_decimal_string(),
                        observed.currency(),
                    )
                };
                self.append_settlement(&LedgerEntry::Committed {
                    id: Uuid::new_v4(),
                    reserved_id: Some(reservation.id),
                    principal: account.principal.to_owned(),
                    amount: observed.clone(),
                    period_keys: period_keys(account.declarations, now)?,
                    at: stamp(now),
                    context: context.to_owned(),
                    note: note.clone(),
                })?;
                Ok(note)
            }
            None => {
                let reason = format!(
                    "released on the receipt: no charge in currency was reported, so nothing \
                     is counted against a monetary allowance ({receipt_note})"
                );
                self.append_settlement(&LedgerEntry::Released {
                    id: Uuid::new_v4(),
                    reserved_id: reservation.id,
                    at: stamp(now),
                    reason: reason.clone(),
                })?;
                Ok(reason)
            }
        }
    }

    /// End a reservation without spend: a declined purchase, a failed
    /// confirm. The reason is the record.
    pub fn release(
        &self,
        reservation: &Reservation,
        now: DateTime<Utc>,
        reason: &str,
    ) -> Result<(), String> {
        let _lock = self.lock()?;
        self.append(&LedgerEntry::Released {
            id: Uuid::new_v4(),
            reserved_id: reservation.id,
            at: stamp(now),
            reason: reason.to_owned(),
        })
    }

    /// Spend that was only knowable after the fact: an observed charge with
    /// no reservation held. Recorded whatever the remaining allowance says —
    /// the money has moved — with the caller's note saying where it came
    /// from.
    pub fn commit_observed(
        &self,
        account: &Account<'_>,
        amount: &Money,
        now: DateTime<Utc>,
        context: &str,
        note: &str,
    ) -> Result<(), String> {
        if account.declarations.is_empty() {
            return Ok(());
        }
        let _lock = self.lock()?;
        self.sweep_expired(now)?;
        self.append_settlement(&LedgerEntry::Committed {
            id: Uuid::new_v4(),
            reserved_id: None,
            principal: account.principal.to_owned(),
            amount: amount.clone(),
            period_keys: period_keys(account.declarations, now)?,
            at: stamp(now),
            context: context.to_owned(),
            note: note.to_owned(),
        })
    }

    /// Each declaration's standing now, for the console, the CLI and the
    /// pre-dispatch exhaustion check.
    pub fn state(
        &self,
        account: &Account<'_>,
        now: DateTime<Utc>,
    ) -> Result<Vec<PeriodState>, String> {
        let _lock = self.lock()?;
        let entries = self.sweep_expired(now)?;
        account
            .declarations
            .iter()
            .map(|declared| {
                let key = declared.period_key(now)?;
                let (committed, reserved) = spent(&entries, account.principal, declared, &key, now);
                let held = committed.saturating_add(reserved);
                Ok(PeriodState {
                    period: declared.period,
                    period_key: key,
                    declared: declared.amount.clone(),
                    committed: Money::new(declared.amount.currency(), committed),
                    reserved: Money::new(declared.amount.currency(), reserved),
                    remaining: Money::new(
                        declared.amount.currency(),
                        declared.amount.micros.saturating_sub(held),
                    ),
                    exceeded: held > declared.amount.micros,
                })
            })
            .collect()
    }

    fn lock(&self) -> Result<declaration::Lock, String> {
        std::fs::create_dir_all(&self.directory)
            .map_err(|error| format!("create {}: {error}", self.directory.display()))?;
        declaration::lock(&self.directory.join("ledger.lock"))
            .map_err(|refused| refused.to_string())
    }

    /// Read every entry. A line this runtime cannot parse is an error naming
    /// the file, never a skipped line: a ledger that silently loses entries
    /// reopens spent authority.
    fn read(&self) -> Result<Vec<LedgerEntry>, String> {
        let path = self.file();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        text.lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(index, line)| {
                serde_json::from_str(line).map_err(|error| {
                    format!(
                        "{} line {} is not a ledger entry: {error}",
                        path.display(),
                        index + 1
                    )
                })
            })
            .collect()
    }

    /// Release every expired, unresolved reservation, then return the entries
    /// including the releases just written. Runs first under every lock, so a
    /// crashed buyer's hold ends at the next writer whoever that is.
    fn sweep_expired(&self, now: DateTime<Utc>) -> Result<Vec<LedgerEntry>, String> {
        let mut entries = self.read()?;
        let resolved = resolved_ids(&entries);
        let expired: Vec<(Uuid, String)> = entries
            .iter()
            .filter_map(|entry| match entry {
                LedgerEntry::Reserved { id, expires_at, .. }
                    if !resolved.contains(id) && stamp(now).as_str() >= expires_at.as_str() =>
                {
                    Some((*id, expires_at.clone()))
                }
                _ => None,
            })
            .collect();
        for (reserved_id, expires_at) in expired {
            let release = LedgerEntry::Released {
                id: Uuid::new_v4(),
                reserved_id,
                at: stamp(now),
                reason: format!(
                    "released by rule: the reservation expired at {expires_at} without a \
                     receipt or a release, so its buyer is taken to have died before settling"
                ),
            };
            self.append(&release)?;
            entries.push(release);
        }
        Ok(entries)
    }

    /// Append one line, flushed to disk before the lock is given up.
    fn append(&self, entry: &LedgerEntry) -> Result<(), String> {
        let path = self.file();
        let line = serde_json::to_string(entry)
            .map_err(|error| format!("encode ledger entry: {error}"))?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        file.write_all(format!("{line}\n").as_bytes())
            .map_err(|error| format!("append to {}: {error}", path.display()))?;
        file.sync_all()
            .map_err(|error| format!("flush {}: {error}", path.display()))
    }

    /// Append a settlement line, retrying once under the lock the caller
    /// holds. A settlement records money that has already moved, so one
    /// failed write is tried again before the caller is told the ledger's
    /// account is now short of the receipt.
    fn append_settlement(&self, entry: &LedgerEntry) -> Result<(), String> {
        let Err(first) = self.append(entry) else {
            return Ok(());
        };
        self.append(entry)
            .map_err(|second| format!("{first}; retried once under the lock: {second}"))
    }
}

/// What the allowance gate decided at one crossing: the evidence record of
/// the consultation, the reservation now held (settle it against the
/// receipt), and a ruling when the gate found anything to say — a refusal in
/// strict, a recorded breach in observe and prefer, nothing when the spend
/// simply fits.
#[derive(Debug)]
pub struct GateDecision {
    pub record: Value,
    pub reservation: Option<Reservation>,
    pub ruling: Option<Ruling>,
}

impl GateDecision {
    fn consulted(context: &AllowanceContext) -> Value {
        json!({
            "consulted": true,
            "principal": context.principal,
            "declared": !context.declarations.is_empty(),
        })
    }
}

impl AllowanceContext {
    /// Reserve `price` against every declared period and build the evidence
    /// record — the shared arithmetic of the quote gate and the dispatch
    /// gate. Strict declines what does not fit or cannot be compared;
    /// observe and prefer hold the reservation with the breach recorded.
    pub fn reserve_decision(
        &self,
        price: &Money,
        mode: PolicyMode,
        now: DateTime<Utc>,
        context: &str,
    ) -> GateDecision {
        let mut record = GateDecision::consulted(self);
        let allow_breach = mode != PolicyMode::Strict;
        match self
            .ledger
            .reserve(&self.account(), price, now, context, allow_breach)
        {
            Ok(ReserveOutcome::NothingDeclared) => {
                record["reason"] = json!(format!(
                    "principal {:?} declares no allowance",
                    self.principal
                ));
                GateDecision {
                    record,
                    reservation: None,
                    ruling: None,
                }
            }
            Ok(ReserveOutcome::Reserved {
                reservation,
                checks,
            }) => {
                record["decision"] = json!("reserved");
                record["reservation_id"] = json!(reservation.id);
                record["checks"] = json!(checks);
                GateDecision {
                    record,
                    reservation: Some(reservation),
                    ruling: None,
                }
            }
            Ok(ReserveOutcome::ReservedWithBreach {
                reservation,
                checks,
            }) => {
                let ruling = Ruling::breach(
                    mode,
                    refusal_reason(&self.principal, price, &checks),
                    breach_gap(&checks),
                );
                record["decision"] = json!("proceeded_with_breach");
                record["reason"] = json!(ruling.reason().unwrap_or_default());
                record["reservation_id"] = json!(reservation.id);
                record["checks"] = json!(checks);
                GateDecision {
                    record,
                    reservation: Some(reservation),
                    ruling: Some(ruling),
                }
            }
            Ok(ReserveOutcome::Declined { checks }) => {
                let ruling = Ruling::breach(
                    mode,
                    refusal_reason(&self.principal, price, &checks),
                    breach_gap(&checks),
                );
                record["decision"] = json!("declined");
                record["reason"] = json!(ruling.reason().unwrap_or_default());
                record["checks"] = json!(checks);
                GateDecision {
                    record,
                    reservation: None,
                    ruling: Some(ruling),
                }
            }
            Err(error) => {
                let ruling = unreadable_ruling(&self.principal, mode, &error);
                record["decision"] = json!(if ruling.is_refusal() {
                    "declined"
                } else {
                    "proceeded_with_breach"
                });
                record["reason"] = json!(ruling.reason().unwrap_or_default());
                GateDecision {
                    record,
                    reservation: None,
                    ruling: Some(ruling),
                }
            }
        }
    }

    /// The gate for an acquisition that is not quote-then-buy: a remote
    /// search or fetch, whose price precedes the call only where the adapter
    /// declares a published one.
    ///
    /// With a declared price the reservation runs exactly as at a quote.
    /// Without one, only what is already unaccountable refuses: a period
    /// that is exhausted has nothing left for any positive charge, declared
    /// or not, so strict refuses the dispatch and observe/prefer record the
    /// breach; a period with room proceeds, and the observed charge — where
    /// the provider reports one — is committed on the receipt.
    pub fn pre_dispatch(
        &self,
        declared_price: Option<&Money>,
        mode: PolicyMode,
        now: DateTime<Utc>,
        context: &str,
    ) -> GateDecision {
        let mut record = GateDecision::consulted(self);
        if self.declarations.is_empty() {
            record["reason"] = json!(format!(
                "principal {:?} declares no allowance",
                self.principal
            ));
            return GateDecision {
                record,
                reservation: None,
                ruling: None,
            };
        }
        if let Some(price) = declared_price {
            let mut decision = self.reserve_decision(price, mode, now, context);
            decision.record["price_basis"] = json!(
                "published price declared by the adapter, reserved before dispatch and \
                 reconciled against the observed receipt"
            );
            return decision;
        }
        match self.ledger.state(&self.account(), now) {
            Ok(periods) => {
                let exhausted: Vec<&PeriodState> = periods
                    .iter()
                    .filter(|period| period.exceeded || period.remaining.micros == 0)
                    .collect();
                if exhausted.is_empty() {
                    record["decision"] = json!("unpriced_within_allowance");
                    record["reason"] = json!(
                        "the provider declares no price to reserve; the allowance has room, \
                         and the observed charge, if any, is committed on the receipt"
                    );
                    return GateDecision {
                        record,
                        reservation: None,
                        ruling: None,
                    };
                }
                let parts: Vec<String> = exhausted
                    .iter()
                    .map(|period| {
                        format!(
                            "the {} allowance for {} is exhausted ({} {} of {} {} held)",
                            period.period.as_str(),
                            period.period_key,
                            period
                                .committed
                                .try_add(&period.reserved)
                                .unwrap_or_else(|| period.committed.clone())
                                .as_decimal_string(),
                            period.declared.currency(),
                            period.declared.as_decimal_string(),
                            period.declared.currency(),
                        )
                    })
                    .collect();
                let ruling = Ruling::breach(
                    mode,
                    format!(
                        "Principal {:?} is over their cumulative allowance: {}; this provider \
                         declares no price, so nothing further can be accounted before \
                         dispatch.",
                        self.principal,
                        parts.join("; ")
                    ),
                    Gap::new(
                        GapReason::BudgetExhausted,
                        "The principal's periodic allowance is exhausted, so the acquisition \
                         was declined before dispatch and nothing was fetched.",
                    ),
                );
                record["decision"] = json!(if ruling.is_refusal() {
                    "declined"
                } else {
                    "proceeded_with_breach"
                });
                record["reason"] = json!(ruling.reason().unwrap_or_default());
                GateDecision {
                    record,
                    reservation: None,
                    ruling: Some(ruling),
                }
            }
            Err(error) => {
                let ruling = unreadable_ruling(&self.principal, mode, &error);
                record["decision"] = json!(if ruling.is_refusal() {
                    "declined"
                } else {
                    "proceeded_with_breach"
                });
                record["reason"] = json!(ruling.reason().unwrap_or_default());
                GateDecision {
                    record,
                    reservation: None,
                    ruling: Some(ruling),
                }
            }
        }
    }

    /// Settle the gate's outcome against a completed acquisition: reconcile
    /// the held reservation with the receipt, or commit an observed charge
    /// nothing reserved for. Writes the settlement into `record`. A ledger
    /// that does not take the write after one retry leaves `settlement`
    /// stating so, and the failure is returned for the caller to record as
    /// a gap — the money has moved either way, and the record must say the
    /// ledger did not count it.
    pub fn settle_success(
        &self,
        record: &mut Value,
        reservation: Option<&Reservation>,
        charge: &AcquisitionCharge,
        now: DateTime<Utc>,
        context: &str,
    ) -> Result<(), SettlementFailure> {
        let note = receipt_note(charge);
        let settlement = match reservation {
            Some(reservation) => self
                .ledger
                .reconcile(
                    &self.account(),
                    reservation,
                    charge.money.as_ref(),
                    now,
                    context,
                    &note,
                )
                .map(|outcome| json!({ "reconciled": true, "note": outcome })),
            None => match charge.money.as_ref() {
                Some(observed) if !self.declarations.is_empty() => self
                    .ledger
                    .commit_observed(&self.account(), observed, now, context, &note)
                    .map(|()| {
                        json!({
                            "committed": true,
                            "note": format!("committed on the receipt: {note}"),
                        })
                    }),
                _ => return Ok(()),
            },
        };
        match settlement {
            Ok(settlement) => {
                record["settlement"] = settlement;
                Ok(())
            }
            Err(error) => {
                let failure = SettlementFailure {
                    reservation_id: reservation.map(|reservation| reservation.id),
                    observed: charge.money.clone(),
                    error,
                };
                record["settlement"] = failure.to_value();
                Err(failure)
            }
        }
    }

    /// End a held reservation after a failed dispatch, recording why. A
    /// ledger that does not take the release leaves `settlement` stating
    /// so with the reservation's id — the reservation stays held until the
    /// expiry sweep releases it — and the error is returned for the caller
    /// to record as a gap.
    pub fn release_after_failure(
        &self,
        record: &mut Value,
        reservation: &Reservation,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let reason = format!("released: {reason}");
        if let Err(error) = self.ledger.release(reservation, now, &reason) {
            record["settlement"] = json!({
                "reconciled": false,
                "reservation_id": reservation.id,
                "error": error,
            });
            return Err(error);
        }
        record["settlement"] = json!({ "reconciled": false, "note": reason });
        Ok(())
    }
}

/// Why an allowance declined or breached, from the periods that failed:
/// exhaustion and incomparability each in plain words, so the record reads
/// without the checks table.
pub fn refusal_reason(principal: &str, price: &Money, checks: &[PeriodCheck]) -> String {
    let mut parts = Vec::new();
    for check in checks {
        if !check.comparable {
            parts.push(format!(
                "the {} allowance is declared in {} and the price is in {}, which this runtime \
                 holds no rate source to compare",
                check.period.as_str(),
                check.declared.currency(),
                price.currency()
            ));
        } else if check.would_exceed {
            let remaining = check
                .declared
                .micros
                .saturating_sub(check.spent_before.micros);
            parts.push(format!(
                "the {} allowance for {} has {} {} remaining of {} {} and the price is {} {}",
                check.period.as_str(),
                check.period_key,
                Money::new(check.declared.currency(), remaining).as_decimal_string(),
                check.declared.currency(),
                check.declared.as_decimal_string(),
                check.declared.currency(),
                price.as_decimal_string(),
                price.currency()
            ));
        }
    }
    format!(
        "Principal {principal:?} is over their cumulative allowance: {}.",
        parts.join("; ")
    )
}

/// Exhaustion is a spent budget; incomparability is missing evidence. The gap
/// vocabulary keeps the two apart, as the job cap's purchase decision does.
pub fn breach_gap(checks: &[PeriodCheck]) -> Gap {
    if checks.iter().any(|check| check.would_exceed) {
        Gap::new(
            GapReason::BudgetExhausted,
            "The price exceeded the principal's remaining periodic allowance, so the \
             acquisition was declined and nothing was bought.",
        )
    } else {
        Gap::new(
            GapReason::EvidenceMissing,
            "The price and the principal's allowance are in different currencies, so the \
             spend could not be checked before money moved.",
        )
    }
}

/// The receipt's charge in words, for the ledger's reconciliation note.
pub fn receipt_note(charge: &AcquisitionCharge) -> String {
    let mut parts = Vec::new();
    if let Some(money) = &charge.money {
        parts.push(format!(
            "{} {} observed on the receipt",
            money.as_decimal_string(),
            money.currency()
        ));
    }
    if let Some(native) = &charge.native {
        parts.push(format!(
            "{} {} in the supplier's own unit",
            native.amount, native.unit
        ));
    }
    if parts.is_empty() {
        "the receipt disclosed no charge".to_owned()
    } else {
        parts.join("; ")
    }
}

/// An unreadable ledger is an unknown remaining amount, and unknown never
/// means "assume it fits" (`docs/FAIL-POLICY.md` §7).
fn unreadable_ruling(principal: &str, mode: PolicyMode, error: &str) -> Ruling {
    Ruling::breach(
        mode,
        format!(
            "Principal {principal:?} holds a cumulative allowance whose ledger could not be \
             consulted: {error}"
        ),
        Gap::new(
            GapReason::EvidenceMissing,
            "The allowance ledger was unreadable, so the remaining allowance is unknown and \
             the spend could not be checked against it.",
        ),
    )
}

/// RFC 3339 in UTC, second precision: sortable as text, which the expiry
/// comparison relies on.
fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn period_keys(
    declarations: &[AllowanceDeclaration],
    now: DateTime<Utc>,
) -> Result<BTreeMap<String, String>, String> {
    declarations
        .iter()
        .map(|declared| {
            Ok((
                declared.period.as_str().to_owned(),
                declared.period_key(now)?,
            ))
        })
        .collect()
}

/// Ids of reservations something later settled or ended.
fn resolved_ids(entries: &[LedgerEntry]) -> Vec<Uuid> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            LedgerEntry::Committed {
                reserved_id: Some(id),
                ..
            } => Some(*id),
            LedgerEntry::Released { reserved_id, .. } => Some(*reserved_id),
            _ => None,
        })
        .collect()
}

/// What one declaration's period already holds: committed spend and live
/// reservations, in micros of the declaration's currency. Amounts in any
/// other currency never enter the sum — they belong to a declaration in
/// their own currency or to none.
fn spent(
    entries: &[LedgerEntry],
    principal: &str,
    declared: &AllowanceDeclaration,
    key: &str,
    now: DateTime<Utc>,
) -> (u64, u64) {
    let resolved = resolved_ids(entries);
    let period = declared.period.as_str();
    let mut committed: u64 = 0;
    let mut reserved: u64 = 0;
    for entry in entries {
        match entry {
            LedgerEntry::Committed {
                principal: entry_principal,
                amount,
                period_keys,
                ..
            } if entry_principal == principal
                && amount.currency() == declared.amount.currency()
                && period_keys.get(period).is_some_and(|k| k == key) =>
            {
                committed = committed.saturating_add(amount.micros);
            }
            LedgerEntry::Reserved {
                id,
                principal: entry_principal,
                amount,
                period_keys,
                expires_at,
                ..
            } if entry_principal == principal
                && amount.currency() == declared.amount.currency()
                && period_keys.get(period).is_some_and(|k| k == key)
                && !resolved.contains(id)
                && stamp(now).as_str() < expires_at.as_str() =>
            {
                reserved = reserved.saturating_add(amount.micros);
            }
            _ => {}
        }
    }
    (committed, reserved)
}

fn checks_for(
    entries: &[LedgerEntry],
    account: &Account<'_>,
    charge: Option<&Money>,
    now: DateTime<Utc>,
) -> Result<Vec<PeriodCheck>, String> {
    account
        .declarations
        .iter()
        .map(|declared| {
            let key = declared.period_key(now)?;
            let (committed, reserved) = spent(entries, account.principal, declared, &key, now);
            let held = committed.saturating_add(reserved);
            let comparable =
                charge.is_some_and(|money| money.currency() == declared.amount.currency());
            let would_exceed = match charge {
                Some(money) if comparable => {
                    held.saturating_add(money.micros) > declared.amount.micros
                }
                _ => false,
            };
            Ok(PeriodCheck {
                period: declared.period,
                period_key: key,
                declared: declared.amount.clone(),
                spent_before: Money::new(declared.amount.currency(), held),
                comparable,
                would_exceed,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    fn usd(micros: u64) -> Money {
        Money::new("USD", micros)
    }

    fn day_usd(micros: u64) -> AllowanceDeclaration {
        AllowanceDeclaration {
            period: AllowancePeriod::Day,
            amount: usd(micros),
            timezone: "UTC".to_owned(),
        }
    }

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 22, hour, 0, 0).unwrap()
    }

    fn account<'a>(declarations: &'a [AllowanceDeclaration]) -> Account<'a> {
        Account {
            principal: "os-user:1000",
            declarations,
        }
    }

    fn ledger() -> (tempfile::TempDir, Ledger) {
        let home = tempfile::tempdir().unwrap();
        let ledger = Ledger::in_home(home.path());
        (home, ledger)
    }

    /// The acceptance line: individually affordable purchases whose total
    /// would exceed the allowance stop before the first excess one.
    #[test]
    fn the_purchase_that_would_exceed_the_period_is_declined_before_money_moves() {
        let (_home, ledger) = ledger();
        let declared = [day_usd(20_000)];
        for n in 0..2 {
            let outcome = ledger
                .reserve(&account(&declared), &usd(7_000), at(n), "test", false)
                .unwrap();
            let ReserveOutcome::Reserved { reservation, .. } = outcome else {
                panic!("an affordable purchase reserves");
            };
            ledger
                .reconcile(
                    &account(&declared),
                    &reservation,
                    Some(&usd(7_000)),
                    at(n),
                    "test",
                    "observed",
                )
                .unwrap();
        }
        let outcome = ledger
            .reserve(&account(&declared), &usd(7_000), at(3), "test", false)
            .unwrap();
        let ReserveOutcome::Declined { checks } = outcome else {
            panic!("the third purchase would exceed 0.020000 USD and must decline");
        };
        assert!(checks[0].would_exceed);
        assert_eq!(checks[0].spent_before, usd(14_000));
    }

    /// Restart cannot reopen spent authority: the sums come from the file.
    #[test]
    fn a_fresh_ledger_handle_sees_the_spend_the_old_one_committed() {
        let (home, ledger) = ledger();
        let declared = [day_usd(10_000)];
        let ReserveOutcome::Reserved { reservation, .. } = ledger
            .reserve(&account(&declared), &usd(9_000), at(0), "test", false)
            .unwrap()
        else {
            panic!("reserves");
        };
        ledger
            .reconcile(
                &account(&declared),
                &reservation,
                Some(&usd(9_000)),
                at(0),
                "test",
                "observed",
            )
            .unwrap();
        drop(ledger);
        let reopened = Ledger::in_home(home.path());
        let outcome = reopened
            .reserve(&account(&declared), &usd(2_000), at(1), "test", false)
            .unwrap();
        assert!(matches!(outcome, ReserveOutcome::Declined { .. }));
    }

    /// Concurrent buyers on one home cannot collectively over-reserve: the
    /// lock makes read-check-append one step.
    #[test]
    fn concurrent_reservations_never_collectively_exceed_the_declaration() {
        let (_home, ledger) = ledger();
        let declared = [day_usd(20_000)];
        let reserved: Vec<bool> = std::thread::scope(|scope| {
            (0..8)
                .map(|_| {
                    let ledger = ledger.clone();
                    let declared = declared.clone();
                    scope.spawn(move || {
                        matches!(
                            ledger
                                .reserve(&account(&declared), &usd(7_000), at(0), "t", false)
                                .unwrap(),
                            ReserveOutcome::Reserved { .. }
                        )
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });
        assert_eq!(
            reserved.iter().filter(|held| **held).count(),
            2,
            "0.020000 USD holds exactly two 0.007000 reservations"
        );
    }

    /// The day boundary rolls the budget over; the month does not reset with
    /// it. Fixed clock throughout.
    #[test]
    fn rollover_renews_the_day_but_not_the_month() {
        let (_home, ledger) = ledger();
        let declared = [
            day_usd(7_000),
            AllowanceDeclaration {
                period: AllowancePeriod::Month,
                amount: usd(10_000),
                timezone: "UTC".to_owned(),
            },
        ];
        let ReserveOutcome::Reserved { reservation, .. } = ledger
            .reserve(&account(&declared), &usd(7_000), at(1), "test", false)
            .unwrap()
        else {
            panic!("day one fits");
        };
        ledger
            .reconcile(
                &account(&declared),
                &reservation,
                Some(&usd(7_000)),
                at(1),
                "test",
                "observed",
            )
            .unwrap();
        // Same day again: the day allowance is spent.
        assert!(matches!(
            ledger
                .reserve(&account(&declared), &usd(7_000), at(2), "test", false)
                .unwrap(),
            ReserveOutcome::Declined { .. }
        ));
        // Next day: the day renews, but the month has 0.003000 left, so the
        // same purchase still declines — on the month, not the day.
        let next_day = Utc.with_ymd_and_hms(2026, 8, 23, 1, 0, 0).unwrap();
        let ReserveOutcome::Declined { checks } = ledger
            .reserve(&account(&declared), &usd(7_000), next_day, "test", false)
            .unwrap()
        else {
            panic!("the month still binds");
        };
        assert!(!checks[0].would_exceed, "the day renewed");
        assert!(checks[1].would_exceed, "the month did not");
    }

    /// The crashed buyer: an unresolved reservation past its expiry is
    /// released by the next writer, with the rule named.
    #[test]
    fn an_expired_reservation_is_released_by_the_next_writer() {
        let (_home, ledger) = ledger();
        let declared = [day_usd(10_000)];
        let ReserveOutcome::Reserved { .. } = ledger
            .reserve(&account(&declared), &usd(9_000), at(1), "crashes", false)
            .unwrap()
        else {
            panic!("reserves");
        };
        // Within the TTL the authority is held.
        assert!(matches!(
            ledger
                .reserve(&account(&declared), &usd(9_000), at(1), "test", false)
                .unwrap(),
            ReserveOutcome::Declined { .. }
        ));
        // Past it, the next writer sweeps and the authority is back.
        let later = at(1) + chrono::Duration::minutes(11);
        assert!(matches!(
            ledger
                .reserve(&account(&declared), &usd(9_000), later, "test", false)
                .unwrap(),
            ReserveOutcome::Reserved { .. }
        ));
        let text = std::fs::read_to_string(ledger.file()).unwrap();
        assert!(
            text.contains("released by rule"),
            "the sweep left its line: {text}"
        );
    }

    /// A receipt that reports no money releases rather than commits: native
    /// units never enter a monetary sum.
    #[test]
    fn a_receipt_with_no_currency_releases_the_reservation() {
        let (_home, ledger) = ledger();
        let declared = [day_usd(10_000)];
        let ReserveOutcome::Reserved { reservation, .. } = ledger
            .reserve(&account(&declared), &usd(8_000), at(1), "test", false)
            .unwrap()
        else {
            panic!("reserves");
        };
        ledger
            .reconcile(
                &account(&declared),
                &reservation,
                None,
                at(1),
                "test",
                "1 trial_queries consumed",
            )
            .unwrap();
        let state = ledger.state(&account(&declared), at(2)).unwrap();
        assert_eq!(state[0].committed, usd(0));
        assert_eq!(state[0].reserved, usd(0));
        assert_eq!(state[0].remaining, usd(10_000));
    }

    /// An incomparable charge cannot be checked; strict callers decline it,
    /// observe callers hold it with the breach stated, and either way it
    /// never counts against the other currency's sums.
    #[test]
    fn a_charge_in_another_currency_is_incomparable_never_converted() {
        let (_home, ledger) = ledger();
        let declared = [day_usd(10_000)];
        let gbp = Money::new("GBP", 5_000);
        let outcome = ledger
            .reserve(&account(&declared), &gbp, at(1), "test", false)
            .unwrap();
        let ReserveOutcome::Declined { checks } = outcome else {
            panic!("strict declines what it cannot compare");
        };
        assert!(!checks[0].comparable);
        let outcome = ledger
            .reserve(&account(&declared), &gbp, at(1), "test", true)
            .unwrap();
        assert!(matches!(outcome, ReserveOutcome::ReservedWithBreach { .. }));
        let state = ledger.state(&account(&declared), at(1)).unwrap();
        assert_eq!(state[0].reserved, usd(0), "GBP never entered the USD sum");
    }

    /// A ledger line nothing can parse is an error naming the file and line,
    /// never a skipped entry.
    #[test]
    fn a_corrupt_line_is_an_error_never_a_smaller_ledger() {
        let (_home, ledger) = ledger();
        std::fs::create_dir_all(ledger.file().parent().unwrap()).unwrap();
        std::fs::write(ledger.file(), "{\"entry\":\"reserved\"\n").unwrap();
        let error = ledger
            .reserve(&account(&[day_usd(1)]), &usd(1), at(0), "t", false)
            .unwrap_err();
        assert!(error.contains("line 1"), "{error}");
    }

    /// The double fault: the reservation was written, the receipt arrived,
    /// and the ledger refused the commit twice. The failure names the
    /// reservation and the observed amount, the record states the
    /// settlement was not recorded, and the ledger's own account is short
    /// of the receipt — which is why the caller must say so elsewhere.
    #[test]
    fn a_settlement_the_ledger_cannot_write_is_retried_once_then_named() {
        use std::os::unix::fs::PermissionsExt as _;

        let home = tempfile::tempdir().unwrap();
        let context = AllowanceContext::new(
            home.path(),
            "os-user:1000".to_owned(),
            vec![day_usd(20_000)],
        );
        let mut decision =
            context.pre_dispatch(Some(&usd(7_000)), PolicyMode::Strict, at(10), "test");
        let reservation = decision.reservation.take().expect("the price is reserved");

        let file = context.ledger.file();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
        let charge = AcquisitionCharge {
            money: Some(usd(7_000)),
            native: None,
        };
        let failure = context
            .settle_success(
                &mut decision.record,
                Some(&reservation),
                &charge,
                at(10),
                "test",
            )
            .expect_err("a read-only ledger takes no settlement");

        assert_eq!(failure.reservation_id, Some(reservation.id));
        assert_eq!(failure.observed, Some(usd(7_000)));
        assert!(
            failure.error.contains("retried once under the lock"),
            "the append is tried twice before the failure is reported: {}",
            failure.error
        );
        let detail = failure.to_string();
        assert!(detail.contains("Money moved"), "{detail}");
        assert!(detail.contains(&reservation.id.to_string()), "{detail}");
        assert!(detail.contains("0.007000 USD"), "{detail}");
        assert_eq!(decision.record["settlement"]["reconciled"], false);
        assert_eq!(
            decision.record["settlement"]["reservation_id"],
            json!(reservation.id)
        );
        assert_eq!(decision.record["settlement"]["observed"]["micros"], 7_000);

        // The ledger's own account after the fault: the reservation still
        // held, nothing committed.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let state = context.ledger.state(&context.account(), at(10)).unwrap();
        assert_eq!(state[0].committed, usd(0));
        assert_eq!(state[0].reserved, usd(7_000));
    }
}
