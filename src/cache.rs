//! Local SQLite cache of adjusted daily bars.
//!
//! First run downloads full history; later runs fetch only recent days. Progress is saved per batch,
//! so an interrupted download resumes where it stopped. Because Alpaca bars are split/dividend
//! adjusted *retroactively*, each incremental fetch overlaps the cached tail: if those overlapping
//! closes no longer match (a split or dividend re-adjusted history), that symbol is fully re-downloaded.
use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::data::alpaca::Alpaca;
use crate::models::Bar;

/// Symbols per full-history download batch (saved to disk after each batch).
const BATCH: usize = 50;
/// Overlap re-fetched on incremental updates, in calendar days.
const OVERLAP_DAYS: i64 = 10;
/// Relative close difference treated as a re-adjustment (split/dividend).
const ADJ_TOLERANCE: f64 = 0.005;

pub struct BarCache {
    conn: Connection,
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

impl BarCache {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS bars (
                symbol TEXT NOT NULL,
                date   TEXT NOT NULL,
                open   REAL NOT NULL,
                high   REAL NOT NULL,
                low    REAL NOT NULL,
                close  REAL NOT NULL,
                volume REAL NOT NULL,
                PRIMARY KEY (symbol, date)
            ) WITHOUT ROWID;
            -- Earliest start date we have requested full history from, per symbol.
            -- Lets us tell 'IPO'd later' apart from 'not downloaded far enough back'.
            CREATE TABLE IF NOT EXISTS bar_coverage (
                symbol       TEXT PRIMARY KEY,
                fetched_from TEXT NOT NULL
            );
            -- Unadjusted month-end closes ('YYYY-MM'), for price ratios against as-reported per-share data.
            CREATE TABLE IF NOT EXISTS raw_monthly (
                symbol TEXT NOT NULL,
                month  TEXT NOT NULL,
                close  REAL NOT NULL,
                PRIMARY KEY (symbol, month)
            ) WITHOUT ROWID;",
        )?;
        Ok(Self { conn })
    }

    fn coverage(&self, symbol: &str) -> Result<Option<(NaiveDate, Option<NaiveDate>)>> {
        let from: Option<String> = self
            .conn
            .query_row("SELECT fetched_from FROM bar_coverage WHERE symbol=?1", [symbol], |r| r.get(0))
            .optional()?;
        let Some(from) = from.as_deref().and_then(parse_date) else { return Ok(None) };
        let last: Option<String> =
            self.conn.query_row("SELECT MAX(date) FROM bars WHERE symbol=?1", [symbol], |r| r.get(0))?;
        Ok(Some((from, last.as_deref().and_then(parse_date))))
    }

    fn cached_closes(&self, symbol: &str, from: NaiveDate) -> Result<HashMap<NaiveDate, f64>> {
        let mut stmt = self.conn.prepare("SELECT date, close FROM bars WHERE symbol=?1 AND date>=?2")?;
        let rows = stmt.query_map(params![symbol, from.to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (d, c) = row?;
            if let Some(d) = parse_date(&d) {
                out.insert(d, c);
            }
        }
        Ok(out)
    }

    fn store(&mut self, symbol: &str, bars: &[Bar], replace_all: bool, fetched_from: Option<NaiveDate>) -> Result<()> {
        let tx = self.conn.transaction()?;
        if replace_all {
            tx.execute("DELETE FROM bars WHERE symbol=?1", [symbol])?;
        }
        {
            let mut ins = tx.prepare(
                "INSERT OR REPLACE INTO bars (symbol, date, open, high, low, close, volume)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for b in bars {
                ins.execute(params![symbol, b.date.to_string(), b.open, b.high, b.low, b.close, b.volume])?;
            }
        }
        if let Some(from) = fetched_from {
            tx.execute(
                "INSERT OR REPLACE INTO bar_coverage (symbol, fetched_from) VALUES (?1, ?2)",
                params![symbol, from.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn load(&self, symbol: &str, from: NaiveDate) -> Result<Vec<Bar>> {
        let mut stmt = self.conn.prepare(
            "SELECT date, open, high, low, close, volume FROM bars WHERE symbol=?1 AND date>=?2 ORDER BY date",
        )?;
        let rows = stmt.query_map(params![symbol, from.to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (d, open, high, low, close, volume) = row?;
            if let Some(date) = parse_date(&d) {
                out.push(Bar { date, open, high, low, close, volume });
            }
        }
        Ok(out)
    }

    /// Returns bars from `start` for every symbol, downloading only what the cache is missing.
    pub async fn get(
        &mut self,
        alpaca: &Alpaca,
        symbols: &[String],
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<HashMap<String, Vec<Bar>>> {
        let start_d = start.date_naive();

        // Plan: which symbols need full history, which only an incremental update (grouped by last cached date).
        let mut full: Vec<String> = Vec::new();
        let mut incremental: HashMap<NaiveDate, Vec<String>> = HashMap::new();
        for s in symbols {
            match self.coverage(s)? {
                Some((from, Some(last))) if from <= start_d => incremental.entry(last).or_default().push(s.clone()),
                // Covered far enough back but the API had no bars (bad/delisted ticker): don't retry every run.
                Some((from, None)) if from <= start_d => {}
                _ => full.push(s.clone()),
            }
        }

        // Incremental updates, with re-adjustment detection on the overlap.
        let mut readjusted = Vec::new();
        let n_incr: usize = incremental.values().map(Vec::len).sum();
        if n_incr > 0 {
            eprintln!("Cache: updating {n_incr} symbol(s) with recent bars...");
        }
        for (last, syms) in incremental {
            let from = last - TimeDelta::days(OVERLAP_DAYS);
            let from_ts = from.and_hms_opt(0, 0, 0).unwrap().and_utc();
            for batch in syms.chunks(BATCH * 4) {
                let fresh = alpaca.daily_bars(batch, from_ts, end).await?;
                for s in batch {
                    let Some(new_bars) = fresh.get(s) else { continue };
                    let cached = self.cached_closes(s, from)?;
                    // Skip the last cached day: if it was fetched mid-session it was a partial bar,
                    // and its close legitimately differs from the final one (it gets overwritten below).
                    let mismatch = new_bars.iter().filter(|b| b.date < last).any(|b| {
                        cached.get(&b.date).is_some_and(|&c| ((b.close - c) / c).abs() > ADJ_TOLERANCE)
                    });
                    if mismatch {
                        readjusted.push(s.clone());
                    } else {
                        self.store(s, new_bars, false, None)?;
                    }
                }
            }
        }
        if !readjusted.is_empty() {
            eprintln!(
                "Cache: price history re-adjusted (split/dividend) for {}; re-downloading those.",
                readjusted.join(", ")
            );
            full.extend(readjusted);
        }

        // Full-history downloads, saved batch by batch.
        if !full.is_empty() {
            eprintln!("Cache: downloading full history for {} symbol(s) from {start_d}...", full.len());
        }
        for (i, batch) in full.chunks(BATCH).enumerate() {
            eprintln!(
                "  batch {}/{} ({}..)",
                i + 1,
                full.len().div_ceil(BATCH),
                batch.first().map(String::as_str).unwrap_or("")
            );
            let fetched = alpaca.daily_bars(batch, start, end).await?;
            for s in batch {
                let bars = fetched.get(s).map(Vec::as_slice).unwrap_or(&[]);
                self.store(s, bars, true, Some(start_d))?;
            }
        }

        let mut out = HashMap::new();
        for s in symbols {
            let bars = self.load(s, start_d)?;
            if !bars.is_empty() {
                out.insert(s.clone(), bars);
            }
        }
        Ok(out)
    }
}

/// Month key used by the raw monthly table.
pub fn month_key(d: NaiveDate) -> String {
    d.format("%Y-%m").to_string()
}

impl BarCache {
    /// Unadjusted month-end closes per symbol, keyed 'YYYY-MM'. Downloads only months not cached yet
    /// (the latest cached month is re-fetched, since it may have been incomplete).
    pub async fn raw_monthly_closes(
        &mut self,
        alpaca: &Alpaca,
        symbols: &[String],
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<HashMap<String, HashMap<String, f64>>> {
        let mut missing = Vec::new();
        let mut oldest_last: Option<NaiveDate> = None;
        for s in symbols {
            let last: Option<String> =
                self.conn.query_row("SELECT MAX(month) FROM raw_monthly WHERE symbol=?1", [s], |r| r.get(0))?;
            match last.and_then(|m| NaiveDate::parse_from_str(&format!("{m}-01"), "%Y-%m-%d").ok()) {
                Some(d) => oldest_last = Some(oldest_last.map_or(d, |o| o.min(d))),
                None => missing.push(s.clone()),
            }
        }
        let mut jobs: Vec<(Vec<String>, DateTime<Utc>)> = Vec::new();
        if !missing.is_empty() {
            jobs.push((missing, start));
        }
        if let Some(d) = oldest_last {
            let have: Vec<String> = symbols.iter().filter(|s| !jobs.iter().any(|(m, _)| m.contains(s))).cloned().collect();
            jobs.push((have, d.and_hms_opt(0, 0, 0).unwrap().and_utc()));
        }
        for (syms, from) in jobs {
            if syms.is_empty() || from >= end {
                continue;
            }
            eprintln!("Cache: unadjusted monthly closes for {} symbol(s) from {}...", syms.len(), from.date_naive());
            let fetched = alpaca.monthly_raw_bars(&syms, from, end).await?;
            let tx = self.conn.transaction()?;
            {
                let mut ins = tx.prepare("INSERT OR REPLACE INTO raw_monthly (symbol, month, close) VALUES (?1, ?2, ?3)")?;
                for (s, bars) in &fetched {
                    for b in bars {
                        ins.execute(params![s, month_key(b.date), b.close])?;
                    }
                }
            }
            tx.commit()?;
        }
        let mut out: HashMap<String, HashMap<String, f64>> = HashMap::new();
        let mut stmt = self.conn.prepare("SELECT month, close FROM raw_monthly WHERE symbol=?1")?;
        for s in symbols {
            let rows = stmt.query_map([s], |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?)))?;
            let m: HashMap<String, f64> = rows.collect::<rusqlite::Result<_>>()?;
            if !m.is_empty() {
                out.insert(s.clone(), m);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(d: &str, close: f64) -> Bar {
        Bar { date: parse_date(d).unwrap(), open: close, high: close, low: close, close, volume: 1.0 }
    }

    #[test]
    fn store_load_and_coverage() {
        let mut c = BarCache::open(":memory:").unwrap();
        let from = parse_date("2024-01-01").unwrap();
        assert!(c.coverage("AAPL").unwrap().is_none());

        c.store("AAPL", &[bar("2024-01-02", 10.0), bar("2024-01-03", 11.0)], true, Some(from)).unwrap();
        assert_eq!(c.coverage("AAPL").unwrap(), Some((from, parse_date("2024-01-03"))));

        // Incremental upsert overwrites a (partial) last bar and appends new days.
        c.store("AAPL", &[bar("2024-01-03", 11.5), bar("2024-01-04", 12.0)], false, None).unwrap();
        let loaded = c.load("AAPL", parse_date("2024-01-03").unwrap()).unwrap();
        assert_eq!(loaded.iter().map(|b| b.close).collect::<Vec<_>>(), vec![11.5, 12.0]);

        // Full replace drops old rows.
        c.store("AAPL", &[bar("2024-01-05", 5.0)], true, Some(from)).unwrap();
        assert_eq!(c.load("AAPL", from).unwrap().len(), 1);
    }
}
