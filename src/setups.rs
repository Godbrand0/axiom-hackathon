//! Named, rule-based setups. Each is explicit enough to backtest, unlike a hand-weighted score.
//! `evaluate` may only look at bars[..=i].
use crate::indicators::Indicators;
use crate::models::{Bar, Evidence, Signal, Stance};

/// Minimum average daily dollar volume, to avoid illiquid names.
const MIN_DOLLAR_VOLUME: f64 = 20_000_000.0;

pub trait Setup: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn max_hold_days(&self) -> usize;
    fn evaluate(&self, symbol: &str, bars: &[Bar], ind: &Indicators, i: usize) -> Option<Signal>;
    /// Setup-specific exit, checked on each close after entry (stop/target are checked first).
    fn exit_on_close(&self, _bars: &[Bar], _ind: &Indicators, _j: usize) -> bool {
        false
    }
}

pub fn all() -> Vec<Box<dyn Setup>> {
    vec![Box::new(TrendPullback), Box::new(Breakout52w), Box::new(Rsi2Pullback), Box::new(GapDrift)]
}

pub fn by_name(name: &str) -> Option<Box<dyn Setup>> {
    all().into_iter().find(|s| s.name() == name)
}

fn volume_evidence(vol: f64, avg: f64, needed: f64) -> Evidence {
    let ratio = vol / avg;
    let stance = if ratio >= needed { Stance::Supports } else { Stance::Opposes };
    Evidence::technical(stance, format!("Volume {:.0} is {:.2}x the 50-day average ({:.0})", vol, ratio, avg))
}

/// Swing long: established uptrend, pullback to the 21-day EMA, RSI reset, bullish close.
pub struct TrendPullback;

impl Setup for TrendPullback {
    fn name(&self) -> &'static str {
        "trend-pullback"
    }
    fn description(&self) -> &'static str {
        "Long. Close > SMA200 and SMA50 > SMA200; day's low within 1% of EMA21 and close back above it; \
         RSI(14) 35-55; close > open; avg dollar volume >= $20M. Stop = entry - 2*ATR14, target = entry + 4*ATR14, max hold 20 sessions."
    }
    fn max_hold_days(&self) -> usize {
        20
    }
    fn evaluate(&self, symbol: &str, bars: &[Bar], ind: &Indicators, i: usize) -> Option<Signal> {
        let b = &bars[i];
        let (sma50, sma200, ema21) = (ind.sma50[i]?, ind.sma200[i]?, ind.ema21[i]?);
        let (rsi, atr, vavg) = (ind.rsi14[i]?, ind.atr14[i]?, ind.vol_avg50[i]?);

        let uptrend = b.close > sma200 && sma50 > sma200;
        let touched_ema = b.low <= ema21 * 1.01 && b.close > ema21;
        let rsi_reset = (35.0..=55.0).contains(&rsi);
        let bullish = b.close > b.open;
        let liquid = b.close * vavg >= MIN_DOLLAR_VOLUME;
        if !(uptrend && touched_ema && rsi_reset && bullish && liquid) {
            return None;
        }

        let entry = b.close;
        let mut evidence = vec![
            Evidence::technical(
                Stance::Supports,
                format!("Uptrend: close {:.2} > SMA200 {:.2}, and SMA50 {:.2} > SMA200", b.close, sma200, sma50),
            ),
            Evidence::technical(
                Stance::Supports,
                format!("Pullback held the 21-day EMA ({:.2}): low {:.2}, close {:.2}", ema21, b.low, b.close),
            ),
            Evidence::technical(Stance::Supports, format!("RSI(14) {:.1}: momentum reset without breakdown", rsi)),
            Evidence::technical(Stance::Supports, format!("Bullish session: open {:.2} -> close {:.2}", b.open, b.close)),
            volume_evidence(b.volume, vavg, 1.0),
        ];
        let ext = (b.close / sma50 - 1.0) * 100.0;
        if ext > 15.0 {
            evidence.push(Evidence::technical(
                Stance::Opposes,
                format!("Price is {:.1}% above SMA50: extended, larger pullback risk", ext),
            ));
        }
        Some(Signal {
            symbol: symbol.to_string(),
            setup: self.name().into(),
            as_of: b.date,
            entry,
            stop: entry - 2.0 * atr,
            target: entry + 4.0 * atr,
            max_hold_days: self.max_hold_days(),
            evidence,
        })
    }
}

/// Swing/position long: close at a new 52-week high on heavy volume.
pub struct Breakout52w;

impl Setup for Breakout52w {
    fn name(&self) -> &'static str {
        "breakout-52w"
    }
    fn description(&self) -> &'static str {
        "Long. Close above the prior 252-session high; volume >= 1.5x 50-day average; close > SMA50; \
         avg dollar volume >= $20M. Stop = entry - 2*ATR14, target = entry + 4*ATR14, max hold 40 sessions."
    }
    fn max_hold_days(&self) -> usize {
        40
    }
    fn evaluate(&self, symbol: &str, bars: &[Bar], ind: &Indicators, i: usize) -> Option<Signal> {
        if i == 0 {
            return None;
        }
        let b = &bars[i];
        let prior_high = ind.high252[i - 1]?;
        let (sma50, rsi, atr, vavg) = (ind.sma50[i]?, ind.rsi14[i]?, ind.atr14[i]?, ind.vol_avg50[i]?);

        let breakout = b.close > prior_high;
        let heavy = b.volume >= 1.5 * vavg;
        let liquid = b.close * vavg >= MIN_DOLLAR_VOLUME;
        if !(breakout && heavy && b.close > sma50 && liquid) {
            return None;
        }

        let entry = b.close;
        let mut evidence = vec![
            Evidence::technical(
                Stance::Supports,
                format!("Closed at {:.2}, above the prior 52-week high of {:.2}", b.close, prior_high),
            ),
            volume_evidence(b.volume, vavg, 1.5),
            Evidence::technical(Stance::Supports, format!("Close {:.2} > SMA50 {:.2}", b.close, sma50)),
        ];
        let ext = (b.close / sma50 - 1.0) * 100.0;
        if ext > 20.0 {
            evidence.push(Evidence::technical(
                Stance::Opposes,
                format!("Price is {:.1}% above SMA50: chasing an extended move", ext),
            ));
        }
        if rsi > 75.0 {
            evidence.push(Evidence::technical(Stance::Opposes, format!("RSI(14) {:.1}: overbought", rsi)));
        }
        Some(Signal {
            symbol: symbol.to_string(),
            setup: self.name().into(),
            as_of: b.date,
            entry,
            stop: entry - 2.0 * atr,
            target: entry + 4.0 * atr,
            max_hold_days: self.max_hold_days(),
            evidence,
        })
    }
}

/// Short-term mean reversion inside a long-term uptrend (the classic RSI(2) pullback).
/// Research: short-horizon reversal (Jegadeesh 1990, Lehmann 1990). Mean reversion is hurt by tight
/// stops, so the stop is a wide catastrophe stop and the real exit is the first close back above SMA5.
pub struct Rsi2Pullback;

impl Setup for Rsi2Pullback {
    fn name(&self) -> &'static str {
        "rsi2-pullback"
    }
    fn description(&self) -> &'static str {
        "Long. Close > SMA200 (long-term uptrend); RSI(2) < 10 (sharp 1-3 day selloff); avg dollar volume >= $20M. \
         Exit on the first close above SMA5. Catastrophe stop = entry - 3*ATR14, cap = entry + 3*ATR14, max hold 10 sessions."
    }
    fn max_hold_days(&self) -> usize {
        10
    }
    fn evaluate(&self, symbol: &str, bars: &[Bar], ind: &Indicators, i: usize) -> Option<Signal> {
        let b = &bars[i];
        let (sma200, sma5, rsi2, atr, vavg) =
            (ind.sma200[i]?, ind.sma5[i]?, ind.rsi2[i]?, ind.atr14[i]?, ind.vol_avg50[i]?);
        let liquid = b.close * vavg >= MIN_DOLLAR_VOLUME;
        if !(b.close > sma200 && rsi2 < 10.0 && b.close < sma5 && liquid) {
            return None;
        }
        let entry = b.close;
        let mut evidence = vec![
            Evidence::technical(Stance::Supports, format!("Long-term uptrend: close {:.2} > SMA200 {:.2}", b.close, sma200)),
            Evidence::technical(
                Stance::Supports,
                format!("RSI(2) {:.1}: short-term oversold; exit planned at the first close above SMA5 ({:.2})", rsi2, sma5),
            ),
        ];
        let cushion = (b.close / sma200 - 1.0) * 100.0;
        if cushion < 3.0 {
            evidence.push(Evidence::technical(
                Stance::Opposes,
                format!("Only {:.1}% above SMA200: the long-term uptrend is close to breaking", cushion),
            ));
        }
        if b.volume > 2.5 * vavg {
            evidence.push(Evidence::technical(
                Stance::Opposes,
                format!("Selloff on {:.1}x average volume: may be news-driven, not noise", b.volume / vavg),
            ));
        }
        Some(Signal {
            symbol: symbol.to_string(),
            setup: self.name().into(),
            as_of: b.date,
            entry,
            stop: entry - 3.0 * atr,
            target: entry + 3.0 * atr,
            max_hold_days: self.max_hold_days(),
            evidence,
        })
    }
    fn exit_on_close(&self, bars: &[Bar], ind: &Indicators, j: usize) -> bool {
        ind.sma5[j].is_some_and(|m| bars[j].close > m)
    }
}

/// Post-announcement drift, price-based proxy: a large gap up on very heavy volume that holds into the close.
/// Research: post-earnings-announcement drift (Ball & Brown 1968; Bernard & Thomas 1989) and
/// earnings-announcement gaps. Most such gaps are earnings or major news; we don't have point-in-time
/// earnings dates yet, so the gap itself is the event.
pub struct GapDrift;

impl Setup for GapDrift {
    fn name(&self) -> &'static str {
        "gap-drift"
    }
    fn description(&self) -> &'static str {
        "Long. Open >= 4% above the prior close; volume >= 3x 50-day average; close >= open (gap held all day); \
         avg dollar volume >= $20M. Stop = prior close (gap fully filled = thesis broken), cap = entry + 8*ATR14, max hold 40 sessions."
    }
    fn max_hold_days(&self) -> usize {
        40
    }
    fn evaluate(&self, symbol: &str, bars: &[Bar], ind: &Indicators, i: usize) -> Option<Signal> {
        if i == 0 {
            return None;
        }
        let (b, prev) = (&bars[i], &bars[i - 1]);
        let (atr, vavg) = (ind.atr14[i]?, ind.vol_avg50[i]?);
        let gap = b.open / prev.close - 1.0;
        let liquid = b.close * vavg >= MIN_DOLLAR_VOLUME;
        if !(gap >= 0.04 && b.volume >= 3.0 * vavg && b.close >= b.open && liquid) {
            return None;
        }
        let entry = b.close;
        let mut evidence = vec![
            Evidence::technical(
                Stance::Supports,
                format!("Gapped up {:.1}% (prior close {:.2} -> open {:.2}) and held: close {:.2}", gap * 100.0, prev.close, b.open, b.close),
            ),
            volume_evidence(b.volume, vavg, 3.0),
            Evidence::technical(
                Stance::Neutral,
                "Gap is treated as an announcement event; check the filings/news evidence for what caused it",
            ),
        ];
        if let Some(sma200) = ind.sma200[i] {
            let above = b.close > sma200;
            evidence.push(Evidence::technical(
                if above { Stance::Supports } else { Stance::Opposes },
                format!("Close {:.2} is {} SMA200 {:.2}", b.close, if above { "above" } else { "below" }, sma200),
            ));
        }
        if (entry - prev.close) / entry > 0.12 {
            evidence.push(Evidence::technical(
                Stance::Opposes,
                format!("Stop at the prior close is {:.1}% away: large risk per share", (entry - prev.close) / entry * 100.0),
            ));
        }
        Some(Signal {
            symbol: symbol.to_string(),
            setup: self.name().into(),
            as_of: b.date,
            entry,
            stop: prev.close,
            target: entry + 8.0 * atr,
            max_hold_days: self.max_hold_days(),
            evidence,
        })
    }
}
