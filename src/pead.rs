//! Post-earnings-announcement drift (Ball & Brown 1968; Bernard & Thomas 1989).
//!
//! For each quarter, the earnings surprise (SUE) comes from the company's own reported EPS
//! (see `fundamentals::surprises`). Two entry points are measured:
//! - after the 10-Q/10-K is filed: strictly point-in-time, and what Axiom can actually act on,
//!   since free SEC data arrives with the filing, usually days to weeks after the press release;
//! - after the price reaction day: the day between quarter end and filing with the largest
//!   volume-weighted overnight gap, a proxy for the announcement. This shows how much drift is
//!   left when you already have the press-release numbers.
use chrono::NaiveDate;

use crate::fundamentals::{surprises, Facts};
use crate::models::Bar;

pub const NAME: &str = "earnings-drift";
pub const DESCRIPTION: &str = "Event study. Long after a strong earnings beat (SUE >= 2: EPS vs the same quarter a year earlier, \
     scaled by its past variability), held 60 sessions with no stop. Strong misses (SUE <= -2) shown for contrast.";
pub const HOLD: usize = 60;
pub const BEAT: f64 = 2.0;
pub const MISS: f64 = -2.0;

#[derive(Debug, Clone)]
pub struct Event {
    pub symbol: String,
    pub period_end: NaiveDate,
    pub filed: NaiveDate,
    /// Estimated announcement reaction day, if found in the bars.
    pub reaction: Option<NaiveDate>,
    /// Overnight gap on the reaction day.
    pub reaction_gap: Option<f64>,
    pub eps: f64,
    pub eps_year_ago: f64,
    pub sue: f64,
    /// Net return holding HOLD sessions from the open after the filing.
    pub ret_after_filing: Option<f64>,
    /// Net return holding HOLD sessions from the open after the reaction day.
    pub ret_after_reaction: Option<f64>,
}

fn forward(bars: &[Bar], entry: usize, cost: f64) -> Option<f64> {
    let exit = bars.get(entry + HOLD - 1)?;
    Some(exit.close / bars.get(entry)?.open - 1.0 - cost)
}

/// Bar index of the most likely announcement reaction in (period_end, filed]:
/// largest overnight gap times relative volume.
fn reaction_day(bars: &[Bar], period_end: NaiveDate, filed: NaiveDate) -> Option<usize> {
    let lo = bars.partition_point(|b| b.date <= period_end).max(50);
    let hi = bars.partition_point(|b| b.date <= filed);
    (lo..hi)
        .map(|k| {
            let avg = bars[k - 50..k].iter().map(|b| b.volume).sum::<f64>() / 50.0;
            let gap = (bars[k].open / bars[k - 1].close - 1.0).abs();
            (k, gap * bars[k].volume / avg.max(1.0))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(k, _)| k)
}

pub fn events(symbol: &str, facts: &Facts, bars: &[Bar], cost_bps: f64) -> Vec<Event> {
    let cost = 2.0 * cost_bps / 10_000.0;
    surprises(facts.eps())
        .into_iter()
        .map(|s| {
            let r = reaction_day(bars, s.end, s.first_filed);
            let after_filing = bars.partition_point(|b| b.date <= s.first_filed);
            Event {
                symbol: symbol.to_string(),
                period_end: s.end,
                filed: s.first_filed,
                reaction: r.map(|k| bars[k].date),
                reaction_gap: r.map(|k| bars[k].open / bars[k - 1].close - 1.0),
                eps: s.eps,
                eps_year_ago: s.eps_year_ago,
                sue: s.sue,
                ret_after_filing: forward(bars, after_filing, cost),
                ret_after_reaction: r.and_then(|k| forward(bars, k + 1, cost)),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_gap_day_and_measures_forward_return() {
        let d0 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let mut bars: Vec<Bar> = (0..200)
            .map(|i| Bar {
                date: d0 + chrono::TimeDelta::days(i),
                open: 100.0,
                high: 100.0,
                low: 100.0,
                close: 100.0,
                volume: 1000.0,
            })
            .collect();
        // Announcement reaction on day 80: gap to 110 on 5x volume, then flat at 110.
        for b in bars.iter_mut().skip(80) {
            (b.open, b.close) = (110.0, 110.0);
        }
        bars[80].volume = 5000.0;
        let k = reaction_day(&bars, d0 + chrono::TimeDelta::days(60), d0 + chrono::TimeDelta::days(100)).unwrap();
        assert_eq!(k, 80);
        assert!(forward(&bars, k + 1, 0.0).unwrap().abs() < 1e-12);
        assert!(forward(&bars, 190, 0.0).is_none()); // not enough future bars
    }
}
