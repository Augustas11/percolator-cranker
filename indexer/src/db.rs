use crate::{decode::Decoded, stats, Config};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, time::Duration};

pub fn open(path: &Path) -> Result<Connection> {
    let c = Connection::open(path)?;
    c.busy_timeout(Duration::from_secs(5))?;
    let legacy: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('fills') WHERE name='exec_price')
         OR (EXISTS(SELECT 1 FROM pragma_table_info('fills'))
             AND NOT EXISTS(SELECT 1 FROM pragma_table_info('fills') WHERE name='executed_size'))
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
CREATE TABLE IF NOT EXISTS fills(signature TEXT,ix_index INTEGER,leg INTEGER,slot INTEGER,block_time INTEGER,taker_portfolio TEXT,taker_portfolio_id TEXT,maker_portfolio TEXT,maker_portfolio_id TEXT,asset_index INTEGER,market_id TEXT,size_q TEXT,reported_price TEXT,limit_price TEXT,requested_fee_bps TEXT,status TEXT,executed_size TEXT,applied_price TEXT,taker_owner TEXT,PRIMARY KEY(signature,ix_index,leg));
CREATE TABLE IF NOT EXISTS marks(signature TEXT,ix_index INTEGER,slot INTEGER,asset_index INTEGER,price TEXT,observation_sequence TEXT,market_id TEXT,PRIMARY KEY(signature,ix_index));
CREATE INDEX IF NOT EXISTS marks_asset_slot ON marks(asset_index,slot DESC);
CREATE INDEX IF NOT EXISTS marks_asset_market_slot ON marks(asset_index,market_id,slot DESC);
CREATE TABLE IF NOT EXISTS current_portfolios(portfolio TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS transfers(signature TEXT,ix_index INTEGER,slot INTEGER,block_time INTEGER,kind TEXT CHECK(kind IN ('deposit','withdraw')),portfolio TEXT,portfolio_id TEXT,signer TEXT,amount TEXT,ok INTEGER,PRIMARY KEY(signature,ix_index));
CREATE TABLE IF NOT EXISTS portfolio_snapshots(portfolio TEXT,slot INTEGER,taken_at INTEGER,owner TEXT,capital TEXT,pnl TEXT,positions_json TEXT,PRIMARY KEY(portfolio,slot));
CREATE TABLE IF NOT EXISTS liquidations(portfolio TEXT,slot INTEGER,signature TEXT,before_json TEXT,after_json TEXT,PRIMARY KEY(portfolio,signature));
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT);
CREATE INDEX IF NOT EXISTS txs_slot ON txs(slot,signature);
CREATE INDEX IF NOT EXISTS fills_slot ON fills(slot DESC,signature DESC,ix_index DESC,leg DESC);
CREATE INDEX IF NOT EXISTS fills_status_slot ON fills(status,slot DESC,signature DESC,ix_index DESC,leg DESC);
CREATE INDEX IF NOT EXISTS fills_status_time ON fills(status,block_time);
CREATE INDEX IF NOT EXISTS instructions_portfolio ON instructions(portfolio, name);
CREATE INDEX IF NOT EXISTS fills_taker_slot ON fills(taker_portfolio,slot);
CREATE INDEX IF NOT EXISTS fills_taker_slot_order ON fills(taker_portfolio,slot DESC,signature DESC,ix_index DESC,leg DESC);
CREATE INDEX IF NOT EXISTS fills_maker_slot ON fills(maker_portfolio,slot);
CREATE INDEX IF NOT EXISTS transfers_portfolio ON transfers(portfolio);
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
pub fn check_start_slot(c: &Connection, start_slot: Option<u64>) -> Result<()> {
    if let Some(existing) = meta(c, "start_slot")? {
        anyhow::ensure!(
            Some(existing.parse::<u64>()?) == start_slot,
            "database start_slot differs from configuration; use a fresh DB for a new season"
        );
    } else if let Some(slot) = start_slot {
        anyhow::ensure!(
            meta(c, "oldest_signature")?.is_none(),
            "database already has ingested history; use a fresh DB for a new season"
        );
        set_meta(c, "start_slot", &slot.to_string())?;
    }
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
        c.execute("INSERT OR IGNORE INTO fills VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",params![r.signature,r.ix_index,r.leg,r.slot,r.block_time,r.taker_portfolio,r.taker_portfolio_id.to_string(),r.maker_portfolio,r.maker_portfolio_id.to_string(),r.asset_index,r.market_id.to_string(),r.size_q,r.reported_price.map(|x|x.to_string()),r.limit_price.map(|x|x.to_string()),r.requested_fee_bps.to_string(),r.status,r.executed_size,r.applied_price.map(|x|x.to_string()),r.taker_owner])?;
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
    let (fills_24h, volume_24h) = window(c, now - 86_400)?;
    Ok(
        json!({"since_slot":cfg.start_slot,"market":cfg.market.to_string(),"program":cfg.program.to_string(),
        "as_of_slot":meta(c,"last_ingest_slot")?.and_then(|s|s.parse::<u64>().ok()),
        "as_of":meta(c,"last_ingest_at")?.and_then(|s|s.parse::<i64>().ok()),"cranks":cranks,"uptime":uptime,
        "liquidations_observed":count("SELECT COUNT(*) FROM liquidations")?,"liquidations_since_slot":since,
        "open_interest":market.get("open_interest").cloned().unwrap_or(json!({})),
        "depositors":count("SELECT COUNT(DISTINCT portfolio) FROM transfers WHERE kind='deposit' AND ok=1")?,
        "portfolios":count("SELECT COUNT(DISTINCT portfolio) FROM portfolio_snapshots")?,
        "fills":count("SELECT COUNT(*) FROM fills WHERE status='executed'")?,
        "fills_24h":fills_24h,"volume_24h":volume_24h.to_string(),"mode":market["mode"],
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
        json!({"since_slot":cfg.start_slot,"market":cfg.market.to_string(),"program":cfg.program.to_string(),
        "as_of_slot":null,"as_of":null,"cranks":cranks,"uptime":uptime,
        "liquidations_observed":0,"liquidations_since_slot":null,"open_interest":{},
        "depositors":0,"portfolios":0,"fills":0,"fills_24h":0,"volume_24h":"0","mode":null,
        "materialized_portfolio_count":null,"insurance":null,"vault":null}),
    )
}
/// Executed fills with block_time >= since: count and notional (|size| * price / 1e6, the
/// season volume unit), all takers. Fills without any known price count without volume.
fn window(c: &Connection, since: i64) -> Result<(u64, i128)> {
    let mut stmt = c.prepare(&format!(
        "SELECT f.executed_size,{FILL_PRICE} FROM fills f
        WHERE f.status='executed' AND f.block_time>=?1"
    ))?;
    let mut count = 0u64;
    let mut volume = 0i128;
    for row in stmt.query_map([since], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, Option<String>>(1)?,
        ))
    })? {
        let (size, price) = row?;
        count += 1;
        if let (Some(size), Some(price)) = (size, price) {
            if price.parse::<u64>().is_ok() && size.parse::<i128>().is_ok() {
                add_notional(&mut volume, &size, &price)?;
            }
        }
    }
    Ok((count, volume))
}
/// Fill price in preference order, and its source label for API rows.
const FILL_PRICE: &str = "COALESCE(f.reported_price,f.applied_price,(SELECT m.price FROM marks m WHERE m.asset_index=f.asset_index AND m.market_id=f.market_id
         AND (m.slot<f.slot OR (m.signature=f.signature AND m.ix_index<f.ix_index))
         ORDER BY m.slot DESC,length(m.observation_sequence) DESC,m.observation_sequence DESC,m.signature DESC,m.ix_index DESC LIMIT 1))";
const FILL_MARK: &str = "(SELECT m.price FROM marks m WHERE m.asset_index=f.asset_index AND m.market_id=f.market_id
         AND (m.slot<f.slot OR (m.signature=f.signature AND m.ix_index<f.ix_index))
         ORDER BY m.slot DESC,length(m.observation_sequence) DESC,m.observation_sequence DESC,m.signature DESC,m.ix_index DESC LIMIT 1)";

pub fn fills(c: &Connection, portfolio: &str, limit: u64, before: Option<&str>) -> Result<Value> {
    let cursor = before.map(fill_cursor).transpose()?;
    let (slot, signature, ix, leg) = cursor.unwrap_or((i64::MAX, String::new(), 0, 0));
    let mut stmt = c.prepare(&format!(
        "SELECT f.signature,f.ix_index,f.leg,f.slot,f.block_time,f.taker_portfolio,f.taker_portfolio_id,
        f.maker_portfolio,f.maker_portfolio_id,f.asset_index,f.market_id,f.size_q,f.reported_price,
        f.limit_price,f.requested_fee_bps,f.status,{FILL_MARK},f.executed_size,f.applied_price
        FROM fills f WHERE (taker_portfolio=?1 OR maker_portfolio=?1)
        AND (?2 IS NULL OR (f.slot,f.signature,f.ix_index,f.leg)<(?3,?4,?5,?6))
        ORDER BY f.slot DESC,f.signature DESC,f.ix_index DESC,f.leg DESC LIMIT ?7",
    ))?;
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
            "executed_size":r.get::<_,Option<String>>(17)?,"applied_price":unsigned(18)?,
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
/// Market-wide executed fills, newest first, with the same cursor as `/fills`.
pub fn trades(c: &Connection, limit: u64, before: Option<&str>) -> Result<Value> {
    let cursor = before.map(fill_cursor).transpose()?;
    let (slot, signature, ix, leg) = cursor.unwrap_or((i64::MAX, String::new(), 0, 0));
    let mut stmt = c.prepare(&format!(
        "SELECT f.signature,f.ix_index,f.leg,f.slot,f.block_time,f.executed_size,f.taker_owner,
        f.taker_portfolio,f.maker_portfolio,f.asset_index,f.reported_price,f.applied_price,{FILL_PRICE}
        FROM fills f WHERE f.status='executed'
        AND (?1 IS NULL OR (f.slot,f.signature,f.ix_index,f.leg)<(?2,?3,?4,?5))
        ORDER BY f.slot DESC,f.signature DESC,f.ix_index DESC,f.leg DESC LIMIT ?6",
    ))?;
    let read = |r: &rusqlite::Row<'_>| -> rusqlite::Result<Value> {
        let size = r
            .get::<_, Option<String>>(5)?
            .and_then(|s| s.parse::<i128>().ok());
        let source = if r.get::<_, Option<String>>(10)?.is_some() {
            "reported"
        } else if r.get::<_, Option<String>>(11)?.is_some() {
            "matcher_oracle"
        } else {
            "last_mark"
        };
        let price = r
            .get::<_, Option<String>>(12)?
            .and_then(|s| s.parse::<u64>().ok());
        Ok(
            json!({"signature":r.get::<_,String>(0)?,"ix_index":r.get::<_,u64>(1)?,"leg":r.get::<_,u64>(2)?,
            "slot":r.get::<_,u64>(3)?,"block_time":r.get::<_,Option<i64>>(4)?,
            "side":if size.is_some_and(|n| n > 0) {"buy"} else {"sell"},
            "size":size.map(|n| n.unsigned_abs().to_string()),
            "price":price,"price_source":if price.is_some() {Value::from(source)} else {Value::Null},
            "wallet":r.get::<_,Option<String>>(6)?,"taker_portfolio":r.get::<_,String>(7)?,
            "maker_portfolio":r.get::<_,String>(8)?,"asset_index":r.get::<_,u64>(9)?}),
        )
    };
    let mut rows = stmt
        .query_map(params![before, slot, signature, ix, leg, limit + 1], read)?
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
    Ok(json!({"trades":rows,"next_before":next,
        "price_note":"reported: NoCpi instruction price; matcher_oracle: the mark the wrapper passed to the matcher (AuthMark markets execute at this mark); last_mark: last pushed mark before the fill (estimate)"}))
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

pub fn cached_leaderboard(c: &Connection, cfg: &Config) -> Result<Value> {
    if let Some(value) = meta(c, "leaderboard_json")? {
        return Ok(serde_json::from_str(&value)?);
    }
    leaderboard(c, cfg)
}

// All arena queries use the same owner resolution, exclusions and season entrants.
fn agent_portfolio(portfolio: &str) -> String {
    format!("SELECT owner FROM (SELECT COALESCE(
        (SELECT owner FROM portfolio_snapshots WHERE portfolio={portfolio} ORDER BY slot DESC LIMIT 1),
        (SELECT signer FROM transfers WHERE portfolio={portfolio} AND kind='deposit' AND ok=1
         AND slot<=?1 ORDER BY slot,signature,ix_index LIMIT 1)) AS owner)
    WHERE {portfolio} NOT IN (SELECT value FROM json_each(?3))
    AND owner IS NOT NULL AND owner NOT IN (SELECT value FROM json_each(?3))
    AND (?2=0 OR EXISTS(SELECT 1 FROM instructions i JOIN txs t ON t.signature=i.signature
        WHERE i.portfolio={portfolio} AND i.name='InitPortfolio' AND t.ok=1))")
}

pub fn leaderboard(c: &Connection, cfg: &Config) -> Result<Value> {
    #[derive(Default)]
    struct Entry {
        equity: i128,
        net_deposits: i128,
        trades: u64,
        open_positions: u64,
        portfolios: Vec<String>,
    }
    let observed_slot = meta(c, "observed_slot")?
        .map(|s| s.parse::<u64>())
        .transpose()?;
    let mut wallets: BTreeMap<String, Entry> = BTreeMap::new();
    let agent = agent_portfolio("a.portfolio");
    let mut stmt = c.prepare(&format!(
        "SELECT a.portfolio,({agent}),COALESCE(s.capital,'0'),COALESCE(s.pnl,'0'),
            COALESCE(s.positions_json,'[]'),c.portfolio IS NOT NULL
        FROM (SELECT portfolio FROM portfolio_snapshots
              UNION SELECT portfolio FROM transfers WHERE ok=1 AND slot<=?1) a
        LEFT JOIN portfolio_snapshots s ON s.portfolio=a.portfolio
            AND s.slot=(SELECT MAX(slot) FROM portfolio_snapshots WHERE portfolio=a.portfolio)
        LEFT JOIN current_portfolios c ON c.portfolio=a.portfolio
        WHERE ?1 IS NOT NULL AND EXISTS({agent}) ORDER BY a.portfolio"
    ))?;
    let excluded = serde_json::to_string(&cfg.leaderboard_exclude)?;
    let rows = stmt.query_map(
        params![observed_slot, cfg.start_slot.is_some(), excluded],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, bool>(5)?,
            ))
        },
    )?;
    for row in rows {
        let (portfolio, owner, capital, pnl, positions, current) = row?;
        let Some(owner) = owner else { continue };
        let entry = wallets.entry(owner).or_default();
        let equity = if current {
            capital
                .parse::<i128>()?
                .checked_add(pnl.parse()?)
                .context("leaderboard equity overflow")?
        } else {
            0
        };
        entry.equity = entry
            .equity
            .checked_add(equity)
            .context("leaderboard equity overflow")?;
        // Withdraw-all pays capital after fees: requested amounts are exact only with
        // maintenance_fee_per_slot = 0 (launch setting; reward epochs reject nonzero).
        // CloseResolved payouts are not transfers; the leaderboard assumes a Live market (the arena resets if the market resolves).
        let mut transfers = c.prepare_cached(
            "SELECT kind,amount FROM transfers WHERE portfolio=?1 AND ok=1 AND slot<=?2",
        )?;
        for transfer in transfers.query_map(params![portfolio, observed_slot], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (kind, amount) = transfer?;
            let amount = amount.parse::<i128>()?;
            entry.net_deposits = if kind == "deposit" {
                entry.net_deposits.checked_add(amount)
            } else {
                entry.net_deposits.checked_sub(amount)
            }
            .context("leaderboard net deposits overflow")?;
        }
        let trades: u64 = c
            .prepare_cached(
                "SELECT COUNT(*) FROM fills WHERE taker_portfolio=?1 OR maker_portfolio=?1",
            )?
            .query_row([&portfolio], |r| r.get(0))?;
        entry.trades = entry
            .trades
            .checked_add(trades)
            .context("leaderboard trades overflow")?;
        if !current {
            continue;
        }
        let positions: Value = serde_json::from_str(&positions)?;
        for leg in positions
            .as_array()
            .context("invalid leaderboard positions")?
        {
            if leg["basis_pos_q"]
                .as_str()
                .context("invalid basis_pos_q")?
                .parse::<i128>()?
                != 0
            {
                entry.open_positions = entry
                    .open_positions
                    .checked_add(1)
                    .context("leaderboard positions overflow")?;
            }
        }
        entry.portfolios.push(portfolio);
    }
    let mut entries = wallets
        .into_iter()
        .map(|(wallet, e)| {
            let pnl = e
                .equity
                .checked_sub(e.net_deposits)
                .context("leaderboard pnl overflow")?;
            Ok((pnl, wallet, e))
        })
        .collect::<Result<Vec<_>>>()?;
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let entries = entries.into_iter().take(100).enumerate().map(|(i, (pnl, wallet, e))| json!({"rank":i+1,"wallet":wallet,"pnl":pnl.to_string(),"equity":e.equity.to_string(),"net_deposits":e.net_deposits.to_string(),"trades":e.trades,"open_positions":e.open_positions,"portfolios":e.portfolios})).collect::<Vec<_>>();
    let slot = meta(c, "last_ingest_slot")?
        .map(|s| s.parse::<u64>())
        .transpose()?;
    Ok(
        json!({"since_slot":cfg.start_slot,"as_of_slot":slot,"note":"devnet, test tokens. PnL = equity (capital + engine PnL at the last indexed change, open positions marked at the last crank) minus net deposits.","entries":entries}),
    )
}

pub fn agent_portfolios(c: &Connection, cfg: &Config) -> Result<BTreeMap<String, String>> {
    let observed = meta(c, "observed_slot")?
        .map(|s| s.parse::<u64>())
        .transpose()?;
    let agent = agent_portfolio("a.portfolio");
    let mut stmt = c.prepare(&format!(
        "SELECT a.portfolio,({agent}) FROM
        (SELECT portfolio FROM portfolio_snapshots UNION SELECT portfolio FROM transfers WHERE ok=1 AND slot<=?1) a
        WHERE ?1 IS NOT NULL AND EXISTS({agent})"
    ))?;
    let rows = stmt.query_map(
        params![
            observed,
            cfg.start_slot.is_some(),
            serde_json::to_string(&cfg.leaderboard_exclude)?
        ],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn cached_recent_fills(c: &Connection, limit: u64) -> Result<Value> {
    let mut value: Value = meta(c, "recent_fills_json")?
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_else(|| json!({"fills":[]}));
    if let Some(fills) = value["fills"].as_array_mut() {
        fills.truncate(limit as usize);
    }
    Ok(value)
}

pub fn recent_fills(c: &Connection, agents: &BTreeMap<String, String>) -> Result<Value> {
    // Each portfolio contributes at most 100 candidates through its ordering index.
    // Resolve historical marks only for the final 100 rows after merging.
    let mut stmt = c.prepare(
        "SELECT signature,ix_index,leg,slot FROM fills
        WHERE taker_portfolio=?1 AND status IN ('executed','requested')
        ORDER BY slot DESC,signature DESC,ix_index DESC,leg DESC LIMIT 100",
    )?;
    let mut candidates = Vec::new();
    for portfolio in agents.keys() {
        for row in stmt.query_map([portfolio], |r| {
            Ok((
                r.get::<_, u64>(3)?,
                r.get::<_, String>(0)?,
                r.get::<_, u64>(1)?,
                r.get::<_, u64>(2)?,
            ))
        })? {
            candidates.push(row?);
        }
    }
    candidates.sort_unstable_by(|a, b| b.cmp(a));
    candidates.truncate(100);
    let mut stmt = c.prepare(&format!(
        "SELECT f.block_time,f.size_q,{FILL_MARK},f.taker_portfolio,f.status
        FROM fills f WHERE f.signature=?1 AND f.ix_index=?2 AND f.leg=?3"
    ))?;
    let mut fills = Vec::new();
    for (slot, signature, ix_index, leg) in candidates {
        let (block_time, size, mark, taker, status) =
            stmt.query_row(params![signature, ix_index, leg], |r| {
                Ok((
                    r.get::<_, Option<i64>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?;
        let size = size.parse::<i128>().ok();
        let mark = mark.and_then(|s| s.parse::<u64>().ok());
        fills.push(json!({"signature":signature,"ix_index":ix_index,"leg":leg,"slot":slot,
            "block_time":block_time,"side":if size.is_some_and(|n| n>0) {"buy"} else {"sell"},
            "size_q":size.map(|n| n.unsigned_abs().to_string()),"mark_price":mark,"wallet":agents.get(&taker),
            "taker_portfolio":taker,"status":status}));
    }
    Ok(json!({"fills":fills}))
}

fn add_notional(total: &mut i128, size: &str, price: &str) -> Result<()> {
    let amount = size
        .parse::<i128>()?
        .checked_abs()
        .and_then(|q| q.checked_mul(price.parse::<i128>().ok()?))
        .and_then(|n| n.checked_div(1_000_000))
        .context("season notional overflow or invalid price")?;
    *total = total
        .checked_add(amount)
        .context("season notional overflow")?;
    Ok(())
}

pub fn cached_season(c: &Connection, cfg: &Config) -> Result<Value> {
    Ok(meta(c, "season_json")?
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_else(|| {
            json!({"season":0,"since_slot":cfg.start_slot,"since":null,
            "agents":0,"trades":0,"volume_usd":"0","open_interest_usd":"0","liquidations":0})
        }))
}

// Same-slot marks pick any one (volume estimate only).
const SEASON_FILLS_SQL: &str = "SELECT COALESCE(f.executed_size,f.size_q),COALESCE(f.reported_price,f.applied_price,
    (SELECT price FROM marks WHERE asset_index=f.asset_index AND market_id=f.market_id AND slot<=f.slot ORDER BY slot DESC LIMIT 1))
    FROM fills f WHERE f.taker_portfolio=?1 AND f.status IN ('executed','requested')";

pub fn season(c: &Connection, cfg: &Config, agents: &BTreeMap<String, String>) -> Result<Value> {
    let mut wallets = std::collections::BTreeSet::new();
    let mut trades = 0u64;
    let mut volume = 0i128;
    let mut oi = 0i128;
    let mut liquidations = 0u64;
    let mut marks = BTreeMap::new();
    let mut fills = c.prepare(SEASON_FILLS_SQL)?;
    let mut positions = c.prepare(
        "SELECT s.positions_json FROM current_portfolios c
        JOIN portfolio_snapshots s ON s.portfolio=c.portfolio
        AND s.slot=(SELECT MAX(slot) FROM portfolio_snapshots WHERE portfolio=c.portfolio)
        WHERE c.portfolio=?1",
    )?;
    for (portfolio, owner) in agents {
        for row in fills.query_map([portfolio], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })? {
            let (size, price) = row?;
            wallets.insert(owner);
            trades += 1;
            if let Some(price) = price.and_then(|s| s.parse::<u64>().ok()) {
                if size.parse::<i128>().is_ok() {
                    add_notional(&mut volume, &size, &price.to_string())?;
                }
            }
        }
        let positions: Option<String> =
            positions.query_row([portfolio], |r| r.get(0)).optional()?;
        if let Some(positions) = positions {
            let positions: Value = serde_json::from_str(&positions)?;
            for leg in positions.as_array().context("invalid season positions")? {
                let asset = leg["asset_index"].as_u64().context("invalid asset_index")?;
                let market = leg["market_id"].as_u64().context("invalid market_id")?;
                let mark = match marks.entry((asset, market)) {
                    std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
                    std::collections::btree_map::Entry::Vacant(e) => e.insert(c.query_row(
                        "SELECT price FROM marks WHERE asset_index=?1 AND market_id=?2
                        ORDER BY slot DESC,length(observation_sequence) DESC,observation_sequence DESC,signature DESC,ix_index DESC LIMIT 1",
                        params![asset, market.to_string()], |r| r.get::<_, Option<String>>(0)
                    ).optional()?.flatten().and_then(|s| s.parse::<u64>().ok())),
                };
                if let Some(mark) = mark {
                    add_notional(
                        &mut oi,
                        leg["basis_pos_q"].as_str().context("invalid basis_pos_q")?,
                        &mark.to_string(),
                    )?;
                }
            }
        }
        liquidations += c.query_row(
            "SELECT COUNT(*) FROM liquidations WHERE portfolio=?1",
            [portfolio],
            |r| r.get::<_, u64>(0),
        )?;
    }
    let since: Option<i64> = c
        .query_row(
            "SELECT block_time FROM txs WHERE (?1 IS NULL OR slot>=?1)
        ORDER BY slot,signature LIMIT 1",
            [cfg.start_slot],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(
        json!({"season":0,"since_slot":cfg.start_slot,"since":since,"agents":wallets.len(),
        "trades":trades,"volume_usd":volume.to_string(),"open_interest_usd":oi.to_string(),"liquidations":liquidations}),
    )
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
    #[test]
    fn leaderboard_aggregates_current_and_closed_portfolios_and_excludes_owners_and_portfolios() {
        let c = memory();
        let (_, mut cfg) = Config::parse(["serve".into()]).unwrap();
        cfg.leaderboard_exclude = vec!["excluded-owner".into(), "excluded-portfolio".into()];
        for (portfolio, owner, capital, pnl, q) in [
            ("a", "alice", "1000", "100", 1),
            ("a2", "alice", "200", "-50", 0),
            ("b", "bob", "500", "-100", -1),
            ("c", "excluded-owner", "9000", "0", 1),
            ("excluded-portfolio", "other", "9000", "0", 1),
            ("closed", "charlie", "9000", "0", 1),
        ] {
            let mut s = snap(1, q);
            s.portfolio = portfolio.into();
            s.owner = "old-owner".into();
            snapshot(&c, &s).unwrap();
            s.slot = 2;
            s.owner = owner.into();
            s.capital = capital.into();
            s.pnl = pnl.into();
            snapshot(&c, &s).unwrap();
            if portfolio != "closed" {
                c.execute("INSERT INTO current_portfolios VALUES(?1)", [portfolio])
                    .unwrap();
            }
        }
        for (i, (portfolio, kind, amount, ok, slot)) in [
            ("a", "deposit", "1000", 1, 1),
            ("a", "withdraw", "100", 1, 2),
            ("a2", "deposit", "200", 1, 1),
            ("a", "deposit", "9999", 0, 2),
            ("b", "deposit", "500", 1, 1),
            ("b", "withdraw", "250", 1, 2),
            ("a", "withdraw", "1000", 1, 3),
            ("a2", "deposit", "500", 1, 3),
            ("closed", "deposit", "1000", 1, 1),
            ("closed", "withdraw", "900", 1, 2),
        ]
        .into_iter()
        .enumerate()
        {
            c.execute("INSERT INTO transfers(signature,ix_index,portfolio,kind,amount,ok,slot) VALUES(?1,0,?2,?3,?4,?5,?6)", params![i.to_string(), portfolio, kind, amount, ok, slot]).unwrap();
        }
        for (i, (taker, maker, status)) in [
            ("a", "b", "executed"),
            ("b", "a2", "requested"),
            ("a", "a", "requested"),
        ]
        .into_iter()
        .enumerate()
        {
            c.execute("INSERT INTO fills(signature,ix_index,leg,taker_portfolio,maker_portfolio,status) VALUES(?1,0,0,?2,?3,?4)", params![i.to_string(), taker, maker, status]).unwrap();
        }
        set_meta(&c, "last_ingest_slot", "123").unwrap();
        set_meta(&c, "observed_slot", "2").unwrap();
        let out = leaderboard(&c, &cfg).unwrap();
        assert_eq!(out["as_of_slot"], 123);
        assert_eq!(
            out["entries"],
            json!([
                {"rank":1,"wallet":"alice","equity":"1250","net_deposits":"1100","pnl":"150","trades":3,"open_positions":1,"portfolios":["a","a2"]},
                {"rank":2,"wallet":"bob","equity":"400","net_deposits":"250","pnl":"150","trades":2,"open_positions":1,"portfolios":["b"]},
                {"rank":3,"wallet":"charlie","equity":"0","net_deposits":"100","pnl":"-100","trades":0,"open_positions":0,"portfolios":[]}
            ])
        );
        assert_eq!(cached_leaderboard(&c, &cfg).unwrap(), out);
        set_meta(&c, "leaderboard_json", &out.to_string()).unwrap();
        assert_eq!(cached_leaderboard(&c, &cfg).unwrap(), out);
        cfg.leaderboard_exclude.clear();
        let out = leaderboard(&c, &cfg).unwrap();
        assert_eq!(out["entries"].as_array().unwrap().len(), 5);
        // The cached path serves the completed pass, even when direct computation changes.
        assert_eq!(
            cached_leaderboard(&c, &cfg).unwrap()["entries"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(out["entries"][0]["wallet"], "excluded-owner");
        assert_eq!(out["entries"][1]["wallet"], "other");
        c.execute(
            "UPDATE portfolio_snapshots SET pnl='-101' WHERE portfolio='b' AND slot=2",
            [],
        )
        .unwrap();
        assert_eq!(leaderboard(&c, &cfg).unwrap()["entries"][3]["pnl"], "149");
    }
    #[test]
    fn leaderboard_counts_deposits_through_unchanged_observations() {
        let c = memory();
        let (_, cfg) = Config::parse(["serve".into()]).unwrap();
        let mut s = snap(1, 0);
        snapshot(&c, &s).unwrap();
        c.execute("INSERT INTO current_portfolios VALUES(?1)", [A])
            .unwrap();
        c.execute("INSERT INTO transfers(signature,ix_index,slot,portfolio,kind,signer,amount,ok) VALUES('initial',0,1,?1,'deposit',?2,'1000',1),('deposit-before-loss',0,2,?1,'deposit',?2,'500',1)", params![A, PROGRAM]).unwrap();
        // The deposit and loss cancel out, so observing again writes no new snapshot.
        s.slot = 3;
        snapshot(&c, &s).unwrap();
        let latest: u64 = c
            .query_row("SELECT MAX(slot) FROM portfolio_snapshots", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(latest, 1);
        set_meta(&c, "observed_slot", "3").unwrap();
        let out = leaderboard(&c, &cfg).unwrap();
        assert_eq!(out["entries"][0]["equity"], "1000");
        assert_eq!(out["entries"][0]["net_deposits"], "1500");
        assert_eq!(out["entries"][0]["pnl"], "-500");
    }
    #[test]
    fn leaderboard_attributes_unsnapshotted_closed_portfolios_to_deposit_signers() {
        let c = memory();
        let (_, cfg) = Config::parse(["serve".into()]).unwrap();
        for (signature, portfolio, slot, kind, signer, amount, ok) in [
            ("failed", "closed", 1, "deposit", "wrong", "9999", 0),
            (
                "earlier-withdraw",
                "closed",
                1,
                "withdraw",
                "delegate",
                "100",
                1,
            ),
            ("first-deposit", "closed", 2, "deposit", "owner", "1000", 1),
            ("later-deposit", "closed", 3, "deposit", "other", "500", 1),
            ("withdraw", "closed", 4, "withdraw", "delegate", "900", 1),
            ("no-deposit", "orphan", 1, "withdraw", "delegate", "100", 1),
            (
                "failed-only",
                "failed-only",
                1,
                "deposit",
                "wrong",
                "100",
                0,
            ),
        ] {
            c.execute("INSERT INTO transfers(signature,ix_index,slot,portfolio,kind,signer,amount,ok) VALUES(?1,0,?2,?3,?4,?5,?6,?7)", params![signature, slot, portfolio, kind, signer, amount, ok]).unwrap();
        }
        set_meta(&c, "observed_slot", "4").unwrap();
        assert_eq!(
            leaderboard(&c, &cfg).unwrap()["entries"],
            json!([{"rank":1,"wallet":"owner","equity":"0","net_deposits":"500","pnl":"-500","trades":0,"open_positions":0,"portfolios":[]}])
        );
    }
    #[test]
    fn leaderboard_ignores_transfers_after_observed_slot() {
        let c = memory();
        let (_, cfg) = Config::parse(["serve".into()]).unwrap();
        for portfolio in ["current", "closed", "unsnapshotted"] {
            if portfolio != "unsnapshotted" {
                let mut s = snap(1, 0);
                s.portfolio = portfolio.into();
                snapshot(&c, &s).unwrap();
            }
            c.execute("INSERT INTO transfers(signature,ix_index,slot,portfolio,kind,signer,amount,ok) VALUES(?1,0,1,?1,'deposit',?2,'1000',1)", params![portfolio, PROGRAM]).unwrap();
        }
        c.execute("INSERT INTO current_portfolios VALUES('current')", [])
            .unwrap();
        set_meta(&c, "observed_slot", "2").unwrap();
        let before = leaderboard(&c, &cfg).unwrap();
        assert_eq!(before["entries"][0]["net_deposits"], "3000");
        for portfolio in ["current", "closed", "unsnapshotted", "future-only"] {
            c.execute("INSERT INTO transfers(signature,ix_index,slot,portfolio,kind,signer,amount,ok) VALUES(?1,1,3,?1,'deposit','future-owner','9999',1),(?1,2,3,?1,'withdraw','delegate','500',1)", [portfolio]).unwrap();
        }
        assert_eq!(leaderboard(&c, &cfg).unwrap(), before);
    }
    #[test]
    fn leaderboard_without_completed_observation_is_empty() {
        let c = memory();
        let (_, cfg) = Config::parse(["serve".into()]).unwrap();
        snapshot(&c, &snap(1, 0)).unwrap();
        c.execute("INSERT INTO current_portfolios VALUES(?1)", [A])
            .unwrap();
        c.execute("INSERT INTO transfers(signature,ix_index,slot,portfolio,kind,signer,amount,ok) VALUES('deposit',0,1,'closed','deposit','owner','1000',1)", []).unwrap();
        set_meta(&c, "last_ingest_slot", "123").unwrap();
        assert_eq!(leaderboard(&c, &cfg).unwrap()["entries"], json!([]));
    }
    #[test]
    fn leaderboard_checks_amount_overflows_and_limits_ranks() {
        let c = memory();
        let (_, mut cfg) = Config::parse(["serve".into()]).unwrap();
        cfg.leaderboard_exclude.clear();
        assert_eq!(leaderboard(&c, &cfg).unwrap()["entries"], json!([]));
        set_meta(&c, "observed_slot", "1").unwrap();
        for i in 0..101 {
            let mut s = snap(1, 0);
            s.portfolio = format!("p{i:03}");
            s.owner = format!("w{i:03}");
            c.execute("INSERT INTO current_portfolios VALUES(?1)", [&s.portfolio])
                .unwrap();
            snapshot(&c, &s).unwrap();
        }
        let out = leaderboard(&c, &cfg).unwrap();
        assert_eq!(out["entries"].as_array().unwrap().len(), 100);
        assert_eq!(out["entries"][99]["rank"], 100);
        assert_eq!(out["entries"][99]["wallet"], "w099");
        c.execute(
            "UPDATE portfolio_snapshots SET capital=?1,pnl='1' WHERE portfolio='p000'",
            [i128::MAX.to_string()],
        )
        .unwrap();
        assert!(leaderboard(&c, &cfg)
            .unwrap_err()
            .to_string()
            .contains("overflow"));
        c.execute(
            "UPDATE portfolio_snapshots SET pnl='0' WHERE portfolio='p000'",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO transfers(signature,ix_index,slot,portfolio,kind,amount,ok) VALUES('withdraw',0,1,'p000','withdraw','1',1)", []).unwrap();
        assert!(leaderboard(&c, &cfg)
            .unwrap_err()
            .to_string()
            .contains("overflow"));
        c.execute("DELETE FROM transfers", []).unwrap();
        c.execute(
            "UPDATE portfolio_snapshots SET owner='w000' WHERE portfolio='p001'",
            [],
        )
        .unwrap();
        assert!(leaderboard(&c, &cfg)
            .unwrap_err()
            .to_string()
            .contains("overflow"));
        c.execute("UPDATE portfolio_snapshots SET capital='0'", [])
            .unwrap();
        for i in 0..2 {
            c.execute("INSERT INTO transfers(signature,ix_index,slot,portfolio,kind,amount,ok) VALUES(?1,0,1,'p000','deposit',?2,1)", params![i.to_string(), i128::MAX.to_string()]).unwrap();
        }
        assert!(leaderboard(&c, &cfg)
            .unwrap_err()
            .to_string()
            .contains("overflow"));
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
    fn executed_cpi_fills_count_in_stats_window_season_and_trades() {
        let c = memory();
        let (_, mut cfg) = Config::parse(["serve".into()]).unwrap();
        cfg.leaderboard_exclude.clear();
        let (_, open_trade) = crate::decode::cpi_tests::compute("trade_cpi_open");
        let (_, close) = crate::decode::cpi_tests::compute("trade_cpi_close");
        insert(&c, &open_trade).unwrap();
        insert(&c, &close).unwrap();
        // An unknown-matcher CPI stays requested: excluded from fills, 24h and /trades.
        let mut unknown = fixture("crank_trade");
        unknown.fills[0].status = "requested";
        unknown.fills[0].executed_size = None;
        insert(&c, &unknown).unwrap();
        let close_time = close.block_time.unwrap();
        let s = stats(&c, &cfg, close_time + 10).unwrap();
        assert_eq!(s["fills"], 2);
        assert_eq!(s["fills_24h"], 2);
        // 2 x 1_000_000 q at 311207 e6 / 1e6.
        assert_eq!(s["volume_24h"], "622414");
        let s = stats(&c, &cfg, close_time + 86_400 - 10).unwrap();
        assert_eq!(
            (s["fills_24h"].clone(), s["volume_24h"].clone()),
            (json!(1), json!("311207"))
        );
        assert_eq!(cached_stats(&c, &cfg).unwrap()["volume_24h"], "0");

        let t = trades(&c, 1, None).unwrap();
        assert_eq!(t["trades"][0]["signature"], close.signature);
        assert_eq!(t["trades"][0]["side"], "sell");
        assert_eq!(t["trades"][0]["size"], "1000000");
        assert_eq!(t["trades"][0]["price"], 311207);
        assert_eq!(t["trades"][0]["price_source"], "matcher_oracle");
        assert_eq!(
            t["trades"][0]["wallet"],
            "ECUyFtvBfRRxdXTgV4Ze4QY6PmhBJKTRgcDCrZq5ZcHQ"
        );
        assert_eq!(
            t["trades"][0]["taker_portfolio"],
            "Gk9KSFvjwf9YZxM9BmkHmNCFwZNC5ndaFZqgLr4qQeVS"
        );
        assert_eq!(t["trades"][0]["block_time"], close_time);
        let next = trades(&c, 5, t["next_before"].as_str()).unwrap();
        assert_eq!(next["trades"].as_array().unwrap().len(), 1);
        assert_eq!(next["trades"][0]["side"], "buy");
        assert!(next["next_before"].is_null());
        // A NoCpi fill keeps its reported price; mark fallback is labelled.
        let nocpi = fixture("crank_trade");
        insert(&c, &nocpi).unwrap();
        c.execute(
            "UPDATE fills SET status='executed',executed_size=size_q WHERE signature=?1",
            [&nocpi.signature],
        )
        .unwrap();
        let all = trades(&c, 10, None).unwrap();
        assert_eq!(all["trades"].as_array().unwrap().len(), 3);
        assert_eq!(all["trades"][2]["price_source"], "reported");
        c.execute(
            "UPDATE fills SET applied_price=NULL WHERE signature=?1",
            [&close.signature],
        )
        .unwrap();
        assert_eq!(
            trades(&c, 1, None).unwrap()["trades"][0]["price"],
            Value::Null
        );
        c.execute("INSERT INTO marks VALUES('m',0,1,0,'300000','1','1')", [])
            .unwrap();
        let t = trades(&c, 1, None).unwrap();
        assert_eq!(
            (
                t["trades"][0]["price"].clone(),
                t["trades"][0]["price_source"].clone()
            ),
            (json!(300000), json!("last_mark"))
        );

        // Season: agent taker volume counts executed CPI fills at the matcher price.
        c.execute("INSERT INTO portfolio_snapshots VALUES('Gk9KSFvjwf9YZxM9BmkHmNCFwZNC5ndaFZqgLr4qQeVS',1,0,'ECUyFtvBfRRxdXTgV4Ze4QY6PmhBJKTRgcDCrZq5ZcHQ','0','0','[]')", []).unwrap();
        set_meta(&c, "observed_slot", "999999999999").unwrap();
        let agents = agent_portfolios(&c, &cfg).unwrap();
        let season = season(&c, &cfg, &agents).unwrap();
        assert_eq!(season["trades"], 2);
        // close falls back to the mark it now resolves (300000) after applied_price was cleared.
        assert_eq!(season["volume_usd"], "611207");
        // /fills exposes the executed size and applied price.
        let f = fills(&c, "Gk9KSFvjwf9YZxM9BmkHmNCFwZNC5ndaFZqgLr4qQeVS", 10, None).unwrap();
        assert_eq!(f["fills"][1]["executed_size"], "1000000");
        assert_eq!(f["fills"][1]["applied_price"], 311207);
        assert_eq!(f["fills"][1]["status"], "executed");
    }
    #[test]
    fn fills_without_executed_size_require_rebuild() {
        let c = memory();
        c.execute_batch("DROP TABLE fills; CREATE TABLE fills(signature TEXT,status TEXT);")
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "indexer-legacy-fills-{}.sqlite",
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
    #[test]
    fn start_slot_is_bound_to_a_fresh_database() {
        let c = memory();
        check_start_slot(&c, None).unwrap();
        assert_eq!(meta(&c, "start_slot").unwrap(), None);
        set_meta(&c, "oldest_signature", "old").unwrap();
        assert!(check_start_slot(&c, Some(100))
            .unwrap_err()
            .to_string()
            .contains("fresh DB"));
        assert_eq!(meta(&c, "start_slot").unwrap(), None);
        let c = memory();
        check_start_slot(&c, Some(100)).unwrap();
        set_meta(&c, "oldest_signature", "new").unwrap();
        check_start_slot(&c, Some(100)).unwrap();
        assert!(check_start_slot(&c, Some(101)).is_err());
        assert!(check_start_slot(&c, None).is_err());
        let c = memory();
        check_start_slot(&c, Some(u64::MAX)).unwrap();
        check_start_slot(&c, Some(u64::MAX)).unwrap();
    }

    fn season_fixture() -> (Connection, Config) {
        let c = memory();
        let (_, mut cfg) = Config::parse(["serve".into()]).unwrap();
        cfg.start_slot = Some(10);
        cfg.leaderboard_exclude = vec!["bot-owner".into(), "excluded".into()];
        set_meta(&c, "observed_slot", "30").unwrap();
        for (portfolio, owner, created, current) in [
            ("a", "alice", true, true),
            ("a2", "alice", true, false),
            ("b", "bob", true, true),
            ("bot", "bot-owner", true, true),
            ("excluded", "other", true, true),
            ("preseason", "old", false, true),
            ("failed-init", "failed", false, true),
        ] {
            let mut s = snap(20, -2_000_000);
            s.portfolio = portfolio.into();
            s.owner = owner.into();
            snapshot(&c, &s).unwrap();
            if current {
                c.execute("INSERT INTO current_portfolios VALUES(?1)", [portfolio])
                    .unwrap();
            }
            if created {
                c.execute(
                    "INSERT INTO txs(signature,slot,block_time,ok) VALUES(?1,10,100,1)",
                    [portfolio],
                )
                .unwrap();
                c.execute("INSERT INTO instructions(signature,ix_index,name,portfolio) VALUES(?1,0,'InitPortfolio',?1)", [portfolio]).unwrap();
            }
        }
        c.execute(
            "INSERT INTO txs(signature,slot,ok) VALUES('failed-init',10,0)",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO instructions(signature,ix_index,name,portfolio) VALUES('failed-init',0,'InitPortfolio','failed-init')", []).unwrap();
        c.execute(
            "INSERT INTO transfers(signature,ix_index,slot,kind,portfolio,signer,amount,ok) VALUES
            ('fallback',0,10,'deposit','fallback','carol','10',1),
            ('failed',0,9,'deposit','fallback','wrong','10',0),
            ('later',0,11,'deposit','fallback','wrong','10',1)",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO txs(signature,slot,ok) VALUES('fallback',10,1)",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO instructions(signature,ix_index,name,portfolio) VALUES('fallback',0,'InitPortfolio','fallback')", []).unwrap();
        c.execute("INSERT INTO marks VALUES('mark',0,11,0,'3000000','1','1'),('latest',0,25,0,'4000000','2','1')", []).unwrap();
        // Newest-first ties exercise signature, instruction and leg ordering.
        for (sig, ix, leg, slot, portfolio, size, status, asset) in [
            ("older", 0, 0, 12, "a", "2000000", "executed", 0),
            ("tie", 0, 0, 20, "b", "-1000000", "executed", 0),
            ("tie", 0, 1, 20, "a2", "1000000", "executed", 0),
            ("tie", 1, 0, 20, "fallback", "-1000000", "requested", 0),
            ("z", 0, 0, 20, "a", "1000000", "requested", 0),
            ("no-mark", 0, 0, 21, "a", "1000000", "executed", 1),
            ("bot-trade", 0, 0, 30, "bot", "999999999", "executed", 0),
            (
                "excluded-trade",
                0,
                0,
                30,
                "excluded",
                "999999999",
                "executed",
                0,
            ),
            (
                "old-trade",
                0,
                0,
                30,
                "preseason",
                "999999999",
                "executed",
                0,
            ),
            (
                "failed-trade",
                0,
                0,
                30,
                "failed-init",
                "999999999",
                "executed",
                0,
            ),
        ] {
            c.execute("INSERT INTO fills(signature,ix_index,leg,slot,block_time,taker_portfolio,asset_index,market_id,size_q,status)
                VALUES(?1,?2,?3,?4,NULL,?5,?6,'1',?7,?8)", params![sig,ix,leg,slot,portfolio,asset,size,status]).unwrap();
        }
        c.execute(
            "UPDATE fills SET reported_price='2000000' WHERE signature!='no-mark'",
            [],
        )
        .unwrap();
        c.execute(
            "UPDATE fills SET reported_price='bad' WHERE signature='no-mark'",
            [],
        )
        .unwrap();
        for portfolio in ["a", "bot", "excluded", "preseason", "failed-init"] {
            c.execute(
                "INSERT INTO liquidations(portfolio,slot,signature) VALUES(?1,30,'liq')",
                [portfolio],
            )
            .unwrap();
        }
        (c, cfg)
    }

    #[test]
    fn recent_fills_order_limit_eligibility_and_taker_side() {
        let (c, cfg) = season_fixture();
        let out = recent_fills(&c, &agent_portfolios(&c, &cfg).unwrap()).unwrap();
        let rows = out["fills"].as_array().unwrap();
        assert_eq!(rows.len(), 6);
        assert_eq!(
            rows.iter()
                .map(|r| (
                    r["signature"].as_str().unwrap(),
                    r["ix_index"].as_u64().unwrap(),
                    r["leg"].as_u64().unwrap()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("no-mark", 0, 0),
                ("z", 0, 0),
                ("tie", 1, 0),
                ("tie", 0, 1),
                ("tie", 0, 0),
                ("older", 0, 0)
            ]
        );
        assert_eq!(
            rows[2],
            json!({"signature":"tie","ix_index":1,"leg":0,"slot":20,"block_time":null,
            "side":"sell","size_q":"1000000","mark_price":3000000,"wallet":"carol","taker_portfolio":"fallback","status":"requested"})
        );
        assert_eq!(rows[0]["mark_price"], Value::Null);
        assert_eq!(rows[1]["side"], "buy");
        assert_eq!(rows[1]["wallet"], "alice");
        assert_eq!(cached_recent_fills(&c, 100).unwrap(), json!({"fills":[]}));
        set_meta(&c, "recent_fills_json", &out.to_string()).unwrap();
        assert_eq!(
            cached_recent_fills(&c, 2).unwrap()["fills"],
            json!(&rows[..2])
        );
        let mut unscoped = cfg.clone();
        unscoped.start_slot = None;
        c.execute(
            "INSERT INTO txs(signature,slot,ok) VALUES('orphan',10,1)",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO instructions(signature,ix_index,name,portfolio) VALUES('orphan',0,'InitPortfolio','orphan')", []).unwrap();
        c.execute("INSERT INTO fills(signature,ix_index,leg,slot,taker_portfolio,size_q,reported_price,status) VALUES('orphan',0,0,40,'orphan','-1000000','999999999','executed')", []).unwrap();
        for cfg in [&cfg, &unscoped] {
            let agents = agent_portfolios(&c, cfg).unwrap();
            assert!(!agents.contains_key("orphan"));
            assert!(recent_fills(&c, &agents).unwrap()["fills"]
                .as_array()
                .unwrap()
                .iter()
                .all(|f| f["taker_portfolio"] != "orphan"));
            assert_eq!(
                season(&c, cfg, &agents).unwrap()["trades"],
                if cfg.start_slot.is_some() { 6 } else { 8 }
            );
        }
        // The per-portfolio candidate query uses a covering ordering index.
        let mut stmt = c
            .prepare(
                "EXPLAIN QUERY PLAN SELECT signature,ix_index,leg,slot FROM fills
            WHERE taker_portfolio=?1 AND status IN ('executed','requested')
            ORDER BY slot DESC,signature DESC,ix_index DESC,leg DESC LIMIT 100",
            )
            .unwrap();
        let plan = stmt
            .query_map(["a"], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            plan.iter().any(|s| s.contains("fills_taker_slot_order")),
            "{plan:?}"
        );
        assert!(!plan.iter().any(|s| s.contains("TEMP B-TREE")), "{plan:?}");
    }

    #[test]
    fn recent_cache_is_bounded_and_bad_prices_are_null() {
        let (c, cfg) = season_fixture();
        for i in 0..150 {
            c.execute("INSERT INTO fills(signature,ix_index,leg,slot,taker_portfolio,size_q,status,asset_index,market_id)
                VALUES(?1,0,0,50,'a','1','executed',0,'1')", [format!("agent-{i:03}")]).unwrap();
            c.execute(
                "INSERT INTO fills(signature,ix_index,leg,slot,taker_portfolio,size_q,status)
                VALUES(?1,0,0,60,'bot','1','executed')",
                [format!("bot-{i:03}")],
            )
            .unwrap();
        }
        c.execute("UPDATE marks SET price='bad'", []).unwrap();
        let feed = recent_fills(&c, &agent_portfolios(&c, &cfg).unwrap()).unwrap();
        let rows = feed["fills"].as_array().unwrap();
        assert_eq!(rows.len(), 100);
        assert_eq!(rows[0]["signature"], "agent-149");
        assert_eq!(rows[99]["signature"], "agent-050");
        assert!(rows.iter().all(|r| r["mark_price"].is_null()));
        set_meta(&c, "recent_fills_json", &feed.to_string()).unwrap();
        c.execute("DELETE FROM fills", []).unwrap();
        assert_eq!(
            cached_recent_fills(&c, 2).unwrap()["fills"],
            json!(&rows[..2])
        );
    }

    #[test]
    fn season_since_uses_slot_order_index() {
        let c = memory();
        let mut stmt = c
            .prepare(
                "EXPLAIN QUERY PLAN SELECT block_time FROM txs WHERE (?1 IS NULL OR slot>=?1)
        ORDER BY slot,signature LIMIT 1",
            )
            .unwrap();
        for start_slot in [None, Some(10u64)] {
            let plan = stmt
                .query_map([start_slot], |r| r.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert!(plan.iter().any(|s| s.contains("txs_slot")), "{plan:?}");
            assert!(!plan.iter().any(|s| s.contains("TEMP B-TREE")), "{plan:?}");
        }
    }

    #[test]
    fn season_totals_and_leaderboard_share_entrants_and_exclusions() {
        let (c, cfg) = season_fixture();
        let board = leaderboard(&c, &cfg).unwrap();
        assert_eq!(board["since_slot"], 10);
        let owners = board["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["wallet"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(owners.len(), 3);
        for owner in ["alice", "bob", "carol"] {
            assert!(owners.contains(&owner));
        }
        let expected = json!({"season":0,"since_slot":10,"since":100,"agents":3,"trades":6,
            "volume_usd":"12000000","open_interest_usd":"16000000","liquidations":1});
        assert_eq!(
            season(&c, &cfg, &agent_portfolios(&c, &cfg).unwrap()).unwrap(),
            expected
        );
        c.execute(
            "UPDATE fills SET reported_price=NULL WHERE signature='no-mark'",
            [],
        )
        .unwrap();
        assert_eq!(
            season(&c, &cfg, &agent_portfolios(&c, &cfg).unwrap()).unwrap(),
            expected
        );
        assert_eq!(stats(&c, &cfg, 100).unwrap()["since_slot"], 10);
        assert_eq!(cached_stats(&c, &cfg).unwrap()["since_slot"], 10);
        assert_eq!(cached_season(&c, &cfg).unwrap()["trades"], 0);
        set_meta(&c, "season_json", &expected.to_string()).unwrap();
        c.execute("DELETE FROM fills", []).unwrap();
        assert_eq!(cached_season(&c, &cfg).unwrap(), expected);
        assert_eq!(
            season(&c, &cfg, &agent_portfolios(&c, &cfg).unwrap()).unwrap()["trades"],
            0
        );
        let mut unscoped = cfg.clone();
        unscoped.start_slot = None;
        assert!(leaderboard(&c, &unscoped).unwrap()["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["wallet"] == "old"));
    }

    #[test]
    fn season_requested_fill_uses_latest_mark_at_or_before_slot() {
        let (c, cfg) = season_fixture();
        c.execute("DELETE FROM fills", []).unwrap();
        c.execute("INSERT INTO fills(signature,ix_index,leg,slot,taker_portfolio,asset_index,market_id,size_q,status)
            VALUES('cpi',0,0,20,'a',0,'1','-2000000','requested')", []).unwrap();
        c.execute(
            "INSERT INTO marks VALUES
            ('before',0,18,0,'5000000','3','1'),
            ('other-market',0,19,0,'9000000','4','2'),
            ('other-asset',0,19,1,'9000000','4','1')",
            [],
        )
        .unwrap();
        let out = season(&c, &cfg, &agent_portfolios(&c, &cfg).unwrap()).unwrap();
        assert_eq!(out["agents"], 1);
        assert_eq!(out["trades"], 1);
        assert_eq!(out["volume_usd"], "10000000");
        assert_eq!(stats(&c, &cfg, 100).unwrap()["fills"], 0);

        c.execute("UPDATE fills SET reported_price='2000000'", [])
            .unwrap();
        assert_eq!(
            season(&c, &cfg, &agent_portfolios(&c, &cfg).unwrap()).unwrap()["volume_usd"],
            "4000000"
        );
        for (size, price) in [("bad", "2000000"), ("2000000", "bad")] {
            c.execute(
                "UPDATE fills SET size_q=?1,reported_price=?2",
                params![size, price],
            )
            .unwrap();
            assert_eq!(
                season(&c, &cfg, &agent_portfolios(&c, &cfg).unwrap()).unwrap()["volume_usd"],
                "0"
            );
        }
    }

    #[test]
    fn season_fill_mark_picks_any_same_slot_mark() {
        let (c, cfg) = season_fixture();
        c.execute("DELETE FROM fills", []).unwrap();
        c.execute("DELETE FROM marks", []).unwrap();
        c.execute("INSERT INTO fills(signature,ix_index,leg,slot,taker_portfolio,asset_index,market_id,size_q,status)
            VALUES('cpi',0,0,20,'a',0,'1','1000000','requested')", []).unwrap();
        for (signature, ix_index, sequence, price) in [
            ("z", 0, "9", "1000000"),
            ("z", 1, "10", "2000000"),
            ("a", 0, "18446744073709551614", "3000000"),
            ("b", 0, "18446744073709551614", "4000000"),
            ("b", 1, "18446744073709551614", "5000000"),
        ] {
            c.execute(
                "INSERT INTO marks VALUES(?1,?2,20,0,?3,?4,'1')",
                params![signature, ix_index, price, sequence],
            )
            .unwrap();
        }
        let out = season(&c, &cfg, &agent_portfolios(&c, &cfg).unwrap()).unwrap();
        assert!(["1000000", "2000000", "3000000", "4000000", "5000000"]
            .contains(&out["volume_usd"].as_str().unwrap()));
    }

    #[test]
    fn season_requested_fill_without_mark_counts_without_volume() {
        let (c, cfg) = season_fixture();
        c.execute("DELETE FROM fills", []).unwrap();
        c.execute("INSERT INTO fills(signature,ix_index,leg,slot,taker_portfolio,asset_index,market_id,size_q,status)
            VALUES('cpi',0,0,10,'a',0,'1','2000000','requested')", []).unwrap();
        let out = season(&c, &cfg, &agent_portfolios(&c, &cfg).unwrap()).unwrap();
        assert_eq!(out["agents"], 1);
        assert_eq!(out["trades"], 1);
        assert_eq!(out["volume_usd"], "0");
    }

    #[test]
    fn season_fill_mark_uses_asset_market_slot_index() {
        let c = memory();
        let mut stmt = c
            .prepare(&format!("EXPLAIN QUERY PLAN {SEASON_FILLS_SQL}"))
            .unwrap();
        let plan = stmt
            .query_map(["a"], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            plan.iter().any(|s| s.contains("marks_asset_market_slot")),
            "{plan:?}"
        );
        assert!(!plan.iter().any(|s| s.contains("SCAN")), "{plan:?}");
        assert!(!plan.iter().any(|s| s.contains("TEMP B-TREE")), "{plan:?}");
    }

    #[test]
    fn season_empty_and_notional_overflow() {
        let c = memory();
        let (_, cfg) = Config::parse(["serve".into()]).unwrap();
        assert_eq!(
            season(&c, &cfg, &agent_portfolios(&c, &cfg).unwrap()).unwrap(),
            cached_season(&c, &cfg).unwrap()
        );
        let mut total = 0;
        add_notional(&mut total, "-2000000", "3000000").unwrap();
        assert_eq!(total, 6000000);
        for (size, price) in [
            (i128::MIN.to_string(), "1"),
            (i128::MAX.to_string(), "2"),
            ("1".into(), "bad"),
        ] {
            assert!(add_notional(&mut total, &size, price).is_err());
        }
        total = i128::MAX;
        assert!(add_notional(&mut total, "1000000", "1").is_err());
    }
    fn now_for_test() -> i64 {
        2_000_000_000
    }
}
