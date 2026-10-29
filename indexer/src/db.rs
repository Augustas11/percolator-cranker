use crate::{decode::Decoded, stats, Config};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

pub fn open(path: &Path) -> Result<Connection> {
    let c = Connection::open(path)?;
    c.busy_timeout(Duration::from_secs(5))?;
    let legacy: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('fills') WHERE name='exec_price')
         OR (EXISTS(SELECT 1 FROM pragma_table_info('marks'))
             AND (NOT EXISTS(SELECT 1 FROM pragma_table_info('marks') WHERE name='observation_sequence')
                  OR NOT EXISTS(SELECT 1 FROM pragma_table_info('marks') WHERE name='market_id')))",
        [],
        |r| r.get(0),
    )?;
    anyhow::ensure!(
        !legacy,
        "legacy indexer schema: use a new database and backfill to rebuild corrected history"
    );
    c.execute_batch("PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS txs(signature TEXT PRIMARY KEY,slot INTEGER,block_time INTEGER,ok INTEGER,err_json TEXT,failed_ix_index INTEGER,error_code INTEGER,error_name TEXT);
-- portfolio associates successful closes with the liquidation exclusion query.
CREATE TABLE IF NOT EXISTS instructions(signature TEXT,ix_index INTEGER,tag INTEGER,name TEXT,decode_error TEXT,portfolio TEXT,PRIMARY KEY(signature,ix_index));
CREATE TABLE IF NOT EXISTS cranks(signature TEXT,ix_index INTEGER,slot INTEGER,block_time INTEGER,signer TEXT,target_portfolio TEXT,outcome TEXT,error_code INTEGER,PRIMARY KEY(signature,ix_index));
CREATE TABLE IF NOT EXISTS fills(signature TEXT,ix_index INTEGER,leg INTEGER,slot INTEGER,block_time INTEGER,taker_portfolio TEXT,taker_portfolio_id TEXT,maker_portfolio TEXT,maker_portfolio_id TEXT,asset_index INTEGER,market_id TEXT,size_q TEXT,reported_price TEXT,limit_price TEXT,requested_fee_bps TEXT,status TEXT,PRIMARY KEY(signature,ix_index,leg));
CREATE TABLE IF NOT EXISTS marks(signature TEXT,ix_index INTEGER,slot INTEGER,asset_index INTEGER,price TEXT,observation_sequence TEXT,market_id TEXT,PRIMARY KEY(signature,ix_index));
CREATE INDEX IF NOT EXISTS marks_asset_slot ON marks(asset_index,slot DESC);
CREATE TABLE IF NOT EXISTS current_portfolios(portfolio TEXT PRIMARY KEY);
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
        c.execute("INSERT OR IGNORE INTO fills VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",params![r.signature,r.ix_index,r.leg,r.slot,r.block_time,r.taker_portfolio,r.taker_portfolio_id.to_string(),r.maker_portfolio,r.maker_portfolio_id.to_string(),r.asset_index,r.market_id.to_string(),r.size_q,r.reported_price.map(|x|x.to_string()),r.limit_price.map(|x|x.to_string()),r.requested_fee_bps.to_string(),r.status])?;
    }
    for r in &t.marks {
        c.execute(
            "INSERT OR IGNORE INTO marks VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                t.signature,
                r.ix_index,
                t.slot,
                r.asset_index,
                r.price.to_string(),
                r.observation_sequence.to_string(),
                r.market_id.to_string()
            ],
        )?;
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
    let previous: Option<(u64,String,String,String,String)> = c.query_row(
        "SELECT slot,capital,pnl,positions_json,owner FROM portfolio_snapshots WHERE portfolio=?1 ORDER BY slot DESC LIMIT 1",
        [&s.portfolio],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let positions = s.positions.to_string();
    if let Some((slot, capital, pnl, p, owner)) = &previous {
        if s.slot <= *slot
            || (owner == &s.owner && capital == &s.capital && pnl == &s.pnl && p == &positions)
        {
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
    if let Some((slot, capital, pnl, p, _)) = previous {
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
        "fills":count("SELECT COUNT(*) FROM fills WHERE status='executed'")?,"mode":market["mode"],
        "materialized_portfolio_count":market["materialized_portfolio_count"],
        "insurance":market["insurance"],"vault":market["vault"]}),
    )
}

/// Cursor numbers must fit SQLite integers; signatures are canonical Solana signatures.
pub fn fill_cursor(value: &str) -> Result<(i64, String, i64, i64)> {
    let parts: Vec<_> = value.split('.').collect();
    anyhow::ensure!(parts.len() == 4, "invalid before cursor");
    let number = |s: &str| -> Result<i64> {
        anyhow::ensure!(
            !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()),
            "invalid cursor number"
        );
        let n: i64 = s.parse()?;
        anyhow::ensure!(n.to_string() == s, "noncanonical cursor number");
        Ok(n)
    };
    let signature = bs58::decode(parts[1]).into_vec()?;
    anyhow::ensure!(
        signature.len() == 64 && bs58::encode(&signature).into_string() == parts[1],
        "invalid cursor signature"
    );
    Ok((
        number(parts[0])?,
        parts[1].into(),
        number(parts[2])?,
        number(parts[3])?,
    ))
}
pub fn cached_stats(c: &Connection, cfg: &Config) -> Result<Value> {
    if let Some(value) = meta(c, "stats_json")? {
        return Ok(serde_json::from_str(&value)?);
    }
    let (cranks, uptime) =
        stats::cranks_and_uptime(&[], 0, cfg.cycle_secs, cfg.uptime_slack_cycles);
    Ok(
        json!({"market":cfg.market.to_string(),"program":cfg.program.to_string(),
        "as_of_slot":null,"as_of":null,"cranks":cranks,"uptime":uptime,
        "liquidations_observed":0,"liquidations_since_slot":null,"open_interest":{},
        "depositors":0,"portfolios":0,"fills":0,"mode":null,
        "materialized_portfolio_count":null,"insurance":null,"vault":null}),
    )
}
pub fn fills(c: &Connection, portfolio: &str, limit: u64, before: Option<&str>) -> Result<Value> {
    let cursor = before.map(fill_cursor).transpose()?;
    let (slot, signature, ix, leg) = cursor.unwrap_or((i64::MAX, String::new(), 0, 0));
    let mut stmt = c.prepare(
        "SELECT f.*,
        (SELECT m.price FROM marks m WHERE m.asset_index=f.asset_index AND m.market_id=f.market_id
         AND (m.slot<f.slot OR (m.signature=f.signature AND m.ix_index<f.ix_index))
         ORDER BY m.slot DESC,length(m.observation_sequence) DESC,m.observation_sequence DESC,m.signature DESC,m.ix_index DESC LIMIT 1)
        FROM fills f WHERE (taker_portfolio=?1 OR maker_portfolio=?1)
        AND (?2 IS NULL OR (f.slot,f.signature,f.ix_index,f.leg)<(?3,?4,?5,?6))
        ORDER BY f.slot DESC,f.signature DESC,f.ix_index DESC,f.leg DESC LIMIT ?7",
    )?;
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
            "asset_index":r.get::<_,u64>(9)?,"market_id":unsigned(10)?,"size_q":size,"reported_price":unsigned(12)?,"limit_price":unsigned(13)?,"requested_fee_bps":unsigned(14)?,"status":r.get::<_,String>(15)?,"mark_price":unsigned(16)?,
            "price_note":"estimate: execution uses the engine mark; mark_price is the last pushed mark before this fill",
            "side":if (taker == portfolio) == positive {"buy"} else {"sell"}}),
        )
    };
    let mut rows = stmt
        .query_map(
            params![portfolio, before, slot, signature, ix, leg, limit + 1],
            read,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut next = None;
    if rows.len() > limit as usize {
        rows.truncate(limit as usize);
        let row = rows.last().unwrap();
        next = Some(format!(
            "{}.{}.{}.{}",
            row["slot"],
            row["signature"].as_str().unwrap(),
            row["ix_index"],
            row["leg"]
        ));
    }
    Ok(json!({"fills":rows,"next_before":next}))
}
pub fn positions(c: &Connection, owner: &str) -> Result<Value> {
    let mut stmt=c.prepare("SELECT s.portfolio,slot,capital,pnl,positions_json FROM portfolio_snapshots s JOIN current_portfolios c ON c.portfolio=s.portfolio WHERE owner=?1 AND slot=(SELECT MAX(slot) FROM portfolio_snapshots WHERE portfolio=s.portfolio) ORDER BY s.portfolio")?;
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
    let slot = meta(c, "last_ingest_slot")?.and_then(|s| s.parse::<u64>().ok());
    let portfolios=rows.into_iter().map(|(p,last_changed_slot,capital,pnl,positions)|Ok(json!({"portfolio":p,"last_changed_slot":last_changed_slot,"capital":capital,"pnl":pnl,"positions":serde_json::from_str::<Value>(&positions)?}))).collect::<Result<Vec<_>>>()?;
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
        c.execute("INSERT INTO current_portfolios VALUES(?1)", [A])
            .unwrap();
        set_meta(&c, "last_ingest_slot", &(slot + 10).to_string()).unwrap();
        // An unchanged portfolio has an older snapshot than ingest freshness.
        snapshot(&c, &snap(slot + 10, 50)).unwrap();
        let out = positions(&c, PROGRAM).unwrap();
        assert_eq!(out["portfolios"][0]["positions"][0]["basis_pos_q"], "50");
        assert_eq!(out["as_of_slot"], slot + 10);
        assert_eq!(out["portfolios"][0]["last_changed_slot"], slot + 1);
        let empty = positions(&c, MARKET).unwrap();
        assert_eq!(empty["as_of_slot"], slot + 10);
        assert_eq!(empty["portfolios"], json!([]));
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
        assert_eq!(fills(&c, MARKET, 1, None).unwrap()["fills"], json!([]));
        trade.signature = bs58::encode([0u8; 64]).into_string();
        // Keep both rows in the same slot to exercise a split page.
        trade.fills[0].signature = bs58::encode([0u8; 64]).into_string();
        trade.fills[0].ix_index += 1;
        insert(&c, &trade).unwrap();
        let first = fills(&c, A, 1, None).unwrap();
        assert_eq!(first["fills"].as_array().unwrap().len(), 1);
        let cursor = first["next_before"].as_str().unwrap();
        let second = fills(&c, A, 1, Some(cursor)).unwrap();
        assert_eq!(
            second["fills"][0]["signature"],
            bs58::encode([0u8; 64]).into_string()
        );
        assert!(second["next_before"].is_null());
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
    #[test]
    fn marks_only_precede_fills_by_slot_or_same_transaction() {
        let c = memory();
        let trade = fixture("crank_trade");
        let f = &trade.fills[0];
        insert(&c, &trade).unwrap();
        assert!(fills(&c, A, 1, None).unwrap()["fills"][0]["mark_price"].is_null());
        let add = |signature: &str, ix: usize, slot: u64, asset: u16, price: &str| {
            c.execute(
                "INSERT INTO marks VALUES(?1,?2,?3,?4,?5,'1',?6)",
                params![signature, ix, slot, asset, price, f.market_id.to_string()],
            )
            .unwrap();
        };
        add("earlier", 0, f.slot - 1, f.asset_index, "11");
        add("other_asset", 0, f.slot - 1, f.asset_index + 1, "99");
        add("same_slot_other_tx", 0, f.slot, f.asset_index, "99");
        add(&f.signature, f.ix_index + 1, f.slot, f.asset_index, "99");
        add("future", 0, f.slot + 1, f.asset_index, "99");
        assert_eq!(fills(&c, A, 1, None).unwrap()["fills"][0]["mark_price"], 11);
        add(
            &f.signature,
            f.ix_index - 1,
            f.slot,
            f.asset_index,
            &u64::MAX.to_string(),
        );
        assert_eq!(
            fills(&c, A, 1, None).unwrap()["fills"][0]["mark_price"],
            u64::MAX
        );
    }
    #[test]
    fn reused_asset_index_requires_a_mark_from_the_same_generation() {
        let c = memory();
        let mut trade = fixture("crank_trade");
        trade.slot = 20;
        trade.fills[0].slot = 20;
        insert(&c, &trade).unwrap();
        let mut mark = fixture("push_mark");
        mark.slot = 10;
        mark.marks[0].asset_index = trade.fills[0].asset_index;
        mark.marks[0].market_id = trade.fills[0].market_id;
        mark.marks[0].price = 11;
        insert(&c, &mark).unwrap();

        trade.signature = "new_generation".into();
        trade.fills[0].signature = trade.signature.clone();
        trade.slot = 30;
        trade.fills[0].slot = 30;
        trade.fills[0].market_id = u64::MAX;
        insert(&c, &trade).unwrap();
        let out = fills(&c, A, 10, None).unwrap();
        assert!(out["fills"][0]["mark_price"].is_null());
        assert_eq!(out["fills"][1]["mark_price"], 11);

        mark.signature = "new_generation_mark".into();
        mark.slot = 25;
        mark.marks[0].market_id = u64::MAX;
        mark.marks[0].price = 22;
        insert(&c, &mark).unwrap();
        let out = fills(&c, A, 10, None).unwrap();
        assert_eq!(out["fills"][0]["mark_price"], 22);
        assert_eq!(out["fills"][1]["mark_price"], 11);
    }
    #[test]
    fn same_slot_marks_order_by_full_width_observation_sequence() {
        let c = memory();
        let trade = fixture("crank_trade");
        let f = &trade.fills[0];
        insert(&c, &trade).unwrap();
        for (signature, sequence, price) in [
            ("z", "9", "11"),
            ("a", "10", "22"),
            ("b", "18446744073709551615", "33"),
        ] {
            c.execute(
                "INSERT INTO marks VALUES(?1,0,?2,?3,?4,?5,?6)",
                params![
                    signature,
                    f.slot - 1,
                    f.asset_index,
                    price,
                    sequence,
                    f.market_id.to_string()
                ],
            )
            .unwrap();
            assert_eq!(
                fills(&c, A, 1, None).unwrap()["fills"][0]["mark_price"],
                price.parse::<u64>().unwrap()
            );
        }
    }
    #[test]
    fn marks_without_market_id_require_rebuild() {
        let c = memory();
        c.execute_batch("DROP TABLE marks; CREATE TABLE marks(signature TEXT,ix_index INTEGER,slot INTEGER,asset_index INTEGER,price TEXT,observation_sequence TEXT);").unwrap();
        // Exercise open() without leaving a database file behind.
        let path = std::env::temp_dir().join(format!(
            "indexer-legacy-marks-{}.sqlite",
            std::process::id()
        ));
        c.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        let result = open(&path);
        std::fs::remove_file(path).unwrap();
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("legacy indexer schema"));
    }
    #[test]
    fn owner_changes_and_closed_portfolios_preserve_history() {
        let mut c = memory();
        snapshot(&c, &snap(1, 100)).unwrap();
        c.execute("INSERT INTO current_portfolios VALUES(?1)", [A])
            .unwrap();
        let mut changed = snap(2, 100);
        changed.owner = MARKET.into();
        snapshot(&c, &changed).unwrap();
        assert!(positions(&c, PROGRAM).unwrap()["portfolios"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(
            positions(&c, MARKET).unwrap()["portfolios"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let tx = c.transaction().unwrap();
        tx.execute("DELETE FROM current_portfolios", []).unwrap();
        drop(tx);
        assert_eq!(
            positions(&c, MARKET).unwrap()["portfolios"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        c.execute("DELETE FROM current_portfolios", []).unwrap();
        assert!(positions(&c, MARKET).unwrap()["portfolios"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM portfolio_snapshots", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            2
        );
    }
    #[test]
    fn requested_fills_are_not_counted_and_stats_are_cached() {
        let c = memory();
        let (_, cfg) = Config::parse(["serve".into()]).unwrap();
        let empty = cached_stats(&c, &cfg).unwrap();
        let mut trade = fixture("crank_trade");
        trade.fills[0].status = "requested";
        insert(&c, &trade).unwrap();
        let s = stats(&c, &cfg, 1234).unwrap();
        assert_eq!(s["fills"], 0);
        assert_eq!(s["uptime"]["window_end"], 1234);
        assert_eq!(cached_stats(&c, &cfg).unwrap(), empty);
        set_meta(&c, "stats_json", &s.to_string()).unwrap();
        c.execute("DELETE FROM cranks", []).unwrap();
        assert_eq!(cached_stats(&c, &cfg).unwrap(), s);
    }
    #[test]
    fn cursor_resumes_inside_one_transaction_and_rejects_invalid_values() {
        let c = memory();
        let mut trade = fixture("crank_trade");
        let mut leg = crate::decode::tests::fixture("crank_trade").fills.remove(0);
        leg.leg = 1;
        trade.fills.push(leg);
        insert(&c, &trade).unwrap();
        let first = fills(&c, A, 1, None).unwrap();
        assert_eq!(first["fills"][0]["leg"], 1);
        let second = fills(&c, A, 1, first["next_before"].as_str()).unwrap();
        assert_eq!(second["fills"][0]["leg"], 0);
        assert!(second["next_before"].is_null());
        for value in ["", "1.a.2.3", "1.a.2.3.4", "-1.a.0.0", "1.' OR 1=1.0.0"] {
            assert!(fill_cursor(value).is_err());
        }
        let sig = bs58::encode([0u8; 64]).into_string();
        for n in ["01", "+1", "9223372036854775808", " 1"] {
            assert!(fill_cursor(&format!("{n}.{sig}.0.0")).is_err());
        }
    }
    fn now_for_test() -> i64 {
        2_000_000_000
    }
}
