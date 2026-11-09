//! SQLite ledger: every signal is recorded and later scored as a paper trade.
use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::Signal;

pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct StoredSignal {
    pub id: i64,
    pub symbol: String,
    pub setup: String,
    pub as_of: NaiveDate,
    /// Planned entry (signal close). Kept for the trade-journal/web app to compare against fills.
    #[allow(dead_code)]
    pub entry: f64,
    pub stop: f64,
    pub target: f64,
    pub max_hold: usize,
    pub status: String,
    pub fill_date: Option<NaiveDate>,
    pub fill_price: Option<f64>,
    pub exit_date: Option<NaiveDate>,
    pub exit_price: Option<f64>,
    pub ret: Option<f64>,
}

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS signals (
                id          INTEGER PRIMARY KEY,
                symbol      TEXT NOT NULL,
                setup       TEXT NOT NULL,
                as_of       TEXT NOT NULL,
                entry       REAL NOT NULL,
                stop        REAL NOT NULL,
                target      REAL NOT NULL,
                max_hold    INTEGER NOT NULL,
                evidence    TEXT NOT NULL,
                report_path TEXT,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                -- open | filled | stop | target | time | skipped_gap
                status      TEXT NOT NULL DEFAULT 'open',
                fill_date   TEXT,
                fill_price  REAL,
                exit_date   TEXT,
                exit_price  REAL,
                ret         REAL,
                UNIQUE(symbol, setup, as_of)
            );",
        )?;
        Ok(Self { conn })
    }

    pub fn exists(&self, symbol: &str, setup: &str, as_of: NaiveDate) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM signals WHERE symbol=?1 AND setup=?2 AND as_of=?3",
                params![symbol, setup, as_of.to_string()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn insert(&self, s: &Signal, report_path: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO signals (symbol, setup, as_of, entry, stop, target, max_hold, evidence, report_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                s.symbol,
                s.setup,
                s.as_of.to_string(),
                s.entry,
                s.stop,
                s.target,
                s.max_hold_days as i64,
                serde_json::to_string(&s.evidence)?,
                report_path
            ],
        )?;
        Ok(())
    }

    pub fn all(&self) -> Result<Vec<StoredSignal>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, symbol, setup, as_of, entry, stop, target, max_hold, status,
                    fill_date, fill_price, exit_date, exit_price, ret
             FROM signals ORDER BY as_of, symbol",
        )?;
        let date = |s: Option<String>| s.and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok());
        let rows = stmt.query_map([], |r| {
            Ok(StoredSignal {
                id: r.get(0)?,
                symbol: r.get(1)?,
                setup: r.get(2)?,
                as_of: date(r.get(3)?).unwrap_or_default(),
                entry: r.get(4)?,
                stop: r.get(5)?,
                target: r.get(6)?,
                max_hold: r.get::<_, i64>(7)? as usize,
                status: r.get(8)?,
                fill_date: date(r.get(9)?),
                fill_price: r.get(10)?,
                exit_date: date(r.get(11)?),
                exit_price: r.get(12)?,
                ret: r.get(13)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn update(&self, s: &StoredSignal) -> Result<()> {
        self.conn.execute(
            "UPDATE signals SET status=?2, fill_date=?3, fill_price=?4, exit_date=?5, exit_price=?6, ret=?7 WHERE id=?1",
            params![
                s.id,
                s.status,
                s.fill_date.map(|d| d.to_string()),
                s.fill_price,
                s.exit_date.map(|d| d.to_string()),
                s.exit_price,
                s.ret
            ],
        )?;
        Ok(())
    }
}
