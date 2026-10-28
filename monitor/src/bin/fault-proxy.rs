//! Test-only forward proxy. The monitor binary never submits transactions.
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use solana_sdk::{pubkey::Pubkey, transaction::Transaction};
use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

fn should_fault(request: &Value, faults: &HashSet<Pubkey>) -> bool {
    if request["method"] != "sendTransaction" {
        return false;
    }
    let Some(encoded) = request["params"][0].as_str() else {
        return false;
    };
    let Ok(bytes) = STANDARD.decode(encoded) else {
        return false;
    };
    let Ok(tx) = bincode::deserialize::<Transaction>(&bytes) else {
        return false;
    };
    tx.message.account_keys.iter().any(|pk| faults.contains(pk))
}

fn fault_response(id: &Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32002,
        "message":"Transaction simulation failed: injected fault",
        "data":{"err":{"InstructionError":[2,{"Custom":4242}]},
            "logs":["injected fault (fault-proxy)"],"accounts":null,"unitsConsumed":0,
            "returnData":null,"innerInstructions":null}}})
}

fn read_http(stream: &mut TcpStream, response: bool) -> Result<(String, Vec<u8>)> {
    let mut r = BufReader::new(stream);
    let mut first = String::new();
    r.read_line(&mut first)?;
    let mut length = None;
    let mut chunked = false;
    let mut headers = first.len();
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            bail!("truncated HTTP headers");
        }
        headers += line.len();
        if headers > 65536 {
            bail!("HTTP headers too large");
        }
        if line == "\r\n" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                length = Some(v.trim().parse::<usize>()?);
            }
            if k.eq_ignore_ascii_case("transfer-encoding") {
                chunked = v.trim().eq_ignore_ascii_case("chunked");
            }
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            let mut size = String::new();
            r.read_line(&mut size)?;
            let n = usize::from_str_radix(size.trim().split(';').next().unwrap_or(""), 16)?;
            if n == 0 {
                break;
            }
            if body.len().saturating_add(n) > 32 * 1024 * 1024 {
                bail!("HTTP body too large");
            }
            let offset = body.len();
            body.resize(offset + n, 0);
            r.read_exact(&mut body[offset..])?;
            let mut crlf = [0; 2];
            r.read_exact(&mut crlf)?;
            if crlf != *b"\r\n" {
                bail!("invalid chunk terminator");
            }
        }
    } else if let Some(n) = length {
        if n > 32 * 1024 * 1024 {
            bail!("HTTP body too large");
        }
        body.resize(n, 0);
        r.read_exact(&mut body)?;
    } else if response {
        r.take(32 * 1024 * 1024 + 1).read_to_end(&mut body)?;
        if body.len() > 32 * 1024 * 1024 {
            bail!("HTTP body too large");
        }
    }
    Ok((first.trim_end().into(), body))
}

fn reply(client: &mut TcpStream, status: &str, body: &[u8]) -> Result<()> {
    write!(client,"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len())?;
    client.write_all(body)?;
    Ok(())
}

fn handle(client: &mut TcpStream, upstream: &str, faults_path: &Path) -> Result<()> {
    client.set_read_timeout(Some(Duration::from_secs(30)))?;
    client.set_write_timeout(Some(Duration::from_secs(30)))?;
    let (_, body) = read_http(client, false)?;
    let faults: HashSet<Pubkey> = match std::fs::read_to_string(faults_path) {
        Ok(text) => text.lines().filter_map(|s| s.trim().parse().ok()).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashSet::new(),
        Err(e) => return Err(e.into()),
    };
    if let Ok(request) = serde_json::from_slice::<Value>(&body) {
        if should_fault(&request, &faults) {
            eprintln!(
                "fault-proxy: injected sendTransaction failure id={}",
                request["id"]
            );
            return reply(
                client,
                "200 OK",
                fault_response(&request["id"]).to_string().as_bytes(),
            );
        }
    }
    let address = upstream
        .strip_prefix("http://")
        .context("upstream must use plain http://")?;
    let (host, path) = address
        .split_once('/')
        .map(|(h, p)| (h, format!("/{p}")))
        .unwrap_or((address, "/".into()));
    let authority = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:80")
    };
    let mut server = TcpStream::connect(authority)?;
    server.set_read_timeout(Some(Duration::from_secs(30)))?;
    server.set_write_timeout(Some(Duration::from_secs(30)))?;
    write!(server,"POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len())?;
    server.write_all(&body)?;
    let (status, body) = read_http(&mut server, true)?;
    let status = status
        .split_once(' ')
        .context("invalid upstream HTTP status")?
        .1;
    reply(client, status, &body)
}

fn main() -> Result<()> {
    let mut listen = "127.0.0.1:9185".to_string();
    let mut upstream = None;
    let mut faults = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let v = args
            .next()
            .with_context(|| format!("missing value for {a}"))?;
        match a.as_str() {
            "--listen" => listen = v,
            "--upstream" => upstream = Some(v),
            "--faults" => faults = Some(PathBuf::from(v)),
            _ => bail!("unknown option {a}"),
        }
    }
    let upstream = upstream.context("--upstream required")?;
    let faults = faults.context("--faults required")?;
    if !upstream.starts_with("http://") {
        bail!("upstream must use plain http://");
    }
    for stream in TcpListener::bind(listen)?.incoming() {
        let mut client = stream?;
        let upstream = upstream.clone();
        let faults = faults.clone();
        std::thread::spawn(move || {
            if let Err(e) = handle(&mut client, &upstream, &faults) {
                eprintln!("fault-proxy: {e:#}");
                let _ = reply(
                    &mut client,
                    "502 Bad Gateway",
                    br#"{"error":"proxy upstream failure"}"#,
                );
            }
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::{
        instruction::Instruction, message::Message, signature::Keypair, signer::Signer,
    };
    #[test]
    fn constructed_transaction_fault_and_rpc_error_shape() {
        let payer = Keypair::new();
        let program = Pubkey::new_unique();
        let tx = Transaction::new_unsigned(Message::new(
            &[Instruction::new_with_bytes(program, &[], vec![])],
            Some(&payer.pubkey()),
        ));
        let request = json!({"jsonrpc":"2.0","id":42,"method":"sendTransaction","params":[STANDARD.encode(bincode::serialize(&tx).unwrap())]});
        assert!(should_fault(&request, &HashSet::from([payer.pubkey()])));
        assert!(should_fault(&request, &HashSet::from([program])));
        assert!(!should_fault(
            &request,
            &HashSet::from([Pubkey::new_unique()])
        ));
        assert!(!should_fault(
            &json!([request.clone()]),
            &HashSet::from([payer.pubkey()])
        ));
        assert!(!should_fault(
            &json!({"method":"getSlot"}),
            &HashSet::from([payer.pubkey()])
        ));
        let response = fault_response(&request["id"]);
        let result: solana_client::rpc_response::RpcSimulateTransactionResult =
            serde_json::from_value(response["error"]["data"].clone()).unwrap();
        assert!(result.err.is_some());
        assert_eq!(response["id"], 42);
    }
    #[test]
    fn solana_client_recognizes_injected_preflight_failure() {
        use solana_client::{
            client_error::ClientErrorKind,
            rpc_client::RpcClient,
            rpc_request::{RpcError, RpcRequest, RpcResponseErrorData},
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (_, body) = read_http(&mut stream, false).unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap();
            reply(
                &mut stream,
                "200 OK",
                fault_response(&request["id"]).to_string().as_bytes(),
            )
            .unwrap();
        });
        let client =
            RpcClient::new_with_timeout(format!("http://{address}"), Duration::from_secs(5));
        let error = client
            .send::<Value>(RpcRequest::SendTransaction, json!(["test"]))
            .unwrap_err();
        assert!(matches!(
            error.kind(),
            ClientErrorKind::RpcError(RpcError::RpcResponseError {
                code: -32002,
                data: RpcResponseErrorData::SendTransactionPreflightFailure(_),
                ..
            })
        ));
        server.join().unwrap();
    }

    #[test]
    fn forwards_absolute_uri_and_chunked_response() {
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = upstream.accept().unwrap();
            let (line, body) = read_http(&mut stream, false).unwrap();
            assert_eq!(line, "POST /rpc HTTP/1.1");
            assert_eq!(body, br#"[{"method":"getSlot"}]"#);
            stream.write_all(b"HTTP/1.1 201 Created\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n2\r\n{}\r\n0\r\n\r\n").unwrap();
        });
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let handler = std::thread::spawn(move || {
            handle(
                &mut proxy.accept().unwrap().0,
                &format!("http://{upstream_addr}/rpc"),
                &std::env::temp_dir()
                    .join(format!("missing-monitor-faults-{}", std::process::id())),
            )
            .unwrap();
        });
        let mut client = TcpStream::connect(proxy_addr).unwrap();
        let body = br#"[{"method":"getSlot"}]"#;
        write!(
            client,
            "POST http://127.0.0.1:8899/ HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .unwrap();
        client.write_all(body).unwrap();
        let (status, body) = read_http(&mut client, true).unwrap();
        assert_eq!(status, "HTTP/1.1 201 Created");
        assert_eq!(body, b"{}");
        handler.join().unwrap();
        server.join().unwrap();
    }
}
