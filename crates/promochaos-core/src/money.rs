//! Money in integer minor units.
//!
//! Shopify sends decimals as strings (`"120.00"`, `"10.0"`), and JavaScript
//! functions often send plain JSON numbers. Both are parsed with `rust_decimal`
//! and turned into `i64` minor units using the ISO 4217 exponent of the
//! currency. Floats are never used for money.

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use std::str::FromStr;

/// One row of the bundled ISO 4217 table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrencyInfo {
    pub code: &'static str,
    pub exponent: u32,
    /// Display prefix. Codes without a common symbol use the code and a space.
    pub symbol: &'static str,
}

/// ISO 4217 exponents for the currencies promochaos knows. Zero-decimal and
/// three-decimal currencies are the ones that catch rounding bugs.
const CURRENCIES: &[CurrencyInfo] = &[
    CurrencyInfo {
        code: "AED",
        exponent: 2,
        symbol: "AED ",
    },
    CurrencyInfo {
        code: "AUD",
        exponent: 2,
        symbol: "A$",
    },
    CurrencyInfo {
        code: "BHD",
        exponent: 3,
        symbol: "BHD ",
    },
    CurrencyInfo {
        code: "BRL",
        exponent: 2,
        symbol: "R$",
    },
    CurrencyInfo {
        code: "CAD",
        exponent: 2,
        symbol: "CA$",
    },
    CurrencyInfo {
        code: "CHF",
        exponent: 2,
        symbol: "CHF ",
    },
    CurrencyInfo {
        code: "CLP",
        exponent: 0,
        symbol: "CLP ",
    },
    CurrencyInfo {
        code: "CNY",
        exponent: 2,
        symbol: "CN¥",
    },
    CurrencyInfo {
        code: "DKK",
        exponent: 2,
        symbol: "DKK ",
    },
    CurrencyInfo {
        code: "EUR",
        exponent: 2,
        symbol: "€",
    },
    CurrencyInfo {
        code: "GBP",
        exponent: 2,
        symbol: "£",
    },
    CurrencyInfo {
        code: "HKD",
        exponent: 2,
        symbol: "HK$",
    },
    CurrencyInfo {
        code: "HUF",
        exponent: 2,
        symbol: "HUF ",
    },
    CurrencyInfo {
        code: "INR",
        exponent: 2,
        symbol: "₹",
    },
    CurrencyInfo {
        code: "ISK",
        exponent: 0,
        symbol: "ISK ",
    },
    CurrencyInfo {
        code: "JOD",
        exponent: 3,
        symbol: "JOD ",
    },
    CurrencyInfo {
        code: "JPY",
        exponent: 0,
        symbol: "¥",
    },
    CurrencyInfo {
        code: "KRW",
        exponent: 0,
        symbol: "₩",
    },
    CurrencyInfo {
        code: "KWD",
        exponent: 3,
        symbol: "KWD ",
    },
    CurrencyInfo {
        code: "MXN",
        exponent: 2,
        symbol: "MX$",
    },
    CurrencyInfo {
        code: "NOK",
        exponent: 2,
        symbol: "NOK ",
    },
    CurrencyInfo {
        code: "NZD",
        exponent: 2,
        symbol: "NZ$",
    },
    CurrencyInfo {
        code: "OMR",
        exponent: 3,
        symbol: "OMR ",
    },
    CurrencyInfo {
        code: "PKR",
        exponent: 2,
        symbol: "PKR ",
    },
    CurrencyInfo {
        code: "PLN",
        exponent: 2,
        symbol: "PLN ",
    },
    CurrencyInfo {
        code: "SAR",
        exponent: 2,
        symbol: "SAR ",
    },
    CurrencyInfo {
        code: "SEK",
        exponent: 2,
        symbol: "SEK ",
    },
    CurrencyInfo {
        code: "SGD",
        exponent: 2,
        symbol: "S$",
    },
    CurrencyInfo {
        code: "TND",
        exponent: 3,
        symbol: "TND ",
    },
    CurrencyInfo {
        code: "TRY",
        exponent: 2,
        symbol: "TRY ",
    },
    CurrencyInfo {
        code: "USD",
        exponent: 2,
        symbol: "$",
    },
    CurrencyInfo {
        code: "VND",
        exponent: 0,
        symbol: "₫",
    },
    CurrencyInfo {
        code: "ZAR",
        exponent: 2,
        symbol: "ZAR ",
    },
];

/// Default presentment rates from USD, used when `[generate] rates` does not
/// set one. They only need to be plausible: they shape test carts.
const DEFAULT_RATES_FROM_USD: &[(&str, &str)] = &[
    ("AUD", "1.52"),
    ("CAD", "1.37"),
    ("EUR", "0.92"),
    ("GBP", "0.79"),
    ("JPY", "150"),
    ("KWD", "0.307"),
    ("BHD", "0.376"),
    ("KRW", "1380"),
    ("INR", "83.2"),
    ("MXN", "17.1"),
];

pub fn currency(code: &str) -> Option<CurrencyInfo> {
    CURRENCIES.iter().copied().find(|c| c.code == code)
}

pub fn exponent(code: &str) -> Option<u32> {
    currency(code).map(|c| c.exponent)
}

/// A default presentment rate, store currency to `code`. Only USD stores have
/// bundled defaults; anything else falls back to 1.
pub fn default_rate(store: &str, code: &str) -> Decimal {
    if store == code {
        return Decimal::ONE;
    }
    if store != "USD" {
        return Decimal::ONE;
    }
    DEFAULT_RATES_FROM_USD
        .iter()
        .find(|(c, _)| *c == code)
        .and_then(|(_, r)| Decimal::from_str(r).ok())
        .unwrap_or(Decimal::ONE)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MoneyError {
    #[error("{0:?} is not a decimal number")]
    NotDecimal(String),
    #[error("{value:?} has more decimal places than the currency allows ({exponent})")]
    TooPrecise { value: String, exponent: u32 },
    #[error("{0:?} is too large")]
    Overflow(String),
    #[error("unknown currency {0:?}")]
    UnknownCurrency(String),
}

/// Parses a decimal string as Shopify sends it: `"120.00"`, `"10.0"`, `"7"`.
pub fn parse_decimal(s: &str) -> Result<Decimal, MoneyError> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(MoneyError::NotDecimal(s.to_string()));
    }
    Decimal::from_str(trimmed).map_err(|_| MoneyError::NotDecimal(s.to_string()))
}

/// Parses a decimal from a JSON value: a string, or a number (JS functions
/// send numbers). With serde_json's `arbitrary_precision` (switched on by
/// function-runner) `Number::to_string` keeps the exact literal.
pub fn decimal_from_json(v: &serde_json::Value) -> Option<Decimal> {
    match v {
        serde_json::Value::String(s) => parse_decimal(s).ok(),
        serde_json::Value::Number(n) => parse_decimal(&n.to_string()).ok(),
        _ => None,
    }
}

fn pow10(exponent: u32) -> i64 {
    10_i64.pow(exponent.min(9))
}

/// Exact conversion: `"120.00"` with exponent 2 is 12000. More decimal places
/// than the currency allows is an error unless they are zeros.
pub fn parse_minor(s: &str, exponent: u32) -> Result<i64, MoneyError> {
    let d = parse_decimal(s)?;
    decimal_to_minor_exact(d, exponent).map_err(|e| match e {
        MoneyError::TooPrecise { exponent, .. } => MoneyError::TooPrecise {
            value: s.to_string(),
            exponent,
        },
        other => other,
    })
}

pub fn decimal_to_minor_exact(d: Decimal, exponent: u32) -> Result<i64, MoneyError> {
    let scaled = d
        .checked_mul(Decimal::from(pow10(exponent)))
        .ok_or_else(|| MoneyError::Overflow(d.to_string()))?;
    if scaled.fract() != Decimal::ZERO {
        return Err(MoneyError::TooPrecise {
            value: d.to_string(),
            exponent,
        });
    }
    scaled
        .to_i64()
        .ok_or_else(|| MoneyError::Overflow(d.to_string()))
}

/// Rounding conversion, half away from zero. Used for function output, where
/// a percentage can produce fractions of a minor unit.
pub fn decimal_to_minor_rounded(d: Decimal, exponent: u32) -> Option<i64> {
    d.checked_mul(Decimal::from(pow10(exponent)))?
        .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
        .to_i64()
}

pub fn minor_to_decimal(minor: i64, exponent: u32) -> Decimal {
    Decimal::new(minor, exponent.min(9))
}

/// The decimal string Shopify uses in input: `12000` with exponent 2 is
/// `"120.00"`, with exponent 0 it is `"120"`.
pub fn format_amount(minor: i64, exponent: u32) -> String {
    minor_to_decimal(minor, exponent).to_string()
}

fn group_thousands(digits: &str) -> String {
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// Display form for reports and the terminal: `$1,234.50`, `¥15,000`,
/// `KWD 30.700`, `-$9.00`.
pub fn format_display(minor: i64, currency_code: &str) -> String {
    let (exp, symbol) = match currency(currency_code) {
        Some(c) => (c.exponent, c.symbol.to_string()),
        None => (2, format!("{currency_code} ")),
    };
    let negative = minor < 0;
    let abs = minor.unsigned_abs();
    let unit = 10_u64.pow(exp);
    let whole = abs / unit;
    let frac = abs % unit;
    let whole_str = group_thousands(&whole.to_string());
    let body = if exp == 0 {
        whole_str
    } else {
        format!("{whole_str}.{frac:0width$}", width = exp as usize)
    };
    if negative {
        format!("-{symbol}{body}")
    } else {
        format!("{symbol}{body}")
    }
}

/// Formats a whole number with thousands separators: `11021348` is
/// `11,021,348`.
pub fn group_int(n: u64) -> String {
    group_thousands(&n.to_string())
}

/// `percent` percent of `minor`, rounded half away from zero to a minor unit.
pub fn percent_of(minor: i64, percent: Decimal) -> i64 {
    Decimal::from(minor)
        .checked_mul(percent)
        .and_then(|v| v.checked_div(Decimal::ONE_HUNDRED))
        .map(|v| v.round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero))
        .and_then(|v| v.to_i64())
        .unwrap_or(0)
}

/// What percent `part` is of `whole`, to two decimal places. Zero when the
/// whole is zero.
pub fn percent_ratio(part: i64, whole: i64) -> Decimal {
    if whole == 0 {
        return Decimal::ZERO;
    }
    Decimal::from(part)
        .checked_mul(Decimal::ONE_HUNDRED)
        .and_then(|v| v.checked_div(Decimal::from(whole)))
        .map(|v| v.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero))
        .unwrap_or(Decimal::ZERO)
}

/// Converts an amount between currencies: `minor` in a currency with
/// `from_exp`, times `rate`, into a currency with `to_exp`, rounded half away
/// from zero.
pub fn convert(minor: i64, from_exp: u32, rate: Decimal, to_exp: u32) -> i64 {
    minor_to_decimal(minor, from_exp)
        .checked_mul(rate)
        .and_then(|d| decimal_to_minor_rounded(d, to_exp))
        .unwrap_or(0)
}

/// Splits `total` across `weights` so the parts always add up to `total`:
/// each part is the floor of its share, and the leftover units go to the
/// largest remainders (ties go to the earlier index). All-zero weights split
/// evenly.
pub fn allocate(total: i64, weights: &[i64]) -> Vec<i64> {
    if weights.is_empty() {
        return Vec::new();
    }
    let clean: Vec<i128> = weights.iter().map(|w| i128::from((*w).max(0))).collect();
    let sum: i128 = clean.iter().sum();
    let clean = if sum == 0 {
        vec![1_i128; weights.len()]
    } else {
        clean
    };
    let sum: i128 = clean.iter().sum();
    let negative = total < 0;
    let t = i128::from(total).abs();
    let mut parts: Vec<i128> = Vec::with_capacity(clean.len());
    let mut remainders: Vec<(i128, usize)> = Vec::with_capacity(clean.len());
    for (i, w) in clean.iter().enumerate() {
        let num = t * w;
        parts.push(num / sum);
        remainders.push((num % sum, i));
    }
    let assigned: i128 = parts.iter().sum();
    let mut left = t - assigned;
    remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, idx) in remainders {
        if left <= 0 {
            break;
        }
        if let Some(p) = parts.get_mut(idx) {
            *p += 1;
            left -= 1;
        }
    }
    parts
        .into_iter()
        .map(|p| {
            let v = i64::try_from(p).unwrap_or(i64::MAX);
            if negative { -v } else { v }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn exponents_from_the_table() {
        assert_eq!(exponent("USD"), Some(2));
        assert_eq!(exponent("JPY"), Some(0));
        assert_eq!(exponent("KWD"), Some(3));
        assert_eq!(exponent("XXX"), None);
    }

    #[test]
    fn parses_shopify_decimal_strings() {
        assert_eq!(parse_minor("120.00", 2), Ok(12000));
        assert_eq!(parse_minor("10.0", 2), Ok(1000));
        assert_eq!(parse_minor("7", 2), Ok(700));
        assert_eq!(parse_minor("15000", 0), Ok(15000));
        assert_eq!(parse_minor("30.700", 3), Ok(30700));
        assert!(parse_minor("1.50", 0).is_err());
        assert!(parse_minor("abc", 2).is_err());
        assert_eq!(parse_minor("100.000", 2), Ok(10000));
    }

    #[test]
    fn decimals_from_json_strings_and_numbers() {
        assert_eq!(
            decimal_from_json(&serde_json::json!("10.0")),
            Some(Decimal::from(10))
        );
        assert_eq!(
            decimal_from_json(&serde_json::json!(10)),
            Some(Decimal::from(10))
        );
        assert_eq!(decimal_from_json(&serde_json::json!(null)), None);
    }

    #[test]
    fn formats_amounts_like_shopify() {
        assert_eq!(format_amount(12000, 2), "120.00");
        assert_eq!(format_amount(15000, 0), "15000");
        assert_eq!(format_amount(30700, 3), "30.700");
        assert_eq!(format_amount(5, 2), "0.05");
    }

    #[test]
    fn formats_display_money() {
        assert_eq!(format_display(10000, "USD"), "$100.00");
        assert_eq!(format_display(123456, "USD"), "$1,234.56");
        assert_eq!(format_display(15000, "JPY"), "¥15,000");
        assert_eq!(format_display(30700, "KWD"), "KWD 30.700");
        assert_eq!(format_display(-900, "USD"), "-$9.00");
        assert_eq!(group_int(11021348), "11,021,348");
    }

    #[test]
    fn percentages_round_half_away_from_zero() {
        assert_eq!(percent_of(10000, Decimal::from(10)), 1000);
        assert_eq!(percent_of(9999, Decimal::from(10)), 1000); // 999.9
        assert_eq!(percent_of(5, Decimal::from(10)), 1); // 0.5
        assert_eq!(percent_of(4, Decimal::from(10)), 0); // 0.4
        assert_eq!(percent_ratio(900, 9000), Decimal::from(10));
        assert_eq!(percent_ratio(1, 0), Decimal::ZERO);
    }

    #[test]
    fn converts_between_exponents() {
        // $100.00 at 150 JPY per USD is 15,000 JPY.
        assert_eq!(convert(10000, 2, Decimal::from(150), 0), 15000);
        // $100.00 at 0.307 KWD per USD is 30.700 KWD.
        assert_eq!(
            convert(10000, 2, Decimal::from_str("0.307").unwrap_or_default(), 3),
            30700
        );
    }

    #[test]
    fn largest_remainder_allocation() {
        assert_eq!(allocate(100, &[1, 1, 1]), vec![34, 33, 33]);
        assert_eq!(allocate(10, &[0, 0]), vec![5, 5]);
        assert_eq!(allocate(-7, &[1, 1]), vec![-4, -3]);
        assert_eq!(allocate(1000, &[3000, 1000]), vec![750, 250]);
        assert_eq!(allocate(5, &[]), Vec::<i64>::new());
    }

    proptest! {
        #[test]
        fn allocations_always_sum_to_the_total(
            total in -1_000_000_i64..1_000_000,
            weights in proptest::collection::vec(0_i64..100_000, 1..40)
        ) {
            let parts = allocate(total, &weights);
            prop_assert_eq!(parts.len(), weights.len());
            prop_assert_eq!(parts.iter().sum::<i64>(), total);
        }

        #[test]
        fn minor_units_round_trip(minor in -10_000_000_i64..10_000_000, exp in 0_u32..4) {
            let s = format_amount(minor, exp);
            prop_assert_eq!(parse_minor(&s, exp), Ok(minor));
        }
    }
}
