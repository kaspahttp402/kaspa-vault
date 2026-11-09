//! `market.supply {}`: the coin's public supply figures (mined so far, the cap, the reward per
//! block now, and the next reward cut), read from the same allow-listed public service the
//! history walk uses (indexer.rs). Keyless and read-only; one fetch at a time, answers cached.

use crate::indexer;
use crate::network::Network;
use serde_json::{Value, json};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a good answer is reused.
pub const FRESH: Duration = Duration::from_secs(600);
/// How long a failed answer is reused (so a page that keeps asking does not hammer the service).
pub const FAILED_FRESH: Duration = Duration::from_secs(60);
pub const SUPPLY_DOWN: &str = "The supply figures could not be loaded right now.";
/// Smallest units per coin.
const UNIT: u128 = 100_000_000;
/// Above this a whole number is read as smallest units, not coins (the cap is under 3e10 coins).
const UNITS_ABOVE: u128 = 1_000_000_000_000;

/// The last answer: the service it came from, when, and the answer.
static CACHE: Mutex<Option<(&'static str, Instant, Value)>> = Mutex::new(None);
/// Held while a fetch runs: never more than one in flight.
static IN_FLIGHT: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

/// The answer for `network`, from the cache when fresh.
pub async fn supply(network: &Network) -> Value {
    let base = indexer::base_for(network);
    if let Some(v) = cached(base) {
        return v;
    }
    let _one = IN_FLIGHT.get_or_init(|| tokio::sync::Mutex::new(())).lock().await;
    if let Some(v) = cached(base) {
        return v; // filled while this call waited
    }
    let (coins, reward, halving) = tokio::join!(
        indexer::get_json(network, "/info/coinsupply"),
        indexer::get_json(network, "/info/blockreward"),
        indexer::get_json(network, "/info/halving"),
    );
    let ok = |r: Result<Option<Value>, String>| r.ok().flatten().unwrap_or(Value::Null);
    let answer = parse(&ok(coins), &ok(reward), &ok(halving));
    *CACHE.lock().unwrap_or_else(|p| p.into_inner()) = Some((base, Instant::now(), answer.clone()));
    answer
}

fn cached(base: &'static str) -> Option<Value> {
    let c = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    let (b, at, v) = c.as_ref()?;
    let life = if v["ok"] == true { FRESH } else { FAILED_FRESH };
    (*b == base && at.elapsed() < life).then(|| v.clone())
}

/// The answer from the three bodies (Null for one that did not come): ok once the mined
/// figure is known.
pub fn parse(coins: &Value, reward: &Value, halving: &Value) -> Value {
    let mined = amount(&coins["circulatingSupply"]);
    if mined.is_none() {
        return json!({"ok": false, "mined_kas": null, "max_kas": null, "reward_kas": null,
            "next_cut_at": null, "next_reward_kas": null, "error": SUPPLY_DOWN});
    }
    json!({
        "ok": true,
        "mined_kas": mined,
        "max_kas": amount(&coins["maxSupply"]),
        "reward_kas": coins_text(&reward["blockreward"]),
        "next_cut_at": cut_date(halving),
        "next_reward_kas": coins_text(&halving["nextHalvingAmount"]),
        "error": null,
    })
}

/// A supply figure (smallest units, or coins when small or with a point) as coin digits.
fn amount(v: &Value) -> Option<String> {
    let s = number_text(v)?;
    if s.contains('.') {
        return Some(trim(&s));
    }
    let n: u128 = s.parse().ok()?;
    if n <= UNITS_ABOVE {
        return Some(n.to_string());
    }
    Some(trim(&format!("{}.{:08}", n / UNIT, n % UNIT)))
}

/// A coin figure (already in coins: the block reward) as digits, at most 8 places.
fn coins_text(v: &Value) -> Option<String> {
    let s = number_text(v)?;
    if !s.contains('.') {
        let t = s.trim_start_matches('0');
        return Some(if t.is_empty() { "0".into() } else { t.to_string() });
    }
    let f: f64 = s.parse().ok()?;
    (f.is_finite() && f >= 0.0).then(|| trim(&format!("{f:.8}")))
}

/// Plain non-negative decimal text from a JSON number or string (nothing else).
fn number_text(v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => {
            let f = n.as_f64()?;
            if let Some(u) = n.as_u64() {
                u.to_string()
            } else if f.fract() == 0.0 && f.abs() < 1e30 {
                format!("{f:.0}") // a whole figure sent as a float: still a whole figure
            } else {
                format!("{f:.8}")
            }
        }
        _ => return None,
    };
    let digits = s.chars().all(|c| c.is_ascii_digit() || c == '.');
    let ends = !s.starts_with('.') && !s.ends_with('.');
    let shape = !s.is_empty() && s.len() <= 40 && s.matches('.').count() <= 1 && ends;
    (digits && shape).then_some(s)
}

/// Drops trailing zeros after a point (and the point when nothing is left after it).
fn trim(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The next cut's date text as the service gives it, else the ISO date of its timestamp.
fn cut_date(h: &Value) -> Option<String> {
    if let Some(d) = h["nextHalvingDate"].as_str() {
        let d = d.trim();
        if !d.is_empty() && d.len() <= 64 && d.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
            return Some(d.to_string());
        }
    }
    let t = number_text(&h["nextHalvingTimestamp"])?;
    let secs: u64 = t.split('.').next()?.parse().ok()?;
    // Milliseconds when far past year 3000 in seconds.
    let secs = if secs > 32_503_680_000 { secs / 1000 } else { secs };
    Some(iso_date(secs / 86_400))
}

/// YYYY-MM-DD for days since 1970-01-01 (civil-from-days).
fn iso_date(days: u64) -> String {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_public_shape() {
        let coins =
            json!({"circulatingSupply": "2612345678912000000", "maxSupply": "2870000000000000000"});
        let reward = json!({"blockreward": 3.27});
        let halving = json!({
            "nextHalvingTimestamp": 1767225600,
            "nextHalvingDate": "2026-01-01 00:00:00 UTC",
            "nextHalvingAmount": 3.0864,
        });
        let a = parse(&coins, &reward, &halving);
        assert_eq!(a["ok"], true);
        assert_eq!(a["mined_kas"], "26123456789.12");
        assert_eq!(a["max_kas"], "28700000000");
        assert_eq!(a["reward_kas"], "3.27");
        assert_eq!(a["next_cut_at"], "2026-01-01 00:00:00 UTC");
        assert_eq!(a["next_reward_kas"], "3.0864");
        assert_eq!(a["error"], Value::Null);
    }

    #[test]
    fn missing_fields_are_null_but_ok_with_mined() {
        let coins = json!({"circulatingSupply": 2612345678912000000u64});
        let a = parse(&coins, &Value::Null, &json!({"nextHalvingTimestamp": 1767225600}));
        assert_eq!(a["ok"], true);
        assert_eq!(a["mined_kas"], "26123456789.12");
        assert_eq!(a["max_kas"], Value::Null);
        assert_eq!(a["reward_kas"], Value::Null);
        assert_eq!(a["next_cut_at"], "2026-01-01");
        assert_eq!(a["next_reward_kas"], Value::Null);
    }

    #[test]
    fn no_mined_figure_is_a_plain_failure() {
        let bad = [
            Value::Null,
            json!({"circulatingSupply": "-5"}),
            json!({"circulatingSupply": "1e9"}),
            json!({"maxSupply": "1"}),
        ];
        for coins in bad {
            let a = parse(&coins, &json!({"blockreward": 3}), &Value::Null);
            assert_eq!(a["ok"], false);
            assert_eq!(a["error"], SUPPLY_DOWN);
            assert_eq!(a["mined_kas"], Value::Null);
        }
        for w in ["key", "DAA", "script", "sompi", "crypto", "wallet"] {
            assert!(!SUPPLY_DOWN.to_lowercase().contains(&w.to_lowercase()));
        }
    }

    #[test]
    fn coin_figures_and_dates() {
        assert_eq!(amount(&json!("26123456789.1200")).as_deref(), Some("26123456789.12"));
        assert_eq!(amount(&json!(28700000000u64)).as_deref(), Some("28700000000"));
        assert_eq!(coins_text(&json!("4")).as_deref(), Some("4"));
        assert_eq!(coins_text(&json!(true)), None);
        assert_eq!(iso_date(0), "1970-01-01");
        assert_eq!(iso_date(20_454), "2026-01-01");
        let ms = json!({"nextHalvingTimestamp": 1767225600000u64});
        assert_eq!(cut_date(&ms).as_deref(), Some("2026-01-01"));
    }
}
