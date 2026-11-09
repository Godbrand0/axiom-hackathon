//! Long-term factor rankings from point-in-time fundamentals, held as monthly portfolios.
//!
//! - quality: cash-based profitability = trailing-12-month operating cash flow / total assets
//!   (Novy-Marx 2013; Ball, Gerakos, Linnainmaa & Nikolaev 2016).
//! - value: earnings yield = trailing-12-month diluted EPS / unadjusted price (Basu 1977; Fama & French 1992).
//! - quality-value: average of the two percentile ranks (Asness, Frazzini & Pedersen 2019).
use std::collections::HashMap;

use chrono::NaiveDate;

use crate::fundamentals::{latest_instant, ttm, Facts, ASSETS, OCF};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Factor {
    Quality,
    Value,
    QualityValue,
}

impl Factor {
    pub const ALL: [Factor; 3] = [Factor::Quality, Factor::Value, Factor::QualityValue];

    pub fn name(&self) -> &'static str {
        match self {
            Factor::Quality => "quality",
            Factor::Value => "value",
            Factor::QualityValue => "quality-value",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == s)
    }

    pub fn description(&self) -> &'static str {
        match self {
            Factor::Quality => "Portfolio. At each month-end, rank stocks by trailing-12-month operating cash flow / total assets, \
                 using only 10-Q/10-K data filed by that date; hold the top 10% equal-weight for a month.",
            Factor::Value => "Portfolio. At each month-end, rank stocks by earnings yield (trailing-12-month diluted EPS / \
                 month-end price as quoted then); hold the top 10% equal-weight for a month.",
            Factor::QualityValue => "Portfolio. At each month-end, average each stock's percentile rank on quality and on value; \
                 hold the top 10% equal-weight for a month.",
        }
    }
}

pub fn quality(f: &Facts, d: NaiveDate) -> Option<f64> {
    let assets = latest_instant(f.get(ASSETS), d)?;
    (assets > 0.0).then_some(())?;
    Some(ttm(f.get(OCF), d)? / assets)
}

pub fn earnings_yield(f: &Facts, d: NaiveDate, price: f64) -> Option<f64> {
    (price > 0.0).then_some(())?;
    Some(ttm(f.eps(), d)? / price)
}

/// Percentile rank (0 = worst, 1 = best) of each value.
fn pct_ranks(v: &[(String, f64)]) -> HashMap<String, f64> {
    let mut sorted: Vec<&(String, f64)> = v.iter().collect();
    sorted.sort_by(|a, b| a.1.total_cmp(&b.1));
    let n = (sorted.len().max(2) - 1) as f64;
    sorted.iter().enumerate().map(|(i, (s, _))| (s.clone(), i as f64 / n)).collect()
}

/// Ranking on date `d`, best first. `eligible` filters index membership; `price` gives the unadjusted price.
pub fn ranking(
    factor: Factor,
    facts: &HashMap<String, Facts>,
    symbols: &[String],
    d: NaiveDate,
    eligible: &dyn Fn(&str) -> bool,
    price: &dyn Fn(&str) -> Option<f64>,
) -> Vec<(String, f64)> {
    let members = symbols.iter().filter(|s| eligible(s));
    let q: Vec<(String, f64)> = match factor {
        Factor::Value => vec![],
        _ => members.clone().filter_map(|s| Some((s.clone(), quality(facts.get(s)?, d)?))).collect(),
    };
    let v: Vec<(String, f64)> = match factor {
        Factor::Quality => vec![],
        _ => members.filter_map(|s| Some((s.clone(), earnings_yield(facts.get(s)?, d, price(s)?)?))).collect(),
    };
    let mut out = match factor {
        Factor::Quality => q,
        Factor::Value => v,
        Factor::QualityValue => {
            let (rq, rv) = (pct_ranks(&q), pct_ranks(&v));
            rq.iter().filter_map(|(s, a)| Some((s.clone(), (a + rv.get(s)?) / 2.0))).collect()
        }
    };
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pct_ranks_order() {
        let r = pct_ranks(&[("A".into(), 3.0), ("B".into(), 1.0), ("C".into(), 2.0)]);
        assert_eq!((r["B"], r["C"], r["A"]), (0.0, 0.5, 1.0));
    }
}
