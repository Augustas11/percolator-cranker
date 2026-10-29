use crate::{decode::Decoded, stats, Config};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

pub fn open(path: &Path) -> Result<Connection> {
    let c = Connection::open(path)?;
    c.busy_timeout(Duration::from_secs(5))?;
    c.execute_batch("PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS txs(signature TEXT PRIMARY KEY,slot INTEGER,block_time INTEGER,ok INTEGER,err_json TEXT,failed_ix_index INTEGER,error_code INTEGER,error_name TEXT);
-- portfolio associates successful closes with the liquidation exclusion query.
CREATE TABLE IF NOT EXISTS instructions(signature TEXT,ix_index INTEGER,tag INTEGER,name TEXT,decode_error TEXT,portfolio TEXT,PRIMARY KEY(signature,ix_index));
CREATE TABLE IF NOT EXISTS cranks(signature TEXT,ix_index INTEGER,slot INTEGER,block_time INTEGER,signer TEXT,target_portfolio TEXT,outcome TEXT,error_code INTEGER,PRIMARY KEY(signature,ix_index));
CREATE TABLE IF NOT EXISTS fills(signature TEXT,ix_index INTEGER,leg INTEGER,slot INTEGER,block_time INTEGER,taker_portfolio TEXT,taker_portfolio_id TEXT,maker_portfolio TEXT,maker_portfolio_id TEXT,asset_index INTEGER,market_id TEXT,size_q TEXT,exec_price TEXT,limit_price TEXT,fee_bps TEXT,price_known INTEGER,PRIMARY KEY(signature,ix_index,leg));
CREATE TABLE IF NOT EXISTS transfers(signature TEXT,ix_index INTEGER,slot INTEGER,block_time INTEGER,kind TEXT CHECK(kind IN ('deposit','withdraw')),portfolio TEXT,portfolio_id TEXT,signer TEXT,amount TEXT,ok INTEGER,PRIMARY KEY(signature,ix_index));
CREATE TABLE IF NOT EXISTS portfolio_snapshots(portfolio TEXT,slot INTEGER,taken_at INTEGER,owner TEXT,capital TEXT,pnl TEXT,positions_json TEXT,PRIMARY KEY(portfolio,slot));
CREATE TABLE IF NOT EXISTS liquidations(portfolio TEXT,slot INTEGER,signature TEXT,before_json TEXT,after_json TEXT,PRIMARY KEY(portfolio,signature));
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT);
CREATE INDEX IF NOT EXISTS fills_taker_slot ON fills(taker_portfolio,slot);
CREATE INDEX IF NOT EXISTS fills_maker_slot ON fills(maker_portfolio,slot);
CREATE INDEX IF NOT EXISTS snapshots_latest ON portfolio_snapshots(portfolio,slot DESC);
CREATE INDEX IF NOT EXISTS cranks_target_slot ON cranks(target_portfolio,slot);")?;
    Ok(c)
}
pub fn meta(c: &Connection, key: &str) -> Result<Option<String>> {
    Ok(
        c.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
            .optional()?,
    )
}
pub fn set_meta(c: &Connection, key: &str, value: &str) -> Result<()> {
    c.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value])?;
    Ok(())
}
pub fn contains(c: &Connection, signature: &str) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM txs WHERE signature=?1)",
        [signature],
        |r| r.get(0),
    )?)
}
/// Caller owns the SQLite transaction, including cursor updates.
pub fn insert(c: &Connection, t: &Decoded) -> Result<()> {
    if t.instructions.is_empty() {
        return Ok(());
    }
    c.execute(
        "INSERT OR IGNORE INTO txs VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            t.signature,
            t.slot,
            t.block_time,
            t.ok,
            t.err_json,
            t.failed_ix_index,
            t.error_code,
            t.error_name
        ],
    )?;
    for i in &t.instructions {
        c.execute(
            "INSERT OR IGNORE INTO instructions VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                t.signature,
                i.ix_index,
                i.tag,
                i.name,
                i.decode_error,
                i.portfolio
            ],
        )?;
    }
    for r in &t.cranks {
        c.execute(
            "INSERT OR IGNORE INTO cranks VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                t.signature,
                r.ix_index,
                t.slot,
                t.block_time,
                r.signer,
                r.target_portfolio,
                r.outcome,
                r.error_code
            ],
        )?;
    }
    for r in &t.fills {
        c.execute("INSERT OR IGNORE INTO fills VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",params![r.signature,r.ix_index,r.leg,r.slot,r.block_time,r.taker_portfolio,r.taker_portfolio_id.to_string(),r.maker_portfolio,r.maker_portfolio_id.to_string(),r.asset_index,r.market_id.to_string(),r.size_q,r.exec_price.map(|x|x.to_string()),r.limit_price.map(|x|x.to_string()),r.fee_bps.to_string(),r.price_known])?;
    }
    for r in &t.transfers {
        c.execute(
            "INSERT OR IGNORE INTO transfers VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                t.signature,
                r.ix_index,
                t.slot,
                t.block_time,
                r.kind,
                r.portfolio,
                r.portfolio_id.to_string(),
                r.signer,
                r.amount,
                t.ok
            ],
        )?;
    }
    Ok(())
}

pub struct Snapshot {
    pub portfolio: String,
    pub slot: u64,
    pub taken_at: i64,
    pub owner: String,
    pub capital: String,
    pub pnl: String,
    pub positions: Value,
}

pub fn snapshot(c: &Connection, s: &Snapshot) -> Result<()> {
    let previous: Option<(u64,String,String,String)> = c.query_row(
        "SELECT slot,capital,pnl,positions_json FROM portfolio_snapshots WHERE portfolio=?1 ORDER BY slot DESC LIMIT 1",
        [&s.portfolio],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let positions = s.positions.to_string();
    if let Some((slot, capital, pnl, p)) = &previous {
        if s.slot <= *slot || (capital == &s.capital && pnl == &s.pnl && p == &positions) {
            return Ok(());
        }
    }
    c.execute(
        "INSERT OR IGNORE INTO portfolio_snapshots VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            s.portfolio,
            s.slot,
            s.taken_at,
            s.owner,
            s.capital,
            s.pnl,
            positions
        ],
    )?;
    if let Some((slot, capital, pnl, p)) = previous {
        let before: Value = serde_json::from_str(&p)?;
        if magnitude(&s.positions)? >= magnitude(&before)? {
            return Ok(());
        }
        let excluded:bool=c.query_row("SELECT
            EXISTS(SELECT 1 FROM fills WHERE (taker_portfolio=?1 OR maker_portfolio=?1) AND slot>?2 AND slot<=?3)
            OR EXISTS(SELECT 1 FROM transfers WHERE portfolio=?1 AND kind='withdraw' AND ok=1 AND slot>?2 AND slot<=?3)
            OR EXISTS(SELECT 1 FROM instructions i JOIN txs t USING(signature) WHERE i.portfolio=?1 AND i.tag IN (8,30) AND t.ok=1 AND t.slot>?2 AND t.slot<=?3)",params![s.portfolio,slot,s.slot],|r|r.get(0))?;
        if !excluded {
            let crank:Option<String>=c.query_row("SELECT signature FROM cranks WHERE target_portfolio=?1 AND outcome='ok' AND slot>?2 AND slot<=?3 ORDER BY slot DESC,signature,ix_index LIMIT 1",params![s.portfolio,slot,s.slot],|r|r.get(0)).optional()?;
            if let Some(signature) = crank {
                let before = json!({"capital":capital,"pnl":pnl,"positions":before});
                let after = json!({"capital":s.capital,"pnl":s.pnl,"positions":s.positions});
                c.execute(
                    "INSERT OR IGNORE INTO liquidations VALUES(?1,?2,?3,?4,?5)",
                    params![
                        s.portfolio,
                        s.slot,
                        signature,
                        before.to_string(),
                        after.to_string()
                    ],
                )?;
            }
        }
    }
    Ok(())
}
fn magnitude(positions: &Value) -> Result<u128> {
    positions
        .as_array()
        .context("positions must be array")?
        .iter()
        .try_fold(0u128, |sum, l| {
            let q: i128 = l["basis_pos_q"]
                .as_str()
                .context("missing basis_pos_q")?
                .parse()?;
            sum.checked_add(q.unsigned_abs())
                .context("position magnitude overflow")
        })
}

pub fn stats(c: &Connection, cfg: &Config, now: i64) -> Result<Value> {
    let mut stmt = c.prepare("SELECT outcome,block_time FROM cranks")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let (cranks, uptime) =
        stats::cranks_and_uptime(&rows, now, cfg.cycle_secs, cfg.uptime_slack_cycles);
    let market: Value = meta(c, "market_json")?
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or(json!({}));
    let count = |sql: &str| -> Result<u64> { Ok(c.query_row(sql, [], |r| r.get(0))?) };
    let since: Option<u64> = c.query_row("SELECT MIN(slot) FROM portfolio_snapshots", [], |r| {
        r.get(0)
    })?;
    Ok(
        json!({"market":cfg.market.to_string(),"program":cfg.program.to_string(),
        "as_of_slot":meta(c,"last_ingest_slot")?.and_then(|s|s.parse::<u64>().ok()),
        "as_of":meta(c,"last_ingest_at")?.and_then(|s|s.parse::<i64>().ok()),"cranks":cranks,"uptime":uptime,
        "liquidations_observed":count("SELECT COUNT(*) FROM liquidations")?,"liquidations_since_slot":since,
        "open_interest":market.get("open_interest").cloned().unwrap_or(json!({})),
        "depositors":count("SELECT COUNT(DISTINCT portfolio) FROM transfers WHERE kind='deposit' AND ok=1")?,
        "portfolios":count("SELECT COUNT(DISTINCT portfolio) FROM portfolio_snapshots")?,
        "fills":count("SELECT COUNT(*) FROM fills")?,"mode":market["mode"],
        "materialized_portfolio_count":market["materialized_portfolio_count"],
        "insurance":market["insurance"],"vault":market["vault"]}),
    )
}

pub fn fills(c: &Connection, portfolio: &str, limit: u64, before: Option<u64>) -> Result<Value> {
    // Fetch one extra row to determine whether there is another page.
    let mut stmt=c.prepare("SELECT * FROM fills WHERE (taker_portfolio=?1 OR maker_portfolio=?1) AND (?2 IS NULL OR slot<?2) ORDER BY slot DESC,signature DESC,ix_index DESC,leg DESC LIMIT ?3")?;
    let read = |r: &rusqlite::Row<'_>| -> rusqlite::Result<Value> {
        let size: String = r.get(11)?;
        let taker: String = r.get(5)?;
        let positive = size.parse::<i128>().map(|n| n > 0).unwrap_or(false);
        let unsigned = |n| -> rusqlite::Result<Value> {
            let s: Option<String> = r.get(n)?;
            Ok(s.and_then(|v| v.parse::<u64>().ok())
                .map_or(Value::Null, |n| json!(n)))
        };
        Ok(
            json!({"signature":r.get::<_,String>(0)?,"ix_index":r.get::<_,u64>(1)?,"leg":r.get::<_,u64>(2)?,"slot":r.get::<_,u64>(3)?,"block_time":r.get::<_,Option<i64>>(4)?,
            "taker_portfolio":taker,"taker_portfolio_id":unsigned(6)?,"maker_portfolio":r.get::<_,String>(7)?,"maker_portfolio_id":unsigned(8)?,
            "asset_index":r.get::<_,u64>(9)?,"market_id":unsigned(10)?,"size_q":size,"exec_price":unsigned(12)?,"limit_price":unsigned(13)?,"fee_bps":unsigned(14)?,"price_known":r.get::<_,bool>(15)?,
            "side":if (taker == portfolio) == positive {"buy"} else {"sell"}}),
        )
    };
    let mut rows = stmt
        .query_map(params![portfolio, before, limit + 1], read)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut next = None;
    if rows.len() > limit as usize {
        rows.truncate(limit as usize);
        let slot = rows.last().unwrap()["slot"].as_u64().unwrap();
        next = Some(slot);
    }
    Ok(json!({"fills":rows,"next_before_slot":next}))
}
pub fn positions(c: &Connection, owner: &str) -> Result<Value> {
    let mut stmt=c.prepare("SELECT portfolio,slot,capital,pnl,positions_json FROM portfolio_snapshots s WHERE owner=?1 AND slot=(SELECT MAX(slot) FROM portfolio_snapshots WHERE portfolio=s.portfolio) ORDER BY portfolio")?;
    let rows = stmt
        .query_map([owner], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let slot = rows.iter().map(|r| r.1).max();
    let portfolios=rows.into_iter().map(|(p,_,capital,pnl,positions)|Ok(json!({"portfolio":p,"capital":capital,"pnl":pnl,"positions":serde_json::from_str::<Value>(&positions)?}))).collect::<Result<Vec<_>>>()?;
    Ok(json!({"owner":owner,"as_of_slot":slot,"portfolios":portfolios}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::tests::{fixture, MARKET, PROGRAM};
    const A: &str = "5XWHkzJhpp1hFjMrTwbARWqCJGGuRmrrDDqHW9EmYbzN";
    fn memory() -> Connection {
        open(Path::new(":memory:")).unwrap()
    }
    fn snap(slot: u64, q: i128) -> Snapshot {
        Snapshot {
            portfolio: A.into(),
            slot,
            taken_at: 100,
            owner: PROGRAM.into(),
            capital: "1000".into(),
            pnl: "0".into(),
            positions: json!([{"asset_index":0,"market_id":1,"side":"long","basis_pos_q":q.to_string()}]),
        }
    }
    fn counts(c: &Connection) -> Vec<u64> {
        [
            "SELECT COUNT(*) FROM txs",
            "SELECT COUNT(*) FROM instructions",
            "SELECT COUNT(*) FROM cranks",
            "SELECT COUNT(*) FROM fills",
            "SELECT COUNT(*) FROM transfers",
        ]
        .iter()
        .map(|sql| c.query_row(sql, [], |r| r.get(0)).unwrap())
        .collect()
    }
    #[test]
    fn duplicate_batches_are_identical_and_nonmarket_txs_not_stored() {
        let mut c = memory();
        let fixtures = [
            "crank_trade",
            "crank",
            "deposit",
            "init_portfolio",
            "push_mark",
        ];
        for _ in 0..2 {
            let batch = c.transaction().unwrap();
            for f in fixtures {
                insert(&batch, &fixture(f)).unwrap();
            }
            set_meta(&batch, "newest_signature", "tip").unwrap();
            batch.commit().unwrap();
            assert_eq!(counts(&c), vec![5, 7, 3, 1, 1]);
        }
        assert_eq!(
            meta(&c, "newest_signature").unwrap().as_deref(),
            Some("tip")
        );
        let mut unrelated = fixture("crank");
        unrelated.instructions.clear();
        unrelated.cranks.clear();
        unrelated.signature = "unrelated".into();
        insert(&c, &unrelated).unwrap();
        assert!(!contains(&c, "unrelated").unwrap());
        // Dropping an uncommitted page must also roll back its cursor.
        let tx = c.transaction().unwrap();
        set_meta(&tx, "newest_signature", "uncommitted").unwrap();
        drop(tx);
        assert_eq!(
            meta(&c, "newest_signature").unwrap().as_deref(),
            Some("tip")
        );
    }
    #[test]
    fn snapshots_only_on_change_and_forward_liquidation_heuristic() {
        let c = memory();
        let crank = fixture("crank");
        let slot = crank.slot;
        snapshot(&c, &snap(slot - 1, 100)).unwrap();
        snapshot(&c, &snap(slot, 100)).unwrap();
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM portfolio_snapshots", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            1
        );
        insert(&c, &crank).unwrap();
        snapshot(&c, &snap(slot + 1, 50)).unwrap();
        snapshot(&c, &snap(slot + 1, 50)).unwrap();
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM liquidations", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            1
        );
        let out = positions(&c, PROGRAM).unwrap();
        assert_eq!(out["portfolios"][0]["positions"][0]["basis_pos_q"], "50");
        assert_eq!(out["as_of_slot"], slot + 1);
    }
    #[test]
    fn successful_trade_withdraw_and_closes_exclude_liquidations() {
        for kind in ["fill", "withdraw", "close", "close_resolved"] {
            let c = memory();
            let mut crank = fixture("crank");
            let slot = crank.slot;
            snapshot(&c, &snap(slot - 1, 100)).unwrap();
            insert(&c, &crank).unwrap();
            match kind {
                "fill" => {
                    let mut trade = fixture("crank_trade");
                    trade.slot = slot;
                    for f in &mut trade.fills {
                        f.slot = slot;
                    }
                    insert(&c, &trade).unwrap();
                }
                "withdraw" => {
                    let mut d = fixture("deposit");
                    d.slot = slot;
                    d.transfers[0].kind = "withdraw";
                    insert(&c, &d).unwrap();
                }
                _ => {
                    crank.signature = kind.into();
                    crank.cranks.clear();
                    crank.instructions[0].tag = Some(if kind == "close" { 8 } else { 30 });
                    insert(&c, &crank).unwrap();
                }
            }
            snapshot(&c, &snap(slot + 1, 50)).unwrap();
            assert_eq!(
                c.query_row("SELECT COUNT(*) FROM liquidations", [], |r| r
                    .get::<_, u64>(0))
                    .unwrap(),
                0,
                "{kind}"
            );
        }
    }
    #[test]
    fn fills_sides_filters_cursors_and_full_width_numbers() {
        let c = memory();
        let mut trade = fixture("crank_trade");
        trade.fills[0].taker_portfolio_id = u64::MAX;
        trade.fills[0].size_q = i128::MIN.to_string();
        insert(&c, &trade).unwrap();
        let f = fills(&c, A, 100, None).unwrap();
        assert_eq!(f["fills"][0]["taker_portfolio_id"], u64::MAX);
        assert_eq!(f["fills"][0]["side"], "sell");
        let b = &trade.fills[0].maker_portfolio;
        assert_eq!(fills(&c, b, 1, None).unwrap()["fills"][0]["side"], "buy");
        assert_eq!(
            fills(&c, A, 1, Some(trade.slot)).unwrap()["fills"],
            json!([])
        );
        assert_eq!(fills(&c, MARKET, 1, None).unwrap()["fills"], json!([]));
        trade.signature = "older".into();
        trade.slot -= 1;
        trade.fills[0].signature = "older".into();
        trade.fills[0].slot -= 1;
        insert(&c, &trade).unwrap();
        let first = fills(&c, A, 1, None).unwrap();
        assert_eq!(first["fills"].as_array().unwrap().len(), 1);
        let cursor = first["next_before_slot"].as_u64().unwrap();
        let second = fills(&c, A, 1, Some(cursor)).unwrap();
        assert_eq!(second["fills"][0]["signature"], "older");
        assert!(second["next_before_slot"].is_null());
    }
    #[test]
    fn stats_have_counts_and_no_volume() {
        let c = memory();
        insert(&c, &fixture("crank_trade")).unwrap();
        insert(&c, &fixture("deposit")).unwrap();
        let (_, cfg) = Config::parse(["serve".into()]).unwrap();
        let s = stats(&c, &cfg, now_for_test()).unwrap();
        assert_eq!(s["cranks"]["total"], 2);
        assert_eq!(s["depositors"], 1);
        assert_eq!(s["fills"], 1);
        assert!(s.get("volume").is_none());
        assert!(s.get("notional").is_none());
    }
    fn now_for_test() -> i64 {
        2_000_000_000
    }
}
