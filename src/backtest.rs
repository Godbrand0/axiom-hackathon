//! Event-style backtest on daily bars.
//! Rules: signal on day i's close -> fill at day i+1's open. Stop is checked before target within a bar
//! (conservative). Gaps through a level fill at the open. Costs are charged per side.
use chrono::NaiveDate;

use crate::indicators::Indicators;
use crate::models::Bar;
use crate::setups::Setup;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Stop,
    Target,
    Time,
    /// The setup's own exit rule fired at the close.
    Rule,
    /// Still open at the end of the data.
    Open,
}

impl ExitReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExitReason::Stop => "stop",
            ExitReason::Target => "target",
            ExitReason::Time => "time",
            ExitReason::Rule => "rule",
            ExitReason::Open => "open",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Trade {
    pub symbol: String,
    pub signal_date: NaiveDate,
    pub entry_date: NaiveDate,
    pub exit_date: NaiveDate,
    pub entry: f64,
    pub exit: f64,
    pub reason: ExitReason,
    /// Net of costs, as a fraction (0.05 = +5%).
    pub ret: f64,
    pub r_multiple: f64,
    pub hold_days: usize,
}

pub struct Exit {
    pub idx: usize,
    pub price: f64,
    pub reason: ExitReason,
}

/// Walks forward from the entry bar until stop, target, the setup's exit rule (checked on the close), or time limit.
pub fn simulate_exit(
    bars: &[Bar],
    entry_idx: usize,
    stop: f64,
    target: f64,
    max_hold: usize,
    exit_on_close: &dyn Fn(usize) -> bool,
) -> Exit {
    for j in entry_idx..bars.len() {
        let b = &bars[j];
        if b.low <= stop {
            return Exit { idx: j, price: b.open.min(stop), reason: ExitReason::Stop };
        }
        if b.high >= target {
            return Exit { idx: j, price: b.open.max(target), reason: ExitReason::Target };
        }
        if exit_on_close(j) {
            return Exit { idx: j, price: b.close, reason: ExitReason::Rule };
        }
        if j + 1 - entry_idx >= max_hold {
            return Exit { idx: j, price: b.close, reason: ExitReason::Time };
        }
    }
    let last = bars.len() - 1;
    Exit { idx: last, price: bars[last].close, reason: ExitReason::Open }
}

/// Runs one setup over one symbol's full history. One position at a time.
pub fn run_symbol(symbol: &str, bars: &[Bar], setup: &dyn Setup, cost_bps: f64) -> Vec<Trade> {
    let ind = Indicators::compute(bars);
    let cost = 2.0 * cost_bps / 10_000.0;
    let mut trades = Vec::new();
    let mut i = 0;
    while i + 1 < bars.len() {
        let Some(sig) = setup.evaluate(symbol, bars, &ind, i) else {
            i += 1;
            continue;
        };
        let ei = i + 1;
        let entry = bars[ei].open;
        // Gapped through a level overnight: the setup as defined no longer exists, skip it.
        if entry <= sig.stop || entry >= sig.target {
            i += 1;
            continue;
        }
        let exit =
            simulate_exit(bars, ei, sig.stop, sig.target, setup.max_hold_days(), &|j| setup.exit_on_close(bars, &ind, j));
        if exit.reason == ExitReason::Open {
            break; // don't score unfinished trades
        }
        trades.push(Trade {
            symbol: symbol.to_string(),
            signal_date: bars[i].date,
            entry_date: bars[ei].date,
            exit_date: bars[exit.idx].date,
            entry,
            exit: exit.price,
            reason: exit.reason,
            ret: exit.price / entry - 1.0 - cost,
            r_multiple: (exit.price - entry) / (entry - sig.stop),
            hold_days: exit.idx + 1 - ei,
        });
        i = exit.idx + 1;
    }
    trades
}

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub n: usize,
    pub win_rate: f64,
    pub avg_win: f64,
    pub avg_loss: f64,
    pub expectancy: f64,
    pub profit_factor: f64,
    pub avg_r: f64,
    pub avg_hold: f64,
}

pub fn stats<'a>(trades: impl IntoIterator<Item = &'a Trade>) -> Stats {
    let t: Vec<&Trade> = trades.into_iter().collect();
    if t.is_empty() {
        return Stats::default();
    }
    let n = t.len() as f64;
    let wins: Vec<f64> = t.iter().filter(|x| x.ret > 0.0).map(|x| x.ret).collect();
    let losses: Vec<f64> = t.iter().filter(|x| x.ret <= 0.0).map(|x| x.ret).collect();
    let gross_win: f64 = wins.iter().sum();
    let gross_loss: f64 = -losses.iter().sum::<f64>();
    Stats {
        n: t.len(),
        win_rate: wins.len() as f64 / n,
        avg_win: if wins.is_empty() { 0.0 } else { gross_win / wins.len() as f64 },
        avg_loss: if losses.is_empty() { 0.0 } else { -gross_loss / losses.len() as f64 },
        expectancy: t.iter().map(|x| x.ret).sum::<f64>() / n,
        profit_factor: if gross_loss > 0.0 { gross_win / gross_loss } else { f64::INFINITY },
        avg_r: t.iter().map(|x| x.r_multiple).sum::<f64>() / n,
        avg_hold: t.iter().map(|x| x.hold_days as f64).sum::<f64>() / n,
    }
}

impl Stats {
    pub fn line(&self) -> String {
        if self.n == 0 {
            return "no completed trades".into();
        }
        format!(
            "{} trades | win {:.0}% | avg win {:+.2}% | avg loss {:+.2}% | expectancy {:+.2}%/trade | PF {:.2} | avg {:+.2}R | avg hold {:.1}d",
            self.n,
            self.win_rate * 100.0,
            self.avg_win * 100.0,
            self.avg_loss * 100.0,
            self.expectancy * 100.0,
            self.profit_factor,
            self.avg_r,
            self.avg_hold
        )
    }
}

/// Average return of simply holding the benchmark for `hold` sessions, from every start day in [from, to].
/// This is the bar a setup's per-trade expectancy has to clear.
pub fn benchmark_forward_return(bench: &[Bar], hold: usize, from: NaiveDate, to: NaiveDate) -> Option<f64> {
    let hold = hold.max(1);
    let rets: Vec<f64> = (0..bench.len().saturating_sub(hold))
        .filter(|&i| bench[i].date >= from && bench[i].date <= to)
        .map(|i| bench[i + hold].close / bench[i].close - 1.0)
        .collect();
    (!rets.is_empty()).then(|| rets.iter().sum::<f64>() / rets.len() as f64)
}

/// Average `hold`-session forward return from *every* day for *every* symbol in the universe (random entry).
/// A setup only has a timing edge if it beats this; beating SPY alone can just mean the
/// watchlist was picked with hindsight or is higher-beta than the index.
pub fn universe_forward_return(
    bars: &std::collections::HashMap<String, Vec<Bar>>,
    symbols: &[String],
    hold: usize,
    from: NaiveDate,
    to: NaiveDate,
    added: &std::collections::HashMap<String, NaiveDate>,
) -> Option<f64> {
    let hold = hold.max(1);
    let (mut sum, mut n) = (0.0, 0usize);
    for s in symbols {
        let Some(b) = bars.get(s) else { continue };
        let from = added.get(s).map_or(from, |&a| a.max(from));
        // Start at 200 to match the history the setups need before they can fire.
        for i in 200..b.len().saturating_sub(hold) {
            if b[i].date >= from && b[i].date <= to {
                sum += b[i + hold].close / b[i].close - 1.0;
                n += 1;
            }
        }
    }
    (n > 0).then(|| sum / n as f64)
}

/// 90% bootstrap confidence interval of the mean (deterministic RNG, so runs are reproducible).
pub fn bootstrap_ci(values: &[f64], iters: usize) -> Option<(f64, f64)> {
    if values.len() < 2 {
        return None;
    }
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let n = values.len();
    let mut means: Vec<f64> = (0..iters)
        .map(|_| (0..n).map(|_| values[(next() % n as u64) as usize]).sum::<f64>() / n as f64)
        .collect();
    means.sort_by(|a, b| a.total_cmp(b));
    Some((means[iters * 5 / 100], means[iters * 95 / 100]))
}

pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let m = v.len() / 2;
    Some(if v.len() % 2 == 0 { (v[m - 1] + v[m]) / 2.0 } else { v[m] })
}

/// Mean after dropping the `k` best results: shows whether a few outliers carry the whole edge.
pub fn mean_excluding_best(values: &[f64], k: usize) -> Option<f64> {
    if values.len() <= k {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| b.total_cmp(a));
    let rest = &v[k..];
    Some(rest.iter().sum::<f64>() / rest.len() as f64)
}

pub fn buy_and_hold(bench: &[Bar], from: NaiveDate, to: NaiveDate) -> Option<f64> {
    let first = bench.iter().find(|b| b.date >= from)?;
    let last = bench.iter().rev().find(|b| b.date <= to)?;
    Some(last.close / first.close - 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(o: f64, h: f64, l: f64, c: f64) -> Bar {
        Bar { date: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(), open: o, high: h, low: l, close: c, volume: 1.0 }
    }

    #[test]
    fn stop_checked_before_target_in_same_bar() {
        let bars = vec![bar(100.0, 120.0, 80.0, 100.0)];
        let e = simulate_exit(&bars, 0, 90.0, 110.0, 10, &|_| false);
        assert_eq!(e.reason, ExitReason::Stop);
        assert_eq!(e.price, 90.0);
    }

    #[test]
    fn gap_down_fills_at_open() {
        let bars = vec![bar(100.0, 101.0, 99.0, 100.0), bar(85.0, 86.0, 84.0, 85.0)];
        let e = simulate_exit(&bars, 0, 90.0, 110.0, 10, &|_| false);
        assert_eq!(e.reason, ExitReason::Stop);
        assert_eq!(e.price, 85.0);
    }

    #[test]
    fn robustness_helpers() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(mean_excluding_best(&[100.0, 1.0, 1.0], 1), Some(1.0));
        let (lo, hi) = bootstrap_ci(&[1.0, 2.0, 3.0, 4.0, 5.0], 2000).unwrap();
        assert!(lo < 3.0 && hi > 3.0 && lo >= 1.0 && hi <= 5.0);
    }

    #[test]
    fn rule_exit_at_close() {
        let bars = vec![bar(100.0, 101.0, 99.0, 100.0), bar(100.0, 103.0, 99.0, 102.0)];
        let e = simulate_exit(&bars, 0, 90.0, 110.0, 10, &|j| j == 1);
        assert_eq!(e.reason, ExitReason::Rule);
        assert_eq!((e.idx, e.price), (1, 102.0));
    }

    #[test]
    fn time_exit_at_close() {
        let bars = vec![bar(100.0, 101.0, 99.0, 100.0), bar(100.0, 101.0, 99.0, 100.5)];
        let e = simulate_exit(&bars, 0, 90.0, 110.0, 2, &|_| false);
        assert_eq!(e.reason, ExitReason::Time);
        assert_eq!(e.price, 100.5);
    }
}
