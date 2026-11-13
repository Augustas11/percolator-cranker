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
    Leaderboard,
    Season,
    RecentFills {
        limit: u64,
    },
    Fills {
        portfolio: String,
        limit: u64,
        before: Option<String>,
    },
    Positions {
        owner: String,
    },
    Trades {
        limit: u64,
        before: Option<String>,
    },
    NotFound,
}
pub fn parse(url: &str) -> Result<Query> {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    if !matches!(
        path,
        "/health"
            | "/stats"
            | "/leaderboard"
            | "/fills"
            | "/fills/recent"
            | "/season"
            | "/positions"
            | "/trades"
    ) {
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
        "/fills" => &["portfolio", "limit", "before"],
        "/fills/recent" => &["limit"],
        "/positions" => &["owner"],
        "/trades" => &["limit", "before"],
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
    let before = || -> Result<Option<String>> {
        let before = pairs.get("before").map(|s| (*s).to_owned());
        if let Some(value) = &before {
            db::fill_cursor(value)?;
        }
        Ok(before)
    };
    let limit = |max: u64| -> Result<u64> {
        let limit = pairs
            .get("limit")
            .unwrap_or(&"100")
            .parse::<u64>()
            .context("invalid limit")?;
        anyhow::ensure!((1..=max).contains(&limit), "limit must be 1..{max}");
        Ok(limit)
    };
    Ok(match path {
        "/health" => Query::Health,
        "/trades" => Query::Trades {
            limit: limit(500)?,
            before: before()?,
        },
        "/stats" => Query::Stats,
        "/leaderboard" => Query::Leaderboard,
        "/season" => Query::Season,
        "/fills/recent" => {
            let limit = pairs
                .get("limit")
                .unwrap_or(&"20")
                .parse::<u64>()
                .context("invalid limit")?;
            anyhow::ensure!((1..=100).contains(&limit), "limit must be 1..100");
            Query::RecentFills { limit }
        }
        "/positions" => Query::Positions {
            owner: pubkey("owner")?,
        },
        "/fills" => Query::Fills {
            limit: limit(500)?,
            before: before()?,
            portfolio: pubkey("portfolio")?,
        },
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
            Query::Stats => db::cached_stats(c, cfg)?,
            Query::Leaderboard => db::cached_leaderboard(c, cfg)?,
            Query::Season => db::cached_season(c, cfg)?,
            Query::RecentFills { limit } => db::cached_recent_fills(c, limit)?,
            Query::Fills {
                portfolio,
                limit,
                before,
            } => db::fills(c, &portfolio, limit, before.as_deref())?,
            Query::Positions { owner } => db::positions(c, &owner)?,
            Query::Trades { limit, before } => db::trades(c, limit, before.as_deref())?,
            Query::NotFound => return Ok((404, json!({"error":"not found"}))),
        },
    ))
}
pub fn server(cfg: &Config) -> Result<Server> {
    let address: std::net::SocketAddr = cfg.bind.parse().context("INDEXER_BIND must be a loopback socket address; put a reverse proxy in front for any non-local use")?;
    anyhow::ensure!(
        address.ip().is_loopback(),
        "INDEXER_BIND must be loopback; put a reverse proxy in front for any non-local use"
    );
    Server::http(address).map_err(|e| anyhow::anyhow!("HTTP bind {}: {e}", cfg.bind))
}
pub fn serve(cfg: Config) -> Result<()> {
    requests(server(&cfg)?, &cfg);
    Ok(())
}
fn body_status(headers: &[Header]) -> Option<u16> {
    for header in headers {
        if header.field.equiv("Transfer-Encoding") {
            return Some(400);
        }
        if header.field.equiv("Content-Length") {
            match header.value.as_str().parse::<u64>() {
                Ok(0) => {}
                Ok(_) => return Some(413),
                Err(_) => return Some(400),
            }
        }
    }
    None
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
            if let Some(status) = body_status(request.headers()) {
                return Ok((status, json!({"error":"request bodies are not supported"})));
            }
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
    fn nonlocal_binds_and_body_headers_are_rejected() {
        let (_, mut cfg) = Config::parse(["serve".into()]).unwrap();
        for bind in [
            "0.0.0.0:8090",
            "[::]:8090",
            "192.168.1.1:8090",
            "localhost:8090",
        ] {
            cfg.bind = bind.into();
            assert!(server(&cfg)
                .err()
                .unwrap()
                .to_string()
                .contains("reverse proxy"));
        }
        for (key, value, expected) in [
            ("Content-Length", "1", Some(413)),
            ("Content-Length", "0", None),
            ("Content-Length", "bad", Some(400)),
            ("Transfer-Encoding", "chunked", Some(400)),
        ] {
            assert_eq!(
                body_status(&[Header::from_bytes(key, value).unwrap()]),
                expected
            );
        }
        assert_eq!(body_status(&[]), None);
    }
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
    fn trades_query() {
        assert_eq!(
            parse("/trades").unwrap(),
            Query::Trades {
                limit: 100,
                before: None
            }
        );
        let cursor = format!("7.{}.0.0", "1".repeat(64));
        assert_eq!(
            parse(&format!("/trades?limit=500&before={cursor}")).unwrap(),
            Query::Trades {
                limit: 500,
                before: Some(cursor)
            }
        );
        for url in [
            "/trades?limit=0",
            "/trades?limit=501",
            "/trades?before=1.bad.0.0",
            "/trades?portfolio=11111111111111111111111111111111",
            "/trades?limit=1&limit=2",
        ] {
            assert!(parse(url).is_err(), "{url}");
        }
        let c = db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, cfg) = Config::parse(["serve".into()]).unwrap();
        let out = query(
            &c,
            &cfg,
            Query::Trades {
                limit: 1,
                before: None,
            },
        )
        .unwrap()
        .1;
        assert_eq!(out["trades"], json!([]));
        assert!(out["next_before"].is_null());
    }
    #[test]
    fn season_and_recent_queries() {
        assert_eq!(parse("/season").unwrap(), Query::Season);
        assert_eq!(
            parse("/fills/recent").unwrap(),
            Query::RecentFills { limit: 20 }
        );
        assert_eq!(
            parse("/fills/recent?limit=100").unwrap(),
            Query::RecentFills { limit: 100 }
        );
        for url in [
            "/season?limit=1",
            "/season?x=1",
            "/fills/recent?portfolio=x",
            "/fills/recent?before=x",
            "/fills/recent?limit=0",
            "/fills/recent?limit=101",
            "/fills/recent?limit=-1",
            "/fills/recent?limit=no",
            "/fills/recent?limit=1&limit=2",
        ] {
            assert!(parse(url).is_err(), "{url}");
        }
        let c = db::open(std::path::Path::new(":memory:")).unwrap();
        let (_, cfg) = Config::parse(["serve".into()]).unwrap();
        assert_eq!(
            query(&c, &cfg, Query::Season).unwrap().1,
            db::cached_season(&c, &cfg).unwrap()
        );
        assert_eq!(
            query(&c, &cfg, Query::RecentFills { limit: 20 }).unwrap().1,
            json!({"fills":[]})
        );
    }
    #[test]
    fn query_validation() {
        let p = "11111111111111111111111111111111";
        assert_eq!(parse("/leaderboard").unwrap(), Query::Leaderboard);
        assert_eq!(
            parse(&format!("/fills?portfolio={p}")).unwrap(),
            Query::Fills {
                portfolio: p.into(),
                limit: 100,
                before: None
            }
        );
        assert_eq!(
            parse(&format!("/fills?portfolio={p}&limit=500&before=7.1111111111111111111111111111111111111111111111111111111111111111.0.0")).unwrap(),
            Query::Fills {
                portfolio: p.into(),
                limit: 500,
                before: Some(format!("7.{}.0.0", "1".repeat(64)))
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
            "/leaderboard?x=1",
        ] {
            assert!(parse(url).is_err(), "{url}");
        }
        for tail in [
            "limit=0",
            "limit=501",
            "limit=-1",
            "limit=no",
            "limit=2&limit=3",
            "before=-1",
            "before=1.bad.0.0",
            "before=01.111.0.0",
            "before=18446744073709551615",
            "before_slot=7",
        ] {
            assert!(
                parse(&format!("/fills?portfolio={p}&{tail}")).is_err(),
                "{tail}"
            );
        }
        assert_eq!(parse("/missing").unwrap(), Query::NotFound);
    }
}
