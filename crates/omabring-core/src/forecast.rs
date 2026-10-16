use chrono::{Duration, NaiveDate};
use serde::Serialize;

use crate::depts::classify_dept;
use crate::error::Result;
use crate::Db;

pub const MIN_BUYS: i32 = 4;
pub const MAX_INTERVAL_DAYS: f64 = 45.0;
pub const MAX_CV: f64 = 0.55;
pub const EWMA_ALPHA: f64 = 0.4;
pub const LAPSED_FACTOR: f64 = 2.0;
pub const MAX_LAST_AGE_DAYS: i64 = 90;

const VOTE_FIRST_UP: i32 = 40;
const VOTE_STACK: i32 = 12;
const VOTE_FLOOR: i32 = 55;
const VOTE_DOWN: f64 = 0.2;
const VOTE_PUSH_MAX: i32 = 50;

const SKIP_NAME: &[&str] = &[
    "plastic bag",
    "shopping bag",
    "plast shopping",
    "t shirt hndl bag",
    "delivery fee",
    "service fee",
];

#[derive(Clone, Debug)]
pub struct BuyRec {
    pub key: String,
    pub receipt_name: String,
    pub official_name: String,
    pub barcode: String,
    pub dates: Vec<NaiveDate>,
    pub qtys: Vec<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ForecastRow {
    pub key: String,
    pub name: String,
    pub receipt_name: String,
    pub official_name: String,
    pub barcode: String,
    pub dept: String,
    pub times_bought: usize,
    pub first_buy: String,
    pub last_buy: String,
    pub days_since: Option<i64>,
    pub mean_interval_days: Option<f64>,
    pub std_interval_days: Option<f64>,
    pub weighted_interval_days: Option<f64>,
    pub cv: Option<f64>,
    pub interval_days: f64,
    pub next_due: String,
    pub due_in_days: Option<i64>,
    pub score: i32,
    pub reason: String,
    pub include: bool,
    pub status: String,
    pub frequency: String,
    pub push: i32,
    pub likely_push: i32,
    pub vote: String,
    pub likely_vote: String,
    pub in_horizon: bool,
}

pub fn ewma(values: &[f64], alpha: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut acc = values[0];
    for x in &values[1..] {
        acc = alpha * x + (1.0 - alpha) * acc;
    }
    acc
}

pub fn mean_std(values: &[f64]) -> (f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let n = values.len() as f64;
    let mu = values.iter().sum::<f64>() / n;
    let var = values.iter().map(|x| (x - mu).powi(2)).sum::<f64>() / n;
    (mu, var.sqrt())
}

pub fn successive_intervals(days: &[NaiveDate]) -> Vec<i64> {
    let mut ordered = days.to_vec();
    ordered.sort();
    ordered.dedup();
    let mut out = Vec::new();
    for w in ordered.windows(2) {
        let gap = (w[1] - w[0]).num_days();
        if gap > 0 {
            out.push(gap);
        }
    }
    out
}

fn pack_reason(name: &str) -> Option<&'static str> {
    let blob = name.to_ascii_lowercase();
    let small = ["500 ml", "500ml", "50 cl", "50cl", "0.5 l", "0.5l", "330 ml", "330ml"]
        .iter()
        .any(|s| blob.contains(s))
        || regex_small(&blob);
    let water = ["san pel", "acqua", "water", "sparkling", "oasis", "perrier", "pellegrino"]
        .iter()
        .any(|s| blob.contains(s));
    let multi = blob.contains("pack")
        || blob.contains("case")
        || blob.contains(" x")
        || blob.contains("x ")
        || blob.contains("24 x")
        || blob.contains("12 x");
    if small && water && !multi {
        Some("occasional_small_pack")
    } else {
        None
    }
}

fn regex_small(blob: &str) -> bool {
    blob.contains("1 can") || blob.contains("1 bottle") || blob.contains("singol")
}

pub fn parse_push(raw: &str) -> i32 {
    let bit = raw.trim().to_ascii_lowercase();
    if bit.is_empty() || bit == "0" {
        return 0;
    }
    let n = if bit == "up" || bit == "+" {
        1
    } else if bit == "down" || bit == "-" {
        -1
    } else {
        bit.parse::<i32>().unwrap_or(0)
    };
    n.clamp(-VOTE_PUSH_MAX, VOTE_PUSH_MAX)
}

pub fn vote_label(push: i32) -> &'static str {
    if push > 0 {
        "up"
    } else if push < 0 {
        "down"
    } else {
        ""
    }
}

pub fn apply_vote(row: &mut ForecastRow, vote: &str) {
    let push = parse_push(vote);
    row.push = push;
    row.likely_push = push;
    row.vote = vote_label(push).into();
    row.likely_vote = row.vote.clone();
    let score = row.score;
    if push > 0 {
        let bumped = score + VOTE_FIRST_UP + VOTE_STACK * (push - 1);
        let floor = VOTE_FLOOR + VOTE_STACK * (push - 1);
        row.score = bumped.max(floor).min(100);
        if row.reason != "noise" {
            row.include = true;
        }
    } else if push < 0 {
        let factor = VOTE_DOWN.powi(-push);
        row.score = (score as f64 * factor).round() as i32;
        row.include = false;
    }
}

fn likely_score(
    times: usize,
    gaps: usize,
    weighted: f64,
    cv: f64,
    elapsed: Option<i64>,
    reason: &str,
    excluded: bool,
) -> i32 {
    if excluded || reason == "noise" {
        return 0;
    }
    if times < 2 || gaps < 1 || weighted == 0.0 {
        return 0;
    }
    let ratio = (elapsed.unwrap_or(0) as f64) / weighted.max(1.0);
    let mut urgency = 100.0 / (1.0 + (-2.8 * (ratio - 0.85)).exp());
    urgency *= (28.0 / weighted.max(1.0)).min(1.0);
    let regularity = 1.0 / (1.0 + cv.max(0.0));
    let support = (1.0 + times as f64).ln() / 13.0_f64.ln();
    let support = support.min(1.0);
    let mut pen = 1.0;
    match reason {
        "occasional_small_pack" => pen *= 0.18,
        "interval_too_long" => pen *= (45.0 / weighted).clamp(0.28, 1.0),
        "irregular" => pen *= 0.55,
        "lapsed" => {
            let extra = (elapsed.unwrap_or(0) - 90).max(0) as f64;
            pen *= (0.7 - extra / 240.0).max(0.2);
        }
        "too_few_buys" => pen *= 0.45,
        _ => {}
    }
    let raw = urgency * (0.4 + 0.6 * regularity) * (0.45 + 0.55 * support) * pen;
    raw.clamp(0.0, 100.0).round() as i32
}

pub fn classify(rec: &BuyRec, today: NaiveDate, excluded: bool, vote: &str) -> ForecastRow {
    let official = rec.official_name.clone();
    let receipt = rec.receipt_name.clone();
    let name = if official.is_empty() { receipt.clone() } else { official.clone() };
    let mut unique = rec.dates.clone();
    unique.sort();
    unique.dedup();
    let times = unique.len();
    let last = unique.last().copied();
    let first = unique.first().copied();
    let gaps = successive_intervals(&unique);
    let gapf: Vec<f64> = gaps.iter().map(|g| *g as f64).collect();
    let (mu, sd) = mean_std(&gapf);
    let weighted = if gaps.is_empty() { 0.0 } else { ewma(&gapf, EWMA_ALPHA) };
    let cv = if mu > 0.0 {
        sd / mu
    } else if times >= 2 {
        10.0
    } else {
        0.0
    };
    let elapsed = last.map(|d| (today - d).num_days());
    let (due_in, next_due) = if let (Some(last), true) = (last, weighted > 0.0) {
        let nxt = last + Duration::days(weighted.round().max(1.0) as i64);
        (Some((nxt - today).num_days()), nxt.format("%Y-%m-%d").to_string())
    } else {
        (None, String::new())
    };
    let pack = pack_reason(&name).or_else(|| pack_reason(&receipt));
    let mut reason;
    let mut include = false;
    if excluded {
        reason = "excluded".into();
    } else if SKIP_NAME.iter().any(|b| name.to_ascii_lowercase().contains(b) || receipt.to_ascii_lowercase().contains(b))
    {
        reason = "noise".into();
    } else if times < 2 || gaps.is_empty() {
        reason = "too_few_buys".into();
    } else if let Some(p) = pack {
        reason = p.into();
    } else if weighted > MAX_INTERVAL_DAYS {
        reason = "interval_too_long".into();
    } else if cv > MAX_CV {
        reason = "irregular".into();
    } else if last.is_some()
        && elapsed.is_some()
        && (elapsed.unwrap() > MAX_LAST_AGE_DAYS || elapsed.unwrap() as f64 > LAPSED_FACTOR * weighted.max(1.0))
    {
        reason = "lapsed".into();
    } else {
        include = true;
        reason = match due_in {
            Some(d) if d < 0 => "regular_overdue",
            Some(0) => "regular_due_today",
            Some(1) => "regular_due_tomorrow",
            _ => "regular_upcoming",
        }
        .into();
    }
    let score = likely_score(times, gaps.len(), weighted, cv, elapsed, &reason, excluded);
    let status = match reason.as_str() {
        "regular_overdue" => "overdue",
        "regular_due_today" => "due_today",
        "regular_due_tomorrow" => "due_tomorrow",
        "regular_upcoming" => "upcoming",
        "lapsed" => "lapsed",
        "excluded" => "excluded",
        _ => "unknown",
    }
    .to_string();
    let mut row = ForecastRow {
        key: rec.key.clone(),
        name,
        receipt_name: receipt,
        official_name: official,
        barcode: rec.barcode.clone(),
        dept: classify_dept(&rec.receipt_name, &rec.official_name),
        times_bought: times,
        first_buy: first.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default(),
        last_buy: last.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default(),
        days_since: elapsed,
        mean_interval_days: if gaps.is_empty() { None } else { Some((mu * 10.0).round() / 10.0) },
        std_interval_days: if gaps.is_empty() { None } else { Some((sd * 10.0).round() / 10.0) },
        weighted_interval_days: if gaps.is_empty() {
            None
        } else {
            Some((weighted * 10.0).round() / 10.0)
        },
        cv: if gaps.is_empty() { None } else { Some((cv * 100.0).round() / 100.0) },
        interval_days: if gaps.is_empty() { 1e9 } else { (weighted * 10.0).round() / 10.0 },
        next_due,
        due_in_days: due_in,
        score,
        reason,
        include,
        status,
        frequency: if weighted > 0.0 {
            format!("every {weighted:.1} days")
        } else {
            "—".into()
        },
        push: 0,
        likely_push: 0,
        vote: String::new(),
        likely_vote: String::new(),
        in_horizon: false,
    };
    apply_vote(&mut row, vote);
    row
}

impl Db {
    pub fn buy_history(&self, user_id: i64, until: NaiveDate) -> Result<Vec<BuyRec>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT it.product_key,
                    MAX(it.name) AS receipt_name,
                    MAX(NULLIF(pm.official_name,'')) AS official_name,
                    MAX(it.barcode) AS barcode,
                    substr(i.invoice_date,1,10) AS day,
                    SUM(it.qty) AS qty
             FROM invoice_items it
             JOIN invoices i ON i.id = it.invoice_id
             LEFT JOIN product_meta pm ON pm.product_key = it.product_key
             WHERE i.user_id=?1 AND substr(i.invoice_date,1,10)<=?2
             GROUP BY it.product_key, i.id
             ORDER BY it.product_key, i.invoice_date",
        )?;
        let rows = stmt.query_map(rusqlite::params![user_id, until.format("%Y-%m-%d").to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1).unwrap_or_default(),
                r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                r.get::<_, f64>(5).unwrap_or(0.0),
            ))
        })?;
        let mut out: Vec<BuyRec> = Vec::new();
        for row in rows {
            let (key, receipt, official, barcode, day, qty) = row?;
            if out.last().map(|r| r.key.as_str()) != Some(key.as_str()) {
                out.push(BuyRec {
                    key: key.clone(),
                    receipt_name: receipt.clone(),
                    official_name: official.clone(),
                    barcode: barcode.clone(),
                    dates: Vec::new(),
                    qtys: Vec::new(),
                });
            }
            let rec = out.last_mut().unwrap();
            if !official.is_empty() {
                rec.official_name = official;
            }
            if let Ok(d) = NaiveDate::parse_from_str(&day, "%Y-%m-%d") {
                rec.dates.push(d);
                rec.qtys.push(qty);
            }
            let _ = (receipt, barcode);
        }
        Ok(out)
    }

    pub fn load_exclusions(&self, user_id: i64) -> Result<Vec<String>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare("SELECT product_key FROM forecast_exclusions WHERE user_id=?")?;
        let rows = stmt.query_map([user_id], |r| r.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn load_votes(&self, user_id: i64) -> Result<Vec<(String, String)>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare("SELECT product_key, vote FROM forecast_votes WHERE user_id=?")?;
        let rows = stmt.query_map([user_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn set_vote(&self, user_id: i64, product_key: &str, vote: &str) -> Result<i32> {
        let conn = self.connect()?;
        let current: String = conn
            .query_row(
                "SELECT vote FROM forecast_votes WHERE user_id=?1 AND product_key=?2",
                rusqlite::params![user_id, product_key],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| "0".into());
        let cur = parse_push(&current);
        let bit = vote.trim().to_ascii_lowercase();
        let push = if vote.trim().is_empty() {
            0
        } else if bit == "up" || bit == "+" {
            cur + 1
        } else if bit == "down" || bit == "-" {
            cur - 1
        } else {
            parse_push(vote)
        };
        let push = push.clamp(-VOTE_PUSH_MAX, VOTE_PUSH_MAX);
        let today = chrono::Local::now().date_naive().format("%Y-%m-%d").to_string();
        if push == 0 {
            conn.execute(
                "DELETE FROM forecast_votes WHERE user_id=?1 AND product_key=?2",
                rusqlite::params![user_id, product_key],
            )?;
        } else {
            conn.execute(
                "INSERT INTO forecast_votes(user_id, product_key, vote, updated_at) VALUES (?1,?2,?3,?4)
                 ON CONFLICT(user_id, product_key) DO UPDATE SET vote=excluded.vote, updated_at=excluded.updated_at",
                rusqlite::params![user_id, product_key, push.to_string(), today],
            )?;
        }
        Ok(push)
    }

    pub fn forecast(
        &self,
        user_id: i64,
        today: NaiveDate,
        horizon_days: i64,
        dept: &str,
        include_excluded: bool,
        extra_exclude: &[String],
    ) -> Result<Vec<ForecastRow>> {
        let blocked = self.load_exclusions(user_id)?;
        let votes = self.load_votes(user_id)?;
        let hist = self.buy_history(user_id, today)?;
        let want = crate::depts::normalize_dept(dept);
        let mut out = Vec::new();
        for rec in hist {
            let name = if rec.official_name.is_empty() {
                rec.receipt_name.to_ascii_lowercase()
            } else {
                rec.official_name.to_ascii_lowercase()
            };
            let excluded = blocked.iter().any(|k| k == &rec.key)
                || extra_exclude.iter().any(|e| {
                    let e = e.to_ascii_lowercase();
                    e == rec.key.to_ascii_lowercase() || e == name
                });
            let vote = votes
                .iter()
                .find(|(k, _)| k == &rec.key)
                .map(|(_, v)| v.as_str())
                .unwrap_or("0");
            let mut row = classify(&rec, today, excluded, vote);
            if !want.is_empty() && row.dept != want {
                continue;
            }
            if !include_excluded && !row.include {
                continue;
            }
            let horizon = horizon_days.max(0);
            row.in_horizon = row.due_in_days.map(|d| d <= horizon).unwrap_or(false);
            out.push(row);
        }
        out.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then(b.likely_push.cmp(&a.likely_push))
                .then(a.due_in_days.unwrap_or(1_000_000).cmp(&b.due_in_days.unwrap_or(1_000_000)))
        });
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ewma_weights_recent_gaps() {
        assert!(ewma(&[10.0, 10.0, 10.0, 4.0], 0.4) < 9.0);
    }
}
