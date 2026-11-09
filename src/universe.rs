//! Index constituent lists, written as plain watchlist files.
//!
//! Note: these are *today's* members. Backtesting on them still carries survivorship bias
//! (companies removed after doing badly are missing), but far less than a hand-picked list.
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::{NaiveDate, Utc};

/// Community-maintained CSV mirror of the Wikipedia S&P 500 table (first column = ticker).
const SP500_CSV: &str = "https://raw.githubusercontent.com/datasets/s-and-p-500-companies/main/data/constituents.csv";

pub async fn download(index: &str, out: &str) -> Result<()> {
    let (url, label) = match index.to_lowercase().as_str() {
        "sp500" | "s&p500" | "spx" => (SP500_CSV, "S&P 500"),
        other => bail!("unknown index '{other}' (supported: sp500)"),
    };
    eprintln!("Downloading {label} constituents...");
    let client = reqwest::Client::builder().timeout(Duration::from_secs(120)).build()?;
    let mut attempt = 0;
    let text = loop {
        attempt += 1;
        let res = async { client.get(url).send().await?.error_for_status()?.text().await }.await;
        match res {
            Ok(t) => break t,
            Err(e) if attempt < 5 => {
                let wait = 2 + attempt * 3;
                eprintln!("  retrying in {wait}s (attempt {}/5): {e}", attempt + 1);
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
            Err(e) => return Err(e).context("downloading constituent list"),
        }
    };

    let tickers = parse_constituents(&text)?;
    // Sanity check: a broken download or changed format must not silently produce a tiny universe.
    if !(450..=550).contains(&tickers.len()) {
        bail!("expected ~500 {label} tickers, got {} -- source format may have changed", tickers.len());
    }

    let mut body = format!(
        "# {label} constituents ({} tickers), downloaded {} from\n# {url}\n\
         # Today's members, with the date each joined the index (backtests only trade a stock after that date).\n\
         # Stocks removed from the index since are still missing: some survivorship bias remains.\n",
        tickers.len(),
        Utc::now().date_naive()
    );
    let dated = tickers.iter().filter(|(_, d)| d.is_some()).count();
    for (t, d) in &tickers {
        match d {
            Some(d) => body.push_str(&format!("{t} {d}\n")),
            None => body.push_str(&format!("{t}\n")),
        }
    }
    if let Some(dir) = std::path::Path::new(out).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out, body).with_context(|| format!("writing {out}"))?;
    println!("Wrote {} tickers to {out} ({dated} with an index-add date)", tickers.len());
    Ok(())
}

/// Splits one CSV line, honouring double quotes (company names contain commas).
fn split_csv(line: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// (ticker, date added) pairs, tickers normalised to Alpaca's format (BRK-B -> BRK.B).
fn parse_constituents(csv: &str) -> Result<Vec<(String, Option<NaiveDate>)>> {
    let mut lines = csv.lines();
    let header = split_csv(lines.next().unwrap_or_default());
    let col = |name: &str| header.iter().position(|h| h.trim().eq_ignore_ascii_case(name));
    let sym_col = col("Symbol").unwrap_or(0);
    let date_col = col("Date added");
    if date_col.is_none() {
        eprintln!("warning: no 'Date added' column; backtests will treat every stock as a member throughout");
    }
    let mut out: Vec<(String, Option<NaiveDate>)> = lines
        .filter_map(|l| {
            let f = split_csv(l);
            let t = f.get(sym_col)?.trim().to_uppercase().replace('-', ".");
            if t.is_empty() || !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
                return None;
            }
            let d = date_col
                .and_then(|c| f.get(c))
                .and_then(|s| s.trim().get(..10))
                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
            Some((t, d))
        })
        .collect();
    out.sort();
    out.dedup_by(|a, b| a.0 == b.0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_csv_with_dates() {
        let csv = "Symbol,Security,GICS Sector,Date added
\
                   MMM,3M,Industrials,1957-03-04
\
                   \"BRK-B\",\"Berkshire Hathaway, Inc.\",Financials,2010-02-16
\
                   aapl,Apple Inc.,IT,
";
        let d = |s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
        assert_eq!(
            parse_constituents(csv).unwrap(),
            vec![("AAPL".into(), None), ("BRK.B".into(), d("2010-02-16")), ("MMM".into(), d("1957-03-04"))]
        );
    }
}
