use crate::{db, now, Config};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use solana_sdk::pubkey::Pubkey;
use std::{collections::BTreeMap, str::FromStr, time::Duration};
use tiny_http::{Header, Method, Response, Server, StatusCode};

pub fn cors<'a>(origin: Option<&'a str>, allowed: &[String]) -> Option<&'a str> {
    origin.filter(|o| allowed.iter().any(|a| a == o))
}
#[derive(Debug, PartialEq)]
pub enum Query {
    Health,
    Stats,
    Fills {
        portfolio: String,
        limit: u64,
        before: Option<u64>,
    },
    Positions {
        owner: String,
    },
    NotFound,
}
pub fn parse(url: &str) -> Result<Query> {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    if !matches!(path, "/health" | "/stats" | "/fills" | "/positions") {
        return Ok(Query::NotFound);
    }
    let mut pairs = BTreeMap::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').context("expected query key=value")?;
        anyhow::ensure!(
            pairs.insert(k, v).is_none(),
            "duplicate query parameter: {k}"
        );
    }
    let allowed: &[&str] = match path {
        "/fills" => &["portfolio", "limit", "before_slot"],
        "/positions" => &["owner"],
        _ => &[],
    };
    for key in pairs.keys() {
        anyhow::ensure!(allowed.contains(key), "unknown query parameter: {key}");
    }
    let pubkey = |key| -> Result<String> {
        let value = pairs.get(key).context(format!("missing {key}"))?;
        Pubkey::from_str(value).with_context(|| format!("invalid {key} pubkey"))?;
        Ok((*value).into())
    };
    Ok(match path {
        "/health" => Query::Health,
        "/stats" => Query::Stats,
        "/positions" => Query::Positions {
            owner: pubkey("owner")?,
        },
        "/fills" => {
            let limit = pairs
                .get("limit")
                .unwrap_or(&"100")
                .parse::<u64>()
                .context("invalid limit")?;
            anyhow::ensure!((1..=500).contains(&limit), "limit must be 1..500");
            let before = pairs
                .get("before_slot")
                .map(|s| s.parse::<u64>())
                .transpose()
                .context("invalid before_slot")?;
            anyhow::ensure!(
                before.is_none_or(|n| n <= i64::MAX as u64),
                "before_slot exceeds SQLite range"
            );
            Query::Fills {
                portfolio: pubkey("portfolio")?,
                limit,
                before,
            }
        }
        _ => unreachable!(),
    })
}
fn query(c: &Connection, cfg: &Config, q: Query) -> Result<(u16, Value)> {
    Ok((
        200,
        match q {
            Query::Health => {
                let at = db::meta(c, "last_ingest_at")?
                    .map(|s| s.parse::<i64>())
                    .transpose()?;
                let slot = db::meta(c, "last_ingest_slot")?
                    .map(|s| s.parse::<u64>())
                    .transpose()?;
                let lag = at.map(|t| now().saturating_sub(t).max(0) as u64);
                json!({"ok":lag.is_some_and(|l|l<=cfg.poll_secs.saturating_mul(3)),"last_ingest_at":at,"last_ingest_slot":slot,
                "backfill_complete":db::meta(c,"backfill_complete")?.as_deref()==Some("true"),"lag_secs":lag})
            }
            Query::Stats => db::stats(c, cfg, now())?,
            Query::Fills {
                portfolio,
                limit,
                before,
            } => db::fills(c, &portfolio, limit, before)?,
            Query::Positions { owner } => db::positions(c, &owner)?,
            Query::NotFound => return Ok((404, json!({"error":"not found"}))),
        },
    ))
}
pub fn server(cfg: &Config) -> Result<Server> {
    Server::http(&cfg.bind).map_err(|e| anyhow::anyhow!("HTTP bind {}: {e}", cfg.bind))
}
pub fn serve(cfg: Config) -> Result<()> {
    requests(server(&cfg)?, &cfg);
    Ok(())
}
pub fn requests(server: Server, cfg: &Config) {
    for request in server.incoming_requests() {
        let origin = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Origin"))
            .map(|h| h.value.as_str());
        let allowed = cors(origin, &cfg.cors_origins).map(str::to_owned);
        let result: Result<(u16, Value)> = (|| {
            if request.method() == &Method::Options {
                if allowed.is_none() {
                    return Ok((403, json!({"error":"origin not allowed"})));
                }
                return match parse(request.url()) {
                    Ok(Query::NotFound) => Ok((404, json!({"error":"not found"}))),
                    Err(e) => Ok((400, json!({"error":e.to_string()}))),
                    _ => Ok((204, Value::Null)),
                };
            }
            if request.method() != &Method::Get {
                return Ok((405, json!({"error":"GET only"})));
            }
            let q = match parse(request.url()) {
                Ok(q) => q,
                Err(e) => return Ok((400, json!({"error":e.to_string()}))),
            };
            if q == Query::NotFound {
                return Ok((404, json!({"error":"not found"})));
            }
            let c = Connection::open_with_flags(&cfg.db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            c.busy_timeout(Duration::from_secs(5))?;
            // All route queries see one consistent WAL snapshot.
            c.execute_batch("BEGIN")?;
            query(&c, cfg, q)
        })();
        let (status, body) = match result {
            Ok(v) => v,
            Err(e) => {
                eprintln!("API: {e:#}");
                (500, json!({"error":"database unavailable"}))
            }
        };
        let mut response = Response::from_string(if status == 204 {
            String::new()
        } else {
            body.to_string()
        })
        .with_status_code(StatusCode(status))
        .with_header(Header::from_bytes("Content-Type", "application/json").unwrap());
        if let Some(origin) = allowed {
            if let Ok(header) = Header::from_bytes("Access-Control-Allow-Origin", origin) {
                response.add_header(header);
            }
            response.add_header(Header::from_bytes("Vary", "Origin").unwrap());
            if request.method() == &Method::Options {
                response.add_header(
                    Header::from_bytes("Access-Control-Allow-Methods", "GET, OPTIONS").unwrap(),
                );
            }
        }
        if status == 405 {
            response.add_header(Header::from_bytes("Allow", "GET, OPTIONS").unwrap());
        }
        if let Err(e) = request.respond(response) {
            eprintln!("HTTP response: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_cors_only() {
        let allowed = vec!["http://localhost:5173".into()];
        assert_eq!(
            cors(Some("http://localhost:5173"), &allowed),
            Some("http://localhost:5173")
        );
        for origin in [
            None,
            Some("null"),
            Some("http://localhost:5173.evil"),
            Some("*"),
        ] {
            assert_eq!(cors(origin, &allowed), None);
        }
    }
    #[test]
    fn query_validation() {
        let p = "11111111111111111111111111111111";
        assert_eq!(
            parse(&format!("/fills?portfolio={p}")).unwrap(),
            Query::Fills {
                portfolio: p.into(),
                limit: 100,
                before: None
            }
        );
        assert_eq!(
            parse(&format!("/fills?portfolio={p}&limit=500&before_slot=7")).unwrap(),
            Query::Fills {
                portfolio: p.into(),
                limit: 500,
                before: Some(7)
            }
        );
        assert_eq!(
            parse(&format!("/positions?owner={p}")).unwrap(),
            Query::Positions { owner: p.into() }
        );
        for url in [
            "/fills",
            "/positions?owner=bad",
            "/fills?portfolio=bad",
            "/stats?x=1",
        ] {
            assert!(parse(url).is_err(), "{url}");
        }
        for tail in [
            "limit=0",
            "limit=501",
            "limit=-1",
            "limit=no",
            "limit=2&limit=3",
            "before_slot=-1",
            "before_slot=18446744073709551615",
        ] {
            assert!(
                parse(&format!("/fills?portfolio={p}&{tail}")).is_err(),
                "{tail}"
            );
        }
        assert_eq!(parse("/missing").unwrap(), Query::NotFound);
    }
}
