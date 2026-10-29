use solana_client::client_error::{ClientError, ClientErrorKind};

pub fn http_err(e: reqwest::Error) -> anyhow::Error {
    anyhow::anyhow!(e.without_url())
}

pub fn rpc_err(e: ClientError) -> anyhow::Error {
    match e.kind {
        ClientErrorKind::Reqwest(r) => http_err(r),
        _ => anyhow::anyhow!(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_client::rpc_client::{RpcClient, RpcClientConfig};
    use solana_rpc_client::http_sender::HttpSender;
    use solana_sdk::commitment_config::CommitmentConfig;
    use std::time::Duration;

    const URLS: [&str; 2] = [
        "https://127.0.0.1:1?api-key=SECRETKEY1",
        "https://127.0.0.1:1/?api-key=SECRETKEY1",
    ];

    #[test]
    fn rpc_errors_omit_urls() {
        for url in URLS {
            let client = reqwest::Client::builder().use_native_tls()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(2)).build().map_err(http_err).unwrap();
            let sender = HttpSender::new_with_client(url, client);
            let rpc = RpcClient::new_sender(sender, RpcClientConfig::with_commitment(CommitmentConfig::confirmed()));
            let e = rpc.get_genesis_hash().map_err(rpc_err).unwrap_err();
            let msg = format!("{e:#}");
            for secret in ["SECRETKEY1", "api-key", "127.0.0.1"] {
                assert!(!msg.contains(secret), "{msg}");
            }
        }
    }

    #[test]
    fn http_errors_omit_urls() {
        let client = reqwest::blocking::Client::builder().use_native_tls()
            .timeout(Duration::from_secs(2)).build().map_err(http_err).unwrap();
        for url in URLS {
            let e = client.get(url).send().map_err(http_err).unwrap_err();
            let msg = format!("{e:#}");
            for secret in ["SECRETKEY1", "api-key", "127.0.0.1"] {
                assert!(!msg.contains(secret), "{msg}");
            }
        }
    }
}
