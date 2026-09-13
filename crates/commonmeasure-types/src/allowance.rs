use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::Money;

/// The calendar unit one allowance covers. Day and month are the two units
/// the allowance ledger starts with; a new unit is a new variant here and a
/// new key shape in [`AllowanceDeclaration::period_key`], nowhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowancePeriod {
    Day,
    Month,
}

impl AllowancePeriod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Month => "month",
        }
    }
}

/// One monetary allowance for one principal and one calendar period.
///
/// This is allocation — desired state in the declared policy artefact — and
/// nothing here enforces it; reservation and reconciliation live in the
/// runtime's ledger (`DECISIONS.md` §Delegated authority and fleet management). The amount is [`Money`], so
/// the currency is explicit and a charge in another currency or a native
/// provider unit is incomparable rather than convertible.
///
/// The timezone is an IANA name and is mandatory, because a calendar day is a
/// wall-clock fact: "which day this purchase falls in" has no answer until a
/// zone says where midnight is, and defaulting one would move the boundary of
/// someone's allowance without them declaring it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllowanceDeclaration {
    pub period: AllowancePeriod,
    pub amount: Money,
    /// IANA zone name (`"Europe/London"`, `"UTC"`) whose calendar defines the
    /// period boundary.
    pub timezone: String,
}

impl AllowanceDeclaration {
    /// The declaration's own validity: the timezone must be a zone this
    /// build's IANA data recognises. A zero amount is legal — it declares
    /// that the principal may spend nothing, which is a statement, not a
    /// mistake. Returns the bare reason; the policy loader wraps it with the
    /// file and principal it belongs to.
    pub fn validate(&self) -> Result<(), String> {
        self.zone().map(|_| ())
    }

    /// The key of the period `now` falls in, in the declared zone:
    /// `"2026-08-22"` for a day allowance, `"2026-08"` for a month one. Two
    /// charges share a budget exactly when their keys are equal, so the key
    /// carries everything boundary-shaped — including DST, which is why this
    /// goes through the zone's own calendar rather than UTC arithmetic.
    pub fn period_key(&self, now: DateTime<Utc>) -> Result<String, String> {
        let local = now.with_timezone(&self.zone()?);
        Ok(match self.period {
            AllowancePeriod::Day => local.format("%Y-%m-%d"),
            AllowancePeriod::Month => local.format("%Y-%m"),
        }
        .to_string())
    }

    fn zone(&self) -> Result<Tz, String> {
        self.timezone.parse::<Tz>().map_err(|_| {
            format!(
                "names timezone {:?}, which is not an IANA zone this build recognises",
                self.timezone
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    fn declaration(period: AllowancePeriod, timezone: &str) -> AllowanceDeclaration {
        AllowanceDeclaration {
            period,
            amount: Money::new("USD", 20_000),
            timezone: timezone.to_owned(),
        }
    }

    #[test]
    fn an_unknown_timezone_is_refused_with_its_name() {
        let error = declaration(AllowancePeriod::Day, "Europe/Trantor")
            .validate()
            .unwrap_err();
        assert!(error.contains("Europe/Trantor"), "{error}");
    }

    #[test]
    fn a_zero_amount_is_a_legal_declaration() {
        let mut declared = declaration(AllowancePeriod::Day, "UTC");
        declared.amount = Money::new("USD", 0);
        assert_eq!(declared.validate(), Ok(()));
    }

    /// London's day is not UTC's day while BST holds: half past eleven at
    /// night UTC has already crossed the declared zone's midnight, so the
    /// charge belongs to the next day's allowance.
    #[test]
    fn the_day_boundary_follows_the_declared_zone_through_dst() {
        let declared = declaration(AllowancePeriod::Day, "Europe/London");
        let just_before_utc_midnight = Utc.with_ymd_and_hms(2026, 10, 24, 23, 30, 0).unwrap();
        assert_eq!(
            declared.period_key(just_before_utc_midnight).unwrap(),
            "2026-10-25"
        );
        // After the clocks go back (25 Oct 2026), London midnight and UTC
        // midnight coincide again.
        let after_transition = Utc.with_ymd_and_hms(2026, 10, 26, 23, 30, 0).unwrap();
        assert_eq!(declared.period_key(after_transition).unwrap(), "2026-10-26");
    }

    #[test]
    fn the_month_boundary_follows_the_declared_zone() {
        let declared = declaration(AllowancePeriod::Month, "Europe/London");
        let utc_still_august = Utc.with_ymd_and_hms(2026, 8, 31, 23, 30, 0).unwrap();
        assert_eq!(declared.period_key(utc_still_august).unwrap(), "2026-09");
        let declared_utc = declaration(AllowancePeriod::Month, "UTC");
        assert_eq!(
            declared_utc.period_key(utc_still_august).unwrap(),
            "2026-08"
        );
    }

    #[test]
    fn a_misspelled_field_is_a_load_error_not_a_lost_allowance() {
        let error = serde_json::from_str::<AllowanceDeclaration>(
            r#"{"period": "day", "ammount": {"currency": "USD", "micros": 1}, "timezone": "UTC"}"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("ammount"), "{error}");
    }
}
