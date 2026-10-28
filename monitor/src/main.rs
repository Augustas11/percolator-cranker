mod chain;
mod keeper_log;
mod metrics;

use anyhow::{anyhow, bail, Context, Result};
use keeper_log::KeeperLog;
use serde_json::{json, Value};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey, signature::Signature};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Args {
    program: Pubkey,
    market: Pubkey,
    rpc: String,
    log: Option<PathBuf>,
    listen: SocketAddr,
    interval: u64,
    window: u64,
    json: bool,
}
impl Args {
    fn parse() -> Result<Self> {
        let mut program = std::env::var("PERCOLATOR_PROGRAM_ID").ok();
        let (mut market, mut log) = (None::<Pubkey>, None::<PathBuf>);
        let mut rpc = "http://127.0.0.1:8899".to_string();
        let mut listen = "127.0.0.1:9184".to_string();
        let (mut interval, mut window, mut json) = (5000, 9000, false);
        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            if a == "--json" {
                json = true;
                continue;
            }
            if ![
                "--program",
                "--market",
                "--rpc",
                "--keeper-log",
                "--listen",
                "--interval-ms",
                "--expiry-warn-slots",
            ]
            .contains(&a.as_str())
            {
                bail!("unknown option {a}");
            }
            let v = args
                .next()
                .with_context(|| format!("missing value for {a}"))?;
            match a.as_str() {
                "--program" => program = Some(v),
                "--market" => market = Some(v.parse()?),
                "--rpc" => rpc = v,
                "--keeper-log" => log = Some(v.into()),
                "--listen" => listen = v,
                "--interval-ms" => interval = v.parse()?,
                "--expiry-warn-slots" => window = v.parse()?,
                _ => unreachable!(),
            }
        }
        if interval == 0 {
            bail!("--interval-ms must be positive");
        }
        if market.is_none() {
            if let Some(path) = &log {
                let mut keeper = KeeperLog::default();
                keeper
                    .read(path)
                    .context("discover market from keeper log")?;
                market = keeper.market.map(|pk| pk.parse()).transpose()?;
            }
        }
        Ok(Self {
            program: program
                .context("--program or PERCOLATOR_PROGRAM_ID is required")?
                .parse()?,
            market: market.context("--market or a market line in --keeper-log is required")?,
            rpc,
            log,
            listen: listen.parse()?,
            interval,
            window,
            json,
        })
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn poll(
    args: &Args,
    rpc: &RpcClient,
    keeper: &mut KeeperLog,
    cached_sig: &mut Option<String>,
    previous: &Value,
) -> Value {
    let mut s = match chain::fetch(rpc, &args.program, &args.market, args.window) {
        Ok(mut s) => {
            s["chain_read_ok"] = json!(1);
            s["chain_error"] = Value::Null;
            s
        }
        Err(e) => {
            let mut s = previous.clone();
            s["chain_read_ok"] = json!(0);
            s["chain_error"] = json!(format!("{e:#}"));
            s
        }
    };
    s["market"] = json!(args.market.to_string());
    s["snapshot_unix_seconds"] = json!(now());
    if let Some(path) = &args.log {
        let result = keeper.read(path);
        // Look up the latest landed-crank signature's slot until one lookup succeeds, then
        // cache it; the previous slot stays in place meanwhile. History search, so a
        // monitor restarted on an old log still finds it.
        if keeper.signature != *cached_sig {
            let found = keeper
                .signature
                .as_ref()
                .and_then(|s| s.parse::<Signature>().ok())
                .and_then(|sig| rpc.get_signature_statuses_with_history(&[sig]).ok())
                .and_then(|status| status.value.first().cloned().flatten())
                .map(|v| v.slot);
            if found.is_some() {
                keeper.last_success_slot = found;
                *cached_sig = keeper.signature.clone();
            }
        }
        let slot = s["chain_slot"].as_u64();
        if keeper.first_seen_slot.is_none() {
            keeper.first_seen_slot = slot;
        }
        let (ok, age, error) = match result {
            Ok(age) => (true, Some(age), None),
            Err(e) => (false, None, Some(e.to_string())),
        };
        s["keeper"] = keeper.snapshot(slot.unwrap_or(0), ok, age, error);
    }
    s
}

fn serve(mut stream: TcpStream, s: &Value) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut line = String::new();
    BufReader::new(&mut stream)
        .take(8192)
        .read_line(&mut line)?;
    let parts: Vec<_> = line.split_whitespace().collect();
    let method = parts.first().copied().unwrap_or("");
    let path = parts.get(1).copied().unwrap_or("");
    let (status, content_type, body) = if method != "GET" && method != "HEAD" {
        (
            "405 Method Not Allowed",
            "text/plain",
            "GET/HEAD only\n".to_string(),
        )
    } else {
        match path {
            "/" => (
                "200 OK",
                "text/html; charset=utf-8",
                include_str!("../dashboard.html").to_string(),
            ),
            "/metrics" => ("200 OK", "text/plain; version=0.0.4", metrics::render(s)),
            "/snapshot.json" => (
                "200 OK",
                "application/json",
                serde_json::to_string_pretty(s)?,
            ),
            _ => ("404 Not Found", "text/plain", "Not found\n".into()),
        }
    };
    write!(stream,"HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len())?;
    if method != "HEAD" {
        stream.write_all(body.as_bytes())?;
    }
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse()?;
    let rpc = RpcClient::new_with_timeout_and_commitment(
        args.rpc.clone(),
        Duration::from_secs(20),
        CommitmentConfig::confirmed(),
    );
    let mut keeper = KeeperLog::default();
    let mut sig = None;
    let initial = poll(&args, &rpc, &mut keeper, &mut sig, &json!({}));
    if args.json {
        println!("{}", serde_json::to_string_pretty(&initial)?);
        return Ok(());
    }
    if !args.listen.ip().is_loopback() {
        eprintln!("warning: monitor listening on non-loopback {}", args.listen);
    }
    let listener = TcpListener::bind(args.listen)?;
    let state = Arc::new(RwLock::new(initial));
    let shared = Arc::clone(&state);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(args.interval));
        let previous = shared.read().unwrap().clone();
        let s = poll(&args, &rpc, &mut keeper, &mut sig, &previous);
        *shared.write().unwrap() = s;
    });
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let snapshot = state
                    .read()
                    .map_err(|_| anyhow!("snapshot lock poisoned"))?
                    .clone();
                if let Err(e) = serve(stream, &snapshot) {
                    eprintln!("HTTP: {e}");
                }
            }
            Err(e) => eprintln!("accept: {e}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Shutdown;
    #[test]
    fn chain_failure_is_nonfatal_and_preserves_last_chain_and_keeper_data() {
        let path = std::env::temp_dir().join(format!("monitor-poll-{}", std::process::id()));
        std::fs::write(
            &path,
            "cycle 9 summary: progressed=1 idle=0 failed=0 cu=42\n",
        )
        .unwrap();
        let args = Args {
            program: Pubkey::new_unique(),
            market: Pubkey::new_unique(),
            rpc: String::new(),
            log: Some(path.clone()),
            listen: "127.0.0.1:0".parse().unwrap(),
            interval: 5000,
            window: 9000,
            json: false,
        };
        let mut keeper = KeeperLog::default();
        let mut signature = None;
        let previous = json!({"chain_slot":100,"mode":"live"});
        let snapshot = poll(
            &args,
            &RpcClient::new_mock("fails"),
            &mut keeper,
            &mut signature,
            &previous,
        );
        assert_eq!(snapshot["chain_read_ok"], 0);
        assert!(snapshot["chain_error"].as_str().is_some());
        assert_eq!(snapshot["chain_slot"], 100);
        assert_eq!(snapshot["keeper"]["last_cycle"], 9);
        std::fs::remove_file(path).unwrap();
        let snapshot = poll(
            &args,
            &RpcClient::new_mock("fails"),
            &mut keeper,
            &mut signature,
            &snapshot,
        );
        assert_eq!(snapshot["keeper"]["log_ok"], false);
        assert_eq!(snapshot["keeper"]["last_cycle"], 9);
    }

    #[test]
    fn http_routes_head_and_methods() {
        for (request, status, content) in [
            ("GET /snapshot.json HTTP/1.1\r\n\r\n", "200 OK", true),
            ("HEAD /metrics HTTP/1.1\r\n\r\n", "200 OK", false),
            ("GET /missing HTTP/1.1\r\n\r\n", "404 Not Found", true),
            ("POST / HTTP/1.1\r\n\r\n", "405 Method Not Allowed", true),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                serve(listener.accept().unwrap().0, &json!({"market":"m"})).unwrap()
            });
            let mut client = TcpStream::connect(address).unwrap();
            client.write_all(request.as_bytes()).unwrap();
            client.shutdown(Shutdown::Write).unwrap();
            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            server.join().unwrap();
            assert!(response.starts_with(&format!("HTTP/1.1 {status}")));
            let (headers, body) = response.split_once("\r\n\r\n").unwrap();
            assert!(headers.contains("Connection: close"));
            assert_eq!(!body.is_empty(), content);
        }
    }
}
