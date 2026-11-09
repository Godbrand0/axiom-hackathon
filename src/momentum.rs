//! Cross-sectional 12-1 momentum (Jegadeesh & Titman 1993; Asness, Moskowitz & Pedersen 2013).
//!
//! Each month-end, rank every liquid stock by its return from 12 months ago to 1 month ago (the most
//! recent month is skipped because it tends to reverse), buy the top fraction equal-weight at the next
//! open, and hold until the next rebalance. The honest baseline is the equal-weight universe: beating
//! SPY alone can come from simply holding mid-caps equal-weight.
use std::collections::{HashMap, HashSet};

use chrono::{Datelike, NaiveDate};

use crate::models::Bar;

pub const NAME: &str = "momentum-12-1";
pub const DESCRIPTION: &str = "Portfolio. At each month-end, rank liquid stocks (avg dollar volume >= $20M) by return \
     from 252 to 21 sessions ago; buy the top 10% equal-weight at the next open, hold one month, rebalance.";

const LOOKBACK: usize = 252;
const SKIP: usize = 21;
const MIN_DOLLAR_VOLUME: f64 = 20_000_000.0;
const MIN_ELIGIBLE: usize = 20;

fn index_of(bars: &[Bar], d: NaiveDate) -> Option<usize> {
    bars.binary_search_by_key(&d, |b| b.date).ok()
}

/// 12-1 momentum score at bar `k`, or None if history is too short or the stock is illiquid.
pub fn score(bars: &[Bar], k: usize) -> Option<f64> {
    if k < LOOKBACK || k >= bars.len() {
        return None;
    }
    let dollar_vol = bars[k + 1 - 50..=k].iter().map(|b| b.close * b.volume).sum::<f64>() / 50.0;
    (dollar_vol >= MIN_DOLLAR_VOLUME).then(|| bars[k - SKIP].close / bars[k - LOOKBACK].close - 1.0)
}

/// Symbols ranked by score on date `d`, best first.
/// Only stocks that were index members on `d` (per `added`; absent = always a member) are ranked.
pub fn ranking(
    bars: &HashMap<String, Vec<Bar>>,
    symbols: &[String],
    d: NaiveDate,
    added: &HashMap<String, NaiveDate>,
) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = symbols
        .iter()
        .filter(|s| added.get(*s).is_none_or(|&a| a <= d))
        .filter_map(|s| {
            let b = bars.get(s)?;
            Some((s.clone(), score(b, index_of(b, d)?)?))
        })
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

pub fn top_count(eligible: usize, top_frac: f64) -> usize {
    ((eligible as f64 * top_frac).round() as usize).max(1)
}

/// One holding month.
pub struct Period {
    /// Rebalance (ranking) date.
    pub date: NaiveDate,
    /// Top-ranked basket, net of turnover costs.
    pub top: f64,
    /// Equal-weight of every eligible stock (the baseline).
    pub universe: f64,
    pub spy: f64,
}

/// Indices of the last session of each month (excluding the final, possibly unfinished one).
fn month_ends(cal: &[Bar]) -> Vec<usize> {
    (0..cal.len().saturating_sub(1)).filter(|&i| cal[i + 1].date.month() != cal[i].date.month()).collect()
}

/// Open of the session after `d0` to the open of the session after `d1`.
fn open_to_open(bars: &[Bar], d0: NaiveDate, d1: NaiveDate) -> Option<f64> {
    let (k0, k1) = (index_of(bars, d0)?, index_of(bars, d1)?);
    Some(bars.get(k1 + 1)?.open / bars.get(k0 + 1)?.open - 1.0)
}

pub fn run(
    bars: &HashMap<String, Vec<Bar>>,
    symbols: &[String],
    spy: &[Bar],
    top_frac: f64,
    cost_bps: f64,
    added: &HashMap<String, NaiveDate>,
) -> Vec<Period> {
    run_ranked(bars, spy, top_frac, cost_bps, &|d| ranking(bars, symbols, d, added))
}

/// Monthly top-fraction portfolio for any ranking (best first). The ranking function is called with
/// each month-end date and must only use information available on that date.
pub fn run_ranked(
    bars: &HashMap<String, Vec<Bar>>,
    spy: &[Bar],
    top_frac: f64,
    cost_bps: f64,
    rank: &dyn Fn(NaiveDate) -> Vec<(String, f64)>,
) -> Vec<Period> {
    let ends = month_ends(spy);
    let mut prev: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for w in ends.windows(2) {
        let (d0, d1) = (spy[w[0]].date, spy[w[1]].date);
        let ranked = rank(d0);
        if ranked.len() < MIN_ELIGIBLE {
            continue;
        }
        let n_top = top_count(ranked.len(), top_frac);
        let ret = |s: &str| open_to_open(bars.get(s)?, d0, d1);
        let top: Vec<(String, f64)> =
            ranked[..n_top].iter().filter_map(|(s, _)| Some((s.clone(), ret(s)?))).collect();
        let all: Vec<f64> = ranked.iter().filter_map(|(s, _)| ret(s)).collect();
        let (Some(spy_ret), false) = (open_to_open(spy, d0, d1), top.is_empty()) else { continue };

        let current: HashSet<String> = top.iter().map(|(s, _)| s.clone()).collect();
        // Fraction of the basket replaced; each replacement is a sell and a buy.
        let turnover = current.difference(&prev).count() as f64 / current.len() as f64;
        let cost = turnover * 2.0 * cost_bps / 10_000.0;
        prev = current;

        out.push(Period {
            date: d0,
            top: top.iter().map(|(_, r)| r).sum::<f64>() / top.len() as f64 - cost,
            universe: all.iter().sum::<f64>() / all.len() as f64,
            spy: spy_ret,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(start: NaiveDate, closes: &[f64]) -> Vec<Bar> {
        closes
            .iter()
            .enumerate()
            .map(|(i, &c)| Bar {
                date: start + chrono::TimeDelta::days(i as i64),
                open: c,
                high: c,
                low: c,
                close: c,
                volume: 1e9,
            })
            .collect()
    }

    #[test]
    fn score_skips_last_month() {
        let d = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
        // Flat at 100, doubles to 200 by 21 bars ago, then crashes to 50 in the last month.
        let mut c = vec![100.0; LOOKBACK - SKIP];
        c.extend(vec![200.0; SKIP]);
        c.push(50.0);
        let b = series(d, &c);
        let k = b.len() - 1;
        assert_eq!(k, LOOKBACK);
        assert!((score(&b, k).unwrap() - 1.0).abs() < 1e-9); // +100%, recent crash ignored
        assert!(score(&b, k - 1).is_none()); // not enough history
    }

    #[test]
    fn top_count_rounds_and_floors_at_one() {
        assert_eq!(top_count(503, 0.1), 50);
        assert_eq!(top_count(3, 0.1), 1);
    }
}
