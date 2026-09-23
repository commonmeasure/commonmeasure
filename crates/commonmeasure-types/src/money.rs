use std::cmp::Ordering;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Exact non-negative money in a named currency's millionth units.
///
/// The currency is explicit because provider comparisons will not remain
/// USD-only, and because two amounts in different currencies are not
/// comparable without a rate source this runtime does not have. Every
/// operation that would need one returns `None` rather than guessing;
/// conversion belongs in a separately evidenced evaluator.
///
/// Every way in — `new`, `from_decimal_str`, deserialisation — upper-cases
/// the code, and the field is private so nothing outside this module can
/// seat a lower-case one past them or overwrite one afterwards. A `"usd"`
/// that got through would compare incomparable to an adapter's `"USD"`,
/// which is a declared cap silently recorded as unenforceable:
///
/// ```compile_fail
/// let cap = commonmeasure_types::Money { currency: "usd".to_owned(), micros: 10 };
/// ```
///
/// ```compile_fail
/// let mut cap = commonmeasure_types::Money::new("USD", 10);
/// cap.currency = "usd".to_owned();
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Money {
    /// Always upper case. Read through [`Money::currency`].
    #[serde(deserialize_with = "uppercase_currency")]
    currency: String,
    pub micros: u64,
}

fn uppercase_currency<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(|currency| currency.to_uppercase())
}

impl Money {
    /// The currency code, always upper case.
    #[must_use]
    pub fn currency(&self) -> &str {
        &self.currency
    }

    pub fn new(currency: impl Into<String>, micros: u64) -> Self {
        Self {
            currency: currency.into().to_uppercase(),
            micros,
        }
    }

    /// Parse a decimal amount such as `"0.007"` exactly.
    ///
    /// Providers that report money report it as a JSON number; reading its
    /// decimal text rather than an `f64` keeps a charge that is already exact
    /// from acquiring a rounding error on the way into the evidence record.
    /// Returns `None` for anything that is not a plain non-negative decimal
    /// with at most six fractional digits, because a charge this runtime
    /// cannot represent exactly must stay unknown rather than be approximated.
    pub fn from_decimal_str(currency: impl Into<String>, decimal: &str) -> Option<Self> {
        let (whole, fraction) = match decimal.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (decimal, ""),
        };
        if whole.is_empty()
            || !whole.chars().all(|c| c.is_ascii_digit())
            || fraction.len() > 6
            || !fraction.chars().all(|c| c.is_ascii_digit())
        {
            return None;
        }
        let fraction_micros: u64 = if fraction.is_empty() {
            0
        } else {
            format!("{fraction:0<6}").parse().ok()?
        };
        whole
            .parse::<u64>()
            .ok()?
            .checked_mul(1_000_000)?
            .checked_add(fraction_micros)
            .map(|micros| Self::new(currency, micros))
    }

    /// The amount as a decimal string in its own currency's major unit.
    pub fn as_decimal_string(&self) -> String {
        format!("{}.{:06}", self.micros / 1_000_000, self.micros % 1_000_000)
    }

    /// Sum two amounts. `None` when the currencies differ or the total would
    /// overflow, both of which are unrepresentable rather than zero.
    pub fn try_add(&self, other: &Money) -> Option<Money> {
        if self.currency != other.currency {
            return None;
        }
        Some(Money {
            currency: self.currency.clone(),
            micros: self.micros.checked_add(other.micros)?,
        })
    }

    /// Order two amounts. `None` when the currencies differ, which is the
    /// answer a policy check must record as unenforceable rather than resolve.
    pub fn compare(&self, other: &Money) -> Option<Ordering> {
        (self.currency == other.currency).then(|| self.micros.cmp(&other.micros))
    }
}
