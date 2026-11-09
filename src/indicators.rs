//! Technical indicators. Every value at index `i` depends only on data at `<= i`,
//! so precomputing over a full series introduces no look-ahead bias.
use crate::models::Bar;

pub type Series = Vec<Option<f64>>;

pub fn sma(v: &[f64], n: usize) -> Series {
    let mut out = vec![None; v.len()];
    if n == 0 {
        return out;
    }
    let mut sum = 0.0;
    for i in 0..v.len() {
        sum += v[i];
        if i >= n {
            sum -= v[i - n];
        }
        if i + 1 >= n {
            out[i] = Some(sum / n as f64);
        }
    }
    out
}

/// EMA seeded with the SMA of the first `n` values.
pub fn ema(v: &[f64], n: usize) -> Series {
    let mut out = vec![None; v.len()];
    if n == 0 || v.len() < n {
        return out;
    }
    let k = 2.0 / (n as f64 + 1.0);
    let mut prev = v[..n].iter().sum::<f64>() / n as f64;
    out[n - 1] = Some(prev);
    for i in n..v.len() {
        prev = v[i] * k + prev * (1.0 - k);
        out[i] = Some(prev);
    }
    out
}

/// Wilder's RSI.
pub fn rsi(close: &[f64], n: usize) -> Series {
    let mut out = vec![None; close.len()];
    if n == 0 || close.len() <= n {
        return out;
    }
    let (mut gain, mut loss) = (0.0, 0.0);
    for i in 1..=n {
        let d = close[i] - close[i - 1];
        if d > 0.0 { gain += d } else { loss -= d }
    }
    let (mut ag, mut al) = (gain / n as f64, loss / n as f64);
    let value = |ag: f64, al: f64| if al == 0.0 { 100.0 } else { 100.0 - 100.0 / (1.0 + ag / al) };
    out[n] = Some(value(ag, al));
    for i in (n + 1)..close.len() {
        let d = close[i] - close[i - 1];
        let (g, l) = if d > 0.0 { (d, 0.0) } else { (0.0, -d) };
        ag = (ag * (n as f64 - 1.0) + g) / n as f64;
        al = (al * (n as f64 - 1.0) + l) / n as f64;
        out[i] = Some(value(ag, al));
    }
    out
}

/// Wilder's ATR.
pub fn atr(bars: &[Bar], n: usize) -> Series {
    let mut out = vec![None; bars.len()];
    if n == 0 || bars.len() < n {
        return out;
    }
    let tr: Vec<f64> = bars
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if i == 0 {
                b.high - b.low
            } else {
                let pc = bars[i - 1].close;
                (b.high - b.low).max((b.high - pc).abs()).max((b.low - pc).abs())
            }
        })
        .collect();
    let mut prev = tr[..n].iter().sum::<f64>() / n as f64;
    out[n - 1] = Some(prev);
    for i in n..bars.len() {
        prev = (prev * (n as f64 - 1.0) + tr[i]) / n as f64;
        out[i] = Some(prev);
    }
    out
}

/// Highest value in the window of `n` values ending at `i` (inclusive).
pub fn highest(v: &[f64], n: usize) -> Series {
    (0..v.len())
        .map(|i| (n > 0 && i + 1 >= n).then(|| v[i + 1 - n..=i].iter().cloned().fold(f64::MIN, f64::max)))
        .collect()
}

/// All indicators the setups use, computed once per symbol.
pub struct Indicators {
    pub sma5: Series,
    pub sma50: Series,
    pub sma200: Series,
    pub ema21: Series,
    pub rsi14: Series,
    pub rsi2: Series,
    pub atr14: Series,
    pub vol_avg50: Series,
    pub high252: Series,
}

impl Indicators {
    pub fn compute(bars: &[Bar]) -> Self {
        let close: Vec<f64> = bars.iter().map(|b| b.close).collect();
        let high: Vec<f64> = bars.iter().map(|b| b.high).collect();
        let vol: Vec<f64> = bars.iter().map(|b| b.volume).collect();
        Self {
            sma5: sma(&close, 5),
            sma50: sma(&close, 50),
            sma200: sma(&close, 200),
            ema21: ema(&close, 21),
            rsi14: rsi(&close, 14),
            rsi2: rsi(&close, 2),
            atr14: atr(bars, 14),
            vol_avg50: sma(&vol, 50),
            high252: highest(&high, 252),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn sma_window() {
        let s = sma(&[1.0, 2.0, 3.0, 4.0, 5.0], 3);
        assert_eq!(s[1], None);
        assert!(approx(s[2].unwrap(), 2.0));
        assert!(approx(s[4].unwrap(), 4.0));
    }

    #[test]
    fn ema_of_constant_is_constant() {
        let s = ema(&[7.0; 30], 10);
        assert!(s[9..].iter().all(|x| approx(x.unwrap(), 7.0)));
    }

    #[test]
    fn rsi_extremes() {
        let up: Vec<f64> = (0..30).map(|i| i as f64).collect();
        assert!(approx(rsi(&up, 14)[29].unwrap(), 100.0));
        let down: Vec<f64> = (0..30).map(|i| 100.0 - i as f64).collect();
        assert!(approx(rsi(&down, 14)[29].unwrap(), 0.0));
    }

    #[test]
    fn atr_of_constant_range() {
        let d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let bars: Vec<Bar> = (0..30)
            .map(|_| Bar { date: d, open: 10.0, high: 11.0, low: 9.0, close: 10.0, volume: 1.0 })
            .collect();
        assert!(approx(atr(&bars, 14)[29].unwrap(), 2.0));
    }

    #[test]
    fn highest_window() {
        let h = highest(&[1.0, 5.0, 2.0, 3.0], 2);
        assert_eq!(h[0], None);
        assert!(approx(h[2].unwrap(), 5.0));
        assert!(approx(h[3].unwrap(), 3.0));
    }
}
