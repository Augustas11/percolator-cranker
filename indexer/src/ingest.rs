use crate::{db, decode, now, Config};
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use percolator::{AssetLifecycleV16, MarketGroupV16HeaderAccount, MarketModeV16};
use percolator_prog::{
    constants::MARKET_GROUP_OFF,
    state::{read_market, read_portfolio},
};
use rusqlite::Connection;
use serde_json::{json, Value};
use solana_client::{
    client_error::ClientErrorKind, rpc_client::RpcClient, rpc_request::RpcRequest,
};
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey};
use std::{collections::BTreeSet, time::Duration};

fn retryable(e: &solana_client::client_error::ClientError) -> bool {
    match e.kind() {
        ClientErrorKind::Reqwest(e) => e
            .status()
            .is_none_or(|status| status.as_u16() == 429 || status.is_server_error()),
        ClientErrorKind::Io(_) => true,
        _ => e.to_string().contains("429") || e.to_string().contains("Too Many Requests"),
    }
}
fn retry<T>(rpc_url: &str, mut f: impl FnMut() -> Result<T>) -> Result<T> {
    let mut delay = Duration::from_millis(500);
    loop {
        match f() {
            Ok(v) => return Ok(v),
            Err(e)
                if e.downcast_ref::<solana_client::client_error::ClientError>()
                    .is_some_and(retryable) =>
            {
                eprintln!(
                    "RPC retry in {:.1}s: {}",
                    delay.as_secs_f64(),
                    crate::redact_rpc(&format!("{e:#}"), rpc_url)
                );
                std::thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_secs(30));
            }
            Err(e) => return Err(anyhow!(crate::redact_rpc(&format!("{e:#}"), rpc_url))),
        }
    }
}
fn signatures(
    rpc: &RpcClient,
    cfg: &Config,
    before: Option<&str>,
    until: Option<&str>,
    min_context_slot: Option<u64>,
) -> Result<Vec<(String, u64)>> {
    let result: Value = retry(&cfg.rpc_url, || {
        rpc.send(
            RpcRequest::GetSignaturesForAddress,
            json!([cfg.program.to_string(),{
        "limit":1000,"before":before,"until":until,"commitment":"finalized","minContextSlot":min_context_slot} ]),
        )
        .map_err(Into::into)
    })?;
    result
        .as_array()
        .context("invalid signatures response")?
        .iter()
        .map(|v| {
            Ok((
                v["signature"]
                    .as_str()
                    .context("missing signature")?
                    .to_owned(),
                v["slot"].as_u64().context("missing signature slot")?,
            ))
        })
        .collect()
}
fn fetch(
    rpc: &RpcClient,
    c: &Connection,
    cfg: &Config,
    signatures: &[(String, u64)],
) -> Result<Vec<decode::Decoded>> {
    let mut rows = Vec::new();
    for (signature, _) in signatures.iter().rev() {
        if db::contains(c, signature)? {
            continue;
        }
        let tx: Value = retry(&cfg.rpc_url, || {
            let result = rpc.send(
                RpcRequest::GetTransaction,
                json!([signature,{
                "encoding":"json","maxSupportedTransactionVersion":0,"commitment":"finalized"}]),
            );
            // Also throttle retries, null responses, and errors; never hammer getTransaction.
            std::thread::sleep(Duration::from_millis(cfg.rpc_delay_ms));
            result.map_err(Into::into)
        })?;
        anyhow::ensure!(
            !tx.is_null(),
            "transaction {signature} unavailable; cursor left pending (RPC history may be pruned)"
        );
        rows.push(decode::decode(
            signature,
            &tx,
            &cfg.program.to_string(),
            &cfg.market.to_string(),
        )?);
    }
    Ok(rows)
}
fn commit(
    c: &mut Connection,
    rows: &[decode::Decoded],
    cursor: Option<(&str, &str)>,
) -> Result<()> {
    let tx = c.transaction()?;
    for row in rows {
        db::insert(&tx, row)?;
    }
    if let Some((key, value)) = cursor {
        db::set_meta(&tx, key, value)?;
    }
    tx.commit()?;
    Ok(())
}
fn backfill(rpc: &RpcClient, c: &mut Connection, cfg: &Config) -> Result<()> {
    let mut before = db::meta(c, "oldest_signature")?;
    loop {
        let mut page = signatures(rpc, cfg, before.as_deref(), None, None)?;
        let newest = page.first().map(|s| s.0.clone());
        let complete = season_page(&mut page, cfg.start_slot);
        let rows = fetch(rpc, c, cfg, &page)?;
        commit_backfill(c, &rows, &page, newest.as_deref(), complete)?;
        if complete {
            return Ok(());
        }
        before = page.last().map(|s| s.0.clone());
    }
}
fn season_page(page: &mut Vec<(String, u64)>, start_slot: Option<u64>) -> bool {
    let len = page.len();
    page.retain(|(_, slot)| start_slot.is_none_or(|start| *slot >= start));
    len == 0 || page.len() < len
}
fn commit_backfill(
    c: &mut Connection,
    rows: &[decode::Decoded],
    page: &[(String, u64)],
    newest: Option<&str>,
    complete: bool,
) -> Result<()> {
    let tx = c.transaction()?;
    for row in rows {
        db::insert(&tx, row)?;
    }
    if let Some(oldest) = page.last() {
        db::set_meta(&tx, "oldest_signature", &oldest.0)?;
    }
    // Keep the tip even when the first page is entirely before the season, so
    // catch-up cannot re-import the discarded history on an empty season DB.
    if let Some(newest) = newest {
        if db::meta(&tx, "newest_signature")?.is_none() {
            db::set_meta(&tx, "newest_signature", newest)?;
        }
    }
    if complete {
        db::set_meta(&tx, "backfill_complete", "true")?;
    }
    tx.commit()?;
    Ok(())
}

fn catch_up(rpc: &RpcClient, c: &mut Connection, cfg: &Config, slot: u64) -> Result<()> {
    let until = db::meta(c, "newest_signature")?;
    let mut before = None;
    let mut pages = Vec::new();
    loop {
        let page = signatures(rpc, cfg, before.as_deref(), until.as_deref(), Some(slot))?;
        if page.is_empty() {
            break;
        }
        before = page.last().map(|s| s.0.clone());
        pages.push(page);
    }
    commit_catch_up(c, cfg, pages, |c, page| fetch(rpc, c, cfg, page))
}

fn commit_catch_up(
    c: &mut Connection,
    cfg: &Config,
    pages: Vec<Vec<(String, u64)>>,
    mut fetch_page: impl FnMut(&Connection, &[(String, u64)]) -> Result<Vec<decode::Decoded>>,
) -> Result<()> {
    let newest = pages.first().map(|p| p[0].0.clone());
    // Commit oldest pages first, but move the tip only once the entire gap is durable.
    let page_count = pages.len();
    for (i, mut page) in pages.into_iter().rev().enumerate() {
        season_page(&mut page, cfg.start_slot);
        let rows = fetch_page(c, &page)?;
        let cursor = if i + 1 == page_count {
            newest.as_deref().map(|s| ("newest_signature", s))
        } else {
            None
        };
        commit(c, &rows, cursor)?;
    }
    Ok(())
}

fn portfolio_keys(
    discovered: impl IntoIterator<Item = Pubkey>,
    current: impl IntoIterator<Item = Pubkey>,
) -> Vec<Pubkey> {
    discovered
        .into_iter()
        .chain(current)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn state(rpc: &RpcClient, c: &Connection, cfg: &Config) -> Result<(u64, Value, Vec<db::Snapshot>)> {
    let response = retry(&cfg.rpc_url, || {
        rpc.get_account_with_commitment(&cfg.market, CommitmentConfig::finalized())
            .map_err(Into::into)
    })?;
    let slot = response.context.slot;
    let account = response.value.context("market account missing")?;
    anyhow::ensure!(
        account.owner == cfg.program,
        "market account has wrong owner"
    );
    let (_, group) = read_market(&account.data).map_err(|e| anyhow!("read_market: {e:?}"))?;
    let raw = account
        .data
        .get(
            MARKET_GROUP_OFF..MARKET_GROUP_OFF + std::mem::size_of::<MarketGroupV16HeaderAccount>(),
        )
        .context("short market header")?;
    let header = bytemuck::pod_read_unaligned::<MarketGroupV16HeaderAccount>(raw);
    let mut oi = serde_json::Map::new();
    for (i, a) in group.assets.iter().enumerate() {
        let lifecycle = match a.lifecycle {
            AssetLifecycleV16::Disabled => continue,
            AssetLifecycleV16::PendingActivation => "pending_activation",
            AssetLifecycleV16::Active => "active",
            AssetLifecycleV16::DrainOnly => "drain_only",
            AssetLifecycleV16::Retired => "retired",
            AssetLifecycleV16::Recovery => "recovery",
        };
        oi.insert(format!("asset_{i}"),json!({"long_q":a.oi_eff_long_q.to_string(),"short_q":a.oi_eff_short_q.to_string(),"lifecycle":lifecycle}));
    }
    let mode = match group.mode {
        MarketModeV16::Live => "live",
        MarketModeV16::Resolved => "resolved",
        MarketModeV16::Recovery => "recovery",
    };
    let summary = json!({"mode":mode,"open_interest":oi,"materialized_portfolio_count":group.materialized_portfolio_count,
        "insurance":header.insurance.get().to_string(),"vault":header.vault.get().to_string()});
    // discovery uses confirmed internally. Use its reusable key/layout filtering, then
    // union with current keys so confirmed closes cannot hide finalized portfolios.
    let discovered = drip::discovery::discover_portfolios(rpc, &cfg.program)?;
    let discovered_keys = discovered
        .iter()
        .filter(|p| {
            p.market_group_id() == Some(cfg.market)
                && p.layout_discriminator() == Some(drip::binding::V16_LAYOUT_DISCRIMINATOR)
        })
        .map(|p| p.pubkey);
    let mut stmt = c.prepare("SELECT portfolio FROM current_portfolios")?;
    let current = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .map(|key| Ok(key?.parse::<Pubkey>()?))
        .collect::<Result<Vec<_>>>()?;
    let keys = portfolio_keys(discovered_keys, current);
    let mut snapshots = Vec::new();
    for chunk in keys.chunks(100) {
        // Raw request permits minContextSlot and returns a slot for this exact account batch.
        let result: Value = retry(&cfg.rpc_url, || {
            rpc.send(
                RpcRequest::GetMultipleAccounts,
                json!([
            chunk.iter().map(ToString::to_string).collect::<Vec<_>>(),
            {"encoding":"base64","commitment":"finalized","minContextSlot":slot}]),
            )
            .map_err(Into::into)
        })?;
        let snapshot_slot = result["context"]["slot"]
            .as_u64()
            .context("missing snapshot slot")?;
        let accounts = result["value"]
            .as_array()
            .context("missing portfolio accounts")?;
        anyhow::ensure!(
            accounts.len() == chunk.len(),
            "incorrect account response length"
        );
        for (key, raw) in chunk.iter().zip(accounts) {
            if raw.is_null() {
                continue;
            }
            if raw["owner"].as_str() != Some(cfg.program.to_string().as_str()) {
                continue;
            }
            let bytes = STANDARD.decode(
                raw["data"][0]
                    .as_str()
                    .context("missing base64 account data")?,
            )?;
            if drip::binding::layout_discriminator(&bytes)
                != Some(drip::binding::V16_LAYOUT_DISCRIMINATOR)
            {
                continue;
            }
            let Ok(p) = read_portfolio(&bytes) else {
                continue;
            };
            if p.provenance_header.market_group_id != cfg.market.to_bytes() {
                continue;
            }
            let positions:Vec<_>=p.legs.iter().filter(|l|l.active!=0).map(|l|json!({"asset_index":l.asset_index.get(),"market_id":l.market_id.get(),
                "side":if l.side==0 {"long"} else {"short"},"basis_pos_q":l.basis_pos_q.get().to_string()})).collect();
            snapshots.push(db::Snapshot {
                portfolio: key.to_string(),
                slot: snapshot_slot,
                taken_at: now(),
                owner: Pubkey::new_from_array(p.owner).to_string(),
                capital: p.capital.get().to_string(),
                pnl: p.pnl.get().to_string(),
                positions: json!(positions),
            });
        }
    }
    Ok((slot, summary, snapshots))
}
pub fn pass(c: &mut Connection, cfg: &Config) -> Result<()> {
    pass_inner(c, cfg).map_err(|e| anyhow!(crate::redact_rpc(&format!("{e:#}"), &cfg.rpc_url)))
}
fn pass_inner(c: &mut Connection, cfg: &Config) -> Result<()> {
    db::check_start_slot(c, cfg.start_slot)?;
    let rpc = RpcClient::new_with_timeout_and_commitment(
        cfg.rpc_url.clone(),
        Duration::from_secs(60),
        CommitmentConfig::finalized(),
    );
    if db::meta(c, "backfill_complete")?.as_deref() != Some("true") {
        backfill(&rpc, c, cfg)?;
    }
    // Capture state first, then ingest landed transactions through that point before
    // applying the liquidation exclusion query. RPC reads never submit transactions.
    let (slot, summary, snapshots) = state(&rpc, c, cfg)?;
    let through_slot = snapshots
        .iter()
        .map(|s| s.slot)
        .max()
        .unwrap_or(slot)
        .max(slot);
    catch_up(&rpc, c, cfg, through_slot)?;
    let tx = c.transaction()?;
    tx.execute("DELETE FROM current_portfolios", [])?;
    for s in &snapshots {
        tx.execute("INSERT INTO current_portfolios VALUES(?1)", [&s.portfolio])?;
        db::snapshot(&tx, s)?;
    }
    db::set_meta(&tx, "market_json", &summary.to_string())?;
    let pass_time = now();
    db::set_meta(&tx, "last_ingest_at", &pass_time.to_string())?;
    db::set_meta(&tx, "last_ingest_slot", &through_slot.to_string())?;
    db::set_meta(&tx, "observed_slot", &through_slot.to_string())?;
    let stats = db::stats(&tx, cfg, pass_time)?;
    db::set_meta(&tx, "stats_json", &stats.to_string())?;
    let leaderboard = db::leaderboard(&tx, cfg)?;
    db::set_meta(&tx, "leaderboard_json", &leaderboard.to_string())?;
    let agents = db::agent_portfolios(&tx, cfg)?;
    let season = db::season(&tx, cfg, &agents)?;
    db::set_meta(&tx, "season_json", &season.to_string())?;
    let recent = db::recent_fills(&tx, &agents)?;
    db::set_meta(&tx, "recent_fills_json", &recent.to_string())?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permanent_errors_fail_without_retry_and_success_returns() {
        let mut attempts = 0;
        let result: Result<()> = retry("https://rpc.invalid/key", || {
            attempts += 1;
            Err(anyhow!("permanent RPC failure"))
        });
        assert!(result.is_err());
        assert_eq!(attempts, 1);
        assert_eq!(retry("https://rpc.invalid/key", || Ok(7)).unwrap(), 7);
        let transport = solana_client::client_error::ClientError::from(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        ));
        assert!(retryable(&transport));
    }
    #[test]
    fn finalized_keys_include_current_portfolios_missing_from_discovery() {
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();
        let c = Pubkey::new_unique();
        let keys = portfolio_keys([a, b], [b, c]);
        assert_eq!(keys.len(), 3);
        for key in [a, b, c] {
            assert!(keys.contains(&key));
        }
        assert_eq!(portfolio_keys([], [c]), vec![c]);
        assert!(portfolio_keys([], []).is_empty());
    }
    #[test]
    fn retry_errors_redact_rpc_url() {
        let url = "https://rpc.invalid/private-key";
        let result: Result<()> = retry(url, || Err(anyhow!("request failed: {url}")));
        assert_eq!(
            format!("{:#}", result.unwrap_err()),
            "request failed: <rpc>"
        );
    }
    #[test]
    fn backfill_page_stops_at_season_boundary_and_commits_kept_rows() {
        let mut c = db::open(std::path::Path::new(":memory:")).unwrap();
        let mut a = crate::decode::tests::fixture("crank");
        a.signature = "new".into();
        a.slot = 11;
        let mut b = crate::decode::tests::fixture("crank");
        b.signature = "boundary".into();
        b.slot = 10;
        let mut page = vec![
            (a.signature.clone(), 11),
            (b.signature.clone(), 10),
            ("old".into(), 9),
        ];
        assert!(season_page(&mut page, Some(10)));
        assert_eq!(page, vec![("new".into(), 11), ("boundary".into(), 10)]);
        commit_backfill(&mut c, &[a, b], &page, Some("new"), true).unwrap();
        assert!(db::contains(&c, "new").unwrap());
        assert!(db::contains(&c, "boundary").unwrap());
        assert!(!db::contains(&c, "old").unwrap());
        assert_eq!(
            db::meta(&c, "backfill_complete").unwrap().as_deref(),
            Some("true")
        );
        assert_eq!(
            db::meta(&c, "oldest_signature").unwrap().as_deref(),
            Some("boundary")
        );
        assert_eq!(
            db::meta(&c, "newest_signature").unwrap().as_deref(),
            Some("new")
        );
        let mut full = vec![("a".into(), 11), ("b".into(), 10)];
        assert!(!season_page(&mut full, Some(10)));
        assert!(!season_page(&mut full, None));
        assert!(season_page(&mut full, Some(12)));
        assert!(full.is_empty());
        assert!(season_page(&mut full, None));
        let mut c = db::open(std::path::Path::new(":memory:")).unwrap();
        commit_backfill(&mut c, &[], &[], Some("preseason-tip"), true).unwrap();
        assert_eq!(
            db::meta(&c, "newest_signature").unwrap().as_deref(),
            Some("preseason-tip")
        );
        assert_eq!(db::meta(&c, "oldest_signature").unwrap(), None);
    }
    #[test]
    fn catch_up_after_preseason_startup_filters_every_page_and_advances_tip() {
        let mut c = db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, mut cfg) = Config::parse(["serve".into()]).unwrap();
        cfg.start_slot = Some(10);
        commit_backfill(&mut c, &[], &[], Some("startup"), true).unwrap();
        for pages in [
            vec![vec![(String::from("still-before"), 9)]],
            vec![
                vec![("new".into(), 12), ("boundary".into(), 10)],
                vec![("old".into(), 9), ("older".into(), 8)],
            ],
        ] {
            let tip = pages[0][0].0.clone();
            commit_catch_up(&mut c, &cfg, pages, |_, page| {
                assert!(page.iter().all(|(_, slot)| *slot >= 10));
                Ok(page
                    .iter()
                    .map(|(signature, slot)| {
                        let mut row = crate::decode::tests::fixture("crank");
                        row.signature = signature.clone();
                        row.slot = *slot;
                        row
                    })
                    .collect())
            })
            .unwrap();
            assert_eq!(db::meta(&c, "newest_signature").unwrap(), Some(tip));
        }
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM txs WHERE slot<10", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
        assert!(db::contains(&c, "new").unwrap());
        assert!(db::contains(&c, "boundary").unwrap());
    }
    #[test]
    fn page_rows_and_cursor_commit_together() {
        let mut c = db::open(std::path::Path::new(":memory:")).unwrap();
        let row = crate::decode::tests::fixture("crank");
        commit(&mut c, &[row], Some(("oldest_signature", "oldest"))).unwrap();
        assert_eq!(
            db::meta(&c, "oldest_signature").unwrap().as_deref(),
            Some("oldest")
        );
        let row = crate::decode::tests::fixture("crank");
        assert!(db::contains(&c, &row.signature).unwrap());
        commit(&mut c, &[row], Some(("newest_signature", "newest"))).unwrap();
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM txs", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            1
        );
    }
}
