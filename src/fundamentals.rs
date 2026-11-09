//! Point-in-time fundamentals from SEC XBRL (companyconcept API), cached in SQLite.
//!
//! Every value keeps the date it was filed, and every calculation takes an `as_of` date and ignores
//! anything filed later. So a backtest only uses numbers once they were public.
use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use chrono::{NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::data::sec::Sec;

pub const EPS_DILUTED: &str = "EarningsPerShareDiluted";
pub const EPS_BASIC: &str = "EarningsPerShareBasic";
pub const OCF: &str = "NetCashProvidedByUsedInOperatingActivities";
pub const ASSETS: &str = "Assets";
const CONCEPTS: &[&str] = &[EPS_DILUTED, EPS_BASIC, OCF, ASSETS];

/// A 10-Q/10-K filing is due roughly every quarter; until ~75 days after the last one, nothing new is expected.
const DAYS_UNTIL_NEXT_FILING: i64 = 75;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fact {
    /// None for point-in-time (balance sheet) values; Some for values over a period.
    pub start: Option<NaiveDate>,
    pub end: NaiveDate,
    pub val: f64,
    pub filed: NaiveDate,
}

#[derive(Debug, Default, Clone)]
pub struct Facts {
    by_concept: HashMap<String, Vec<Fact>>,
}

impl Facts {
    pub fn get(&self, concept: &str) -> &[Fact] {
        self.by_concept.get(concept).map(Vec::as_slice).unwrap_or(&[])
    }
    /// Diluted EPS, falling back to basic for companies that only report basic.
    pub fn eps(&self) -> &[Fact] {
        let d = self.get(EPS_DILUTED);
        if d.is_empty() {
            self.get(EPS_BASIC)
        } else {
            d
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quarter {
    pub end: NaiveDate,
    pub val: f64,
    /// When this quarter's number first became public (its first 10-Q/10-K).
    pub first_filed: NaiveDate,
}

fn days(a: NaiveDate, b: NaiveDate) -> i64 {
    (b - a).num_days()
}

const QUARTER_DAYS: std::ops::RangeInclusive<i64> = 80..=100;
const YEAR_DAYS: std::ops::RangeInclusive<i64> = 350..=380;

/// Discrete quarterly values of a period concept, as known on `as_of`.
/// Companies report Q1-Q3 as 3-month and/or year-to-date figures, and Q4 only inside the annual
/// figure, so quarters are also derived as differences of consecutive year-to-date values
/// (6M - 3M, 9M - 6M, 12M - 9M). Where a value was later restated, the latest one filed by `as_of` wins.
pub fn quarterly(facts: &[Fact], as_of: NaiveDate) -> Vec<Quarter> {
    // (start, end) -> (latest value known by as_of, its filing date, first filing date)
    let mut per: HashMap<(NaiveDate, NaiveDate), (f64, NaiveDate, NaiveDate)> = HashMap::new();
    for f in facts.iter().filter(|f| f.filed <= as_of) {
        let Some(s) = f.start else { continue };
        let e = per.entry((s, f.end)).or_insert((f.val, f.filed, f.filed));
        if f.filed >= e.1 {
            e.0 = f.val;
            e.1 = f.filed;
        }
        e.2 = e.2.min(f.filed);
    }
    let mut out: BTreeMap<NaiveDate, Quarter> = BTreeMap::new();
    let mut by_start: HashMap<NaiveDate, Vec<(NaiveDate, f64, NaiveDate)>> = HashMap::new();
    for (&(s, e), &(v, _, first)) in &per {
        if QUARTER_DAYS.contains(&days(s, e)) {
            out.insert(e, Quarter { end: e, val: v, first_filed: first });
        }
        by_start.entry(s).or_default().push((e, v, first));
    }
    for (_, mut cum) in by_start {
        cum.sort_by_key(|x| x.0);
        for w in cum.windows(2) {
            let ((ea, va, _), (eb, vb, fb)) = (w[0], w[1]);
            if QUARTER_DAYS.contains(&days(ea, eb)) {
                out.entry(eb).or_insert(Quarter { end: eb, val: vb - va, first_filed: fb });
            }
        }
    }
    out.into_values().collect()
}

/// Sum of the last four quarters known on `as_of` (None if stale or if quarters are missing).
pub fn ttm(facts: &[Fact], as_of: NaiveDate) -> Option<f64> {
    let q = quarterly(facts, as_of);
    let last4 = &q[q.len().checked_sub(4)?..];
    let (first, last) = (last4[0].end, last4[3].end);
    (days(last, as_of) <= 200 && (250..=300).contains(&days(first, last))).then(|| last4.iter().map(|x| x.val).sum())
}

/// Most recent balance-sheet value known on `as_of` (None if older than ~2 quarters).
pub fn latest_instant(facts: &[Fact], as_of: NaiveDate) -> Option<f64> {
    facts
        .iter()
        .filter(|f| f.start.is_none() && f.filed <= as_of)
        .max_by_key(|f| (f.end, f.filed))
        .filter(|f| days(f.end, as_of) <= 200)
        .map(|f| f.val)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surprise {
    pub end: NaiveDate,
    pub first_filed: NaiveDate,
    pub eps: f64,
    pub eps_year_ago: f64,
    /// Standardized unexpected earnings.
    pub sue: f64,
}

/// Standardized unexpected earnings for each quarter, using only what was known when it was first filed.
/// Seasonal random walk (Foster 1977; Bernard & Thomas 1989): the expected EPS is the same quarter a year
/// earlier plus the average year-over-year change; the surprise is scaled by the standard deviation of
/// that change over the previous (up to) 8 quarters.
pub fn surprises(eps: &[Fact]) -> Vec<Surprise> {
    let mut out = Vec::new();
    for q in quarterly(eps, NaiveDate::MAX) {
        let known = quarterly(eps, q.first_filed);
        let year_ago = |k: &Quarter| known.iter().find(|p| YEAR_DAYS.contains(&days(p.end, k.end))).map(|p| p.val);
        let Some(cur) = known.iter().find(|k| k.end == q.end) else { continue };
        let Some(prior) = year_ago(cur) else { continue };
        let hist: Vec<f64> =
            known.iter().filter(|k| k.end < q.end).filter_map(|k| Some(k.val - year_ago(k)?)).collect();
        let hist = &hist[hist.len().saturating_sub(8)..];
        if hist.len() < 4 {
            continue;
        }
        let n = hist.len() as f64;
        let mean = hist.iter().sum::<f64>() / n;
        let sd = (hist.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt().max(0.01);
        out.push(Surprise {
            end: q.end,
            first_filed: q.first_filed,
            eps: cur.val,
            eps_year_ago: prior,
            sue: (cur.val - prior - mean) / sd,
        });
    }
    out
}

pub struct FactStore {
    conn: Connection,
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

impl FactStore {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS facts (
                symbol  TEXT NOT NULL,
                concept TEXT NOT NULL,
                start   TEXT NOT NULL,  -- '' for balance-sheet values
                end     TEXT NOT NULL,
                filed   TEXT NOT NULL,
                val     REAL NOT NULL,
                PRIMARY KEY (symbol, concept, start, end, filed)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS facts_fetched (
                symbol     TEXT PRIMARY KEY,
                fetched    TEXT NOT NULL,
                last_filed TEXT
            );",
        )?;
        Ok(Self { conn })
    }

    /// Downloads facts for symbols never fetched, or (when `max_age_days` has passed) for symbols
    /// whose next quarterly filing is due. Failures are reported and retried on the next run.
    pub async fn refresh(&mut self, sec: &Sec, symbols: &[String], max_age_days: i64) -> Result<()> {
        let today = Utc::now().date_naive();
        let mut todo = Vec::new();
        for s in symbols {
            let row: Option<(String, Option<String>)> = self
                .conn
                .query_row("SELECT fetched, last_filed FROM facts_fetched WHERE symbol=?1", [s], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            let stale = match row {
                None => true,
                Some((fetched, last)) => {
                    let old = parse_date(&fetched).is_none_or(|f| days(f, today) > max_age_days);
                    let due = last.as_deref().and_then(parse_date).is_none_or(|l| days(l, today) > DAYS_UNTIL_NEXT_FILING);
                    old && due
                }
            };
            if stale && sec.has_cik(s) {
                todo.push(s.clone());
            }
        }
        if todo.is_empty() {
            return Ok(());
        }
        eprintln!(
            "SEC fundamentals: downloading EPS, cash flow and assets for {} symbol(s) (~{} min)...",
            todo.len(),
            (todo.len() * 3) / 300 + 1
        );
        let mut failed = Vec::new();
        for (i, s) in todo.iter().enumerate() {
            eprint!("\r  {}/{} {:<6}   ", i + 1, todo.len(), s);
            let mut facts: Vec<(&str, Fact)> = Vec::new();
            let mut ok = true;
            for &c in CONCEPTS {
                // Basic EPS is only a fallback for companies that don't report diluted EPS.
                if c == EPS_BASIC && facts.iter().any(|(k, _)| *k == EPS_DILUTED) {
                    continue;
                }
                match sec.concept(s, c).await {
                    Ok(Some(v)) => facts.extend(v.into_iter().map(|f| (c, f))),
                    Ok(None) => {}
                    Err(e) => {
                        eprintln!("\n  {s} {c}: {e:#}");
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                self.store(s, &facts, today)?;
            } else {
                failed.push(s.clone());
            }
        }
        eprintln!();
        if !failed.is_empty() {
            eprintln!("  {} symbol(s) failed and will be retried next run: {}", failed.len(), failed.join(", "));
        }
        Ok(())
    }

    fn store(&mut self, symbol: &str, facts: &[(&str, Fact)], today: NaiveDate) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM facts WHERE symbol=?1", [symbol])?;
        {
            let mut ins = tx.prepare(
                "INSERT OR REPLACE INTO facts (symbol, concept, start, end, filed, val) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for (c, f) in facts {
                let start = f.start.map(|d| d.to_string()).unwrap_or_default();
                ins.execute(params![symbol, c, start, f.end.to_string(), f.filed.to_string(), f.val])?;
            }
        }
        let last = facts.iter().map(|(_, f)| f.filed).max().map(|d| d.to_string());
        tx.execute(
            "INSERT OR REPLACE INTO facts_fetched (symbol, fetched, last_filed) VALUES (?1, ?2, ?3)",
            params![symbol, today.to_string(), last],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn load_all(&self, symbols: &[String]) -> Result<HashMap<String, Facts>> {
        let mut stmt = self.conn.prepare("SELECT concept, start, end, filed, val FROM facts WHERE symbol=?1")?;
        let mut out = HashMap::new();
        for s in symbols {
            let mut f = Facts::default();
            let rows = stmt.query_map([s], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get(4)?))
            })?;
            for row in rows {
                let (c, start, end, filed, val) = row?;
                let (Some(end), Some(filed)) = (parse_date(&end), parse_date(&filed)) else { continue };
                f.by_concept.entry(c).or_default().push(Fact { start: parse_date(&start), end, val, filed });
            }
            if !f.by_concept.is_empty() {
                out.insert(s.clone(), f);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        parse_date(s).unwrap()
    }
    fn dur(start: &str, end: &str, val: f64, filed: &str) -> Fact {
        Fact { start: Some(d(start)), end: d(end), val, filed: d(filed) }
    }

    /// One fiscal year reported the usual way: Q1 3M; Q2 and Q3 as 3M + YTD; Q4 only inside the 12M figure.
    fn year(y: i32, q: [f64; 4]) -> Vec<Fact> {
        let s = format!("{y}-01-01");
        let f = |m: &str| format!("{y}-{m}");
        let next = format!("{}-02-15", y + 1);
        vec![
            dur(&s, &f("03-31"), q[0], &f("05-01")),
            dur(&f("04-01"), &f("06-30"), q[1], &f("08-01")),
            dur(&s, &f("06-30"), q[0] + q[1], &f("08-01")),
            dur(&f("07-01"), &f("09-30"), q[2], &f("11-01")),
            dur(&s, &f("09-30"), q[0] + q[1] + q[2], &f("11-01")),
            dur(&s, &f("12-31"), q.iter().sum(), &next),
        ]
    }

    #[test]
    fn derives_q4_and_respects_as_of() {
        let facts = year(2023, [1.0, 2.0, 3.0, 4.0]);
        let q = quarterly(&facts, NaiveDate::MAX);
        assert_eq!(q.iter().map(|x| x.val).collect::<Vec<_>>(), vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(q[3].first_filed, d("2024-02-15"));
        // Before the 10-K is filed, Q4 is unknown.
        assert_eq!(quarterly(&facts, d("2024-01-31")).len(), 3);
        assert_eq!(ttm(&facts, d("2024-03-01")), Some(10.0));
        assert_eq!(ttm(&facts, d("2024-01-31")), None);
    }

    #[test]
    fn surprise_uses_seasonal_change() {
        // Steady +0.1/quarter YoY growth for three years, then a big jump.
        let mut facts = Vec::new();
        for (i, y) in (2020..2023).enumerate() {
            let b = i as f64 * 0.4;
            facts.extend(year(y, [1.0 + b, 1.1 + b, 1.2 + b, 1.3 + b]));
        }
        facts.extend(year(2023, [2.0 + 1.2, 1.1 + 1.2, 1.2 + 1.2, 1.3 + 1.2]));
        let s = surprises(&facts);
        let q1_2023 = s.iter().find(|x| x.end == d("2023-03-31")).unwrap();
        assert!(q1_2023.sue > 10.0, "sue {}", q1_2023.sue);
        let q4_2022 = s.iter().find(|x| x.end == d("2022-12-31")).unwrap();
        assert!(q4_2022.sue.abs() < 1.0);
    }

    #[test]
    fn instant_is_point_in_time() {
        let f = vec![
            Fact { start: None, end: d("2024-03-31"), val: 100.0, filed: d("2024-05-01") },
            Fact { start: None, end: d("2024-06-30"), val: 110.0, filed: d("2024-08-01") },
        ];
        assert_eq!(latest_instant(&f, d("2024-07-15")), Some(100.0));
        assert_eq!(latest_instant(&f, d("2024-08-02")), Some(110.0));
    }
}
