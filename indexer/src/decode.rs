//! Pure decoding of raw `getTransaction` JSON. Only top-level wrapper instructions.
use anyhow::{Context, Result};
use percolator_prog::ix::Instruction;
use serde::Serialize;
use serde_json::Value;

// Wrapper error enum order, repo-prog/src/v16_program.rs:119.
const ERRORS: &[&str] = &[
    "InvalidMagic",
    "InvalidVersion",
    "AlreadyInitialized",
    "NotInitialized",
    "InvalidAccountKind",
    "InvalidAccountLen",
    "ExpectedSigner",
    "ExpectedWritable",
    "Unauthorized",
    "InvalidInstruction",
    "InvalidMint",
    "InvalidTokenAccount",
    "InvalidVaultAccount",
    "InvalidTokenProgram",
    "EngineInvalidConfig",
    "EngineArithmeticOverflow",
    "EngineProvenanceMismatch",
    "EngineHiddenLeg",
    "EngineInvalidLeg",
    "EngineStale",
    "EngineBStale",
    "EngineLockActive",
    "EngineNonProgress",
    "EngineRecoveryRequired",
    "EngineCounterOverflow",
    "EngineCounterUnderflow",
    "OracleInvalid",
    "OracleStale",
    "OracleConfTooWide",
    "InvalidOracleKey",
    "AssetGenerationMismatch",
    "RentExemptRequired",
];
pub fn error_name(code: u64) -> Option<&'static str> {
    usize::try_from(code)
        .ok()
        .and_then(|n| ERRORS.get(n).copied())
}

/// Account-index table: (market, portfolio A/target, portfolio B).
/// Handler citations are in repo-prog/src/v16_program.rs at the pinned wrapper.
pub fn account_indices(tag: Option<u8>) -> (Option<usize>, Option<usize>, Option<usize>) {
    match tag {
        // handle_trade_nocpi:8695; handle_batch_trade_nocpi:8272
        Some(6 | 66) => (Some(2), Some(3), Some(4)),
        // handle_trade_cpi:9235; handle_batch_trade_cpi:9638
        Some(10 | 67) => (Some(1), Some(2), Some(3)),
        // init_portfolio:7757, deposit:7825, withdraw:7896, crank:14333,
        // close_portfolio:9950, close_resolved:13609, claim_resolved_payout_topup:13730.
        Some(1 | 3 | 4 | 5 | 8 | 30 | 46) => (Some(1), Some(2), None),
        // init_market:7639; push_managed_mark (PushAuthMark tag 63):13516.
        Some(0 | 63) => (Some(1), None, None),
        _ => (None, None, None),
    }
}

#[derive(Debug)]
pub struct Decoded {
    pub signature: String,
    pub slot: u64,
    pub block_time: Option<i64>,
    pub ok: bool,
    pub err_json: Option<String>,
    pub failed_ix_index: Option<u64>,
    pub error_code: Option<u64>,
    pub error_name: Option<String>,
    pub instructions: Vec<IxRow>,
    pub cranks: Vec<Crank>,
    pub fills: Vec<Fill>,
    pub transfers: Vec<Transfer>,
    pub marks: Vec<Mark>,
}
#[derive(Debug)]
pub struct IxRow {
    pub ix_index: usize,
    pub tag: Option<u8>,
    pub name: String,
    pub decode_error: Option<String>,
    pub portfolio: Option<String>,
}
#[derive(Debug)]
pub struct Crank {
    pub ix_index: usize,
    pub signer: String,
    pub target_portfolio: String,
    pub outcome: &'static str,
    pub error_code: Option<u64>,
}
#[derive(Debug, Serialize)]
pub struct Fill {
    pub signature: String,
    pub ix_index: usize,
    pub leg: usize,
    pub slot: u64,
    pub block_time: Option<i64>,
    pub taker_portfolio: String,
    pub taker_portfolio_id: u64,
    pub maker_portfolio: String,
    pub maker_portfolio_id: u64,
    pub asset_index: u16,
    pub market_id: u64,
    pub size_q: String,
    pub reported_price: Option<u64>,
    pub limit_price: Option<u64>,
    pub requested_fee_bps: u64,
    pub status: &'static str,
}
#[derive(Debug)]
pub struct Mark {
    pub ix_index: usize,
    pub asset_index: u16,
    pub price: u64,
}
#[derive(Debug)]
pub struct Transfer {
    pub ix_index: usize,
    pub kind: &'static str,
    pub portfolio: String,
    pub portfolio_id: u64,
    pub signer: String,
    pub amount: String,
}

pub fn decode(signature: &str, tx: &Value, program: &str, market: &str) -> Result<Decoded> {
    let message = &tx["transaction"]["message"];
    let mut keys = message["accountKeys"]
        .as_array()
        .context("missing account keys")?
        .clone();
    for kind in ["writable", "readonly"] {
        if let Some(loaded) = tx["meta"]["loadedAddresses"][kind].as_array() {
            keys.extend_from_slice(loaded);
        }
    }
    anyhow::ensure!(tx["meta"].is_object(), "missing transaction metadata");
    let err = &tx["meta"]["err"];
    let failed_ix_index = err["InstructionError"][0].as_u64();
    let error_code = err["InstructionError"][1]["Custom"].as_u64();
    let mut out = Decoded {
        signature: signature.into(),
        slot: tx["slot"].as_u64().context("missing slot")?,
        block_time: tx["blockTime"].as_i64(),
        ok: err.is_null(),
        err_json: (!err.is_null()).then(|| err.to_string()),
        failed_ix_index,
        error_code,
        error_name: error_code.and_then(error_name).map(str::to_owned),
        instructions: vec![],
        cranks: vec![],
        fills: vec![],
        transfers: vec![],
        marks: vec![],
    };
    for (i, raw) in message["instructions"]
        .as_array()
        .context("missing instructions")?
        .iter()
        .enumerate()
    {
        let key = |index: &Value| {
            index
                .as_u64()
                .and_then(|n| keys.get(n as usize))
                .and_then(Value::as_str)
        };
        if key(&raw["programIdIndex"]) != Some(program) {
            continue;
        }
        let accounts = raw["accounts"]
            .as_array()
            .context("missing instruction accounts")?;
        let account = |n: usize| -> Result<String> {
            accounts
                .get(n)
                .and_then(key)
                .map(str::to_owned)
                .context("missing instruction account")
        };
        let bytes = bs58::decode(raw["data"].as_str().unwrap_or("")).into_vec();
        let tag = bytes.as_ref().ok().and_then(|b| b.first().copied());
        let decoded = bytes
            .map_err(|e| e.to_string())
            .and_then(|b| Instruction::decode(&b).map_err(|e| format!("{e:?}")));
        let name = decoded
            .as_ref()
            .map(|ix| {
                format!("{ix:?}")
                    .split([' ', '{'])
                    .next()
                    .unwrap()
                    .to_owned()
            })
            .unwrap_or_else(|_| "Undecodable".into());
        let (market_index, a, b) = account_indices(tag);
        let matches_market = match market_index {
            Some(n) => accounts.get(n).and_then(key) == Some(market),
            None => accounts.iter().any(|n| key(n) == Some(market)),
        };
        if !matches_market {
            continue;
        }
        out.instructions.push(IxRow {
            ix_index: i,
            tag,
            name,
            decode_error: decoded.as_ref().err().cloned(),
            portfolio: a.and_then(|n| account(n).ok()),
        });
        if market_index.is_none() {
            continue;
        }
        // Classification is by public tag even when malformed data was rejected.
        // Invalid account lists remain generic rows rather than blocking a whole page.
        if tag == Some(5) {
            if let (Ok(signer), Ok(target_portfolio)) = (account(0), account(2)) {
                let code = (failed_ix_index == Some(i as u64))
                    .then_some(error_code)
                    .flatten();
                out.cranks.push(Crank {
                    ix_index: i,
                    signer,
                    target_portfolio,
                    outcome: if out.ok {
                        "ok"
                    } else if failed_ix_index != Some(i as u64) {
                        "reverted"
                    } else if code == Some(22) {
                        "idle"
                    } else {
                        "failed"
                    },
                    error_code: code,
                });
            }
        }
        let Ok(ix) = decoded else {
            continue;
        };
        if [Some(0), a, b]
            .into_iter()
            .flatten()
            .any(|n| account(n).is_err())
        {
            out.instructions.last_mut().unwrap().decode_error =
                Some("missing instruction account".into());
            continue;
        }
        if let Instruction::Deposit {
            portfolio_id,
            amount,
            ..
        }
        | Instruction::Withdraw {
            portfolio_id,
            amount,
            ..
        } = ix
        {
            out.transfers.push(Transfer {
                ix_index: i,
                kind: if tag == Some(3) {
                    "deposit"
                } else {
                    "withdraw"
                },
                portfolio: account(2)?,
                portfolio_id,
                signer: account(0)?,
                amount: amount.to_string(),
            });
        }
        if !out.ok {
            continue;
        }
        if let Instruction::PushAuthMark {
            asset_index,
            mark_e6,
            ..
        } = &ix
        {
            out.marks.push(Mark {
                ix_index: i,
                asset_index: *asset_index,
                price: *mark_e6,
            });
        }
        let mut fill = |leg,
                        aid,
                        bid,
                        asset_index,
                        market_id,
                        size_q: i128,
                        exec_price,
                        limit_price,
                        fee_bps|
         -> Result<()> {
            out.fills.push(Fill {
                signature: signature.into(),
                ix_index: i,
                leg,
                slot: out.slot,
                block_time: out.block_time,
                taker_portfolio: account(a.context("missing taker index")?)?,
                taker_portfolio_id: aid,
                maker_portfolio: account(b.context("missing maker index")?)?,
                maker_portfolio_id: bid,
                asset_index,
                market_id,
                size_q: size_q.to_string(),
                reported_price: exec_price,
                limit_price,
                requested_fee_bps: fee_bps,
                status: if matches!(tag, Some(10 | 67)) {
                    "requested"
                } else {
                    "executed"
                },
            });
            Ok(())
        };
        match ix {
            Instruction::TradeNoCpi {
                account_a_portfolio_id: a,
                account_b_portfolio_id: b,
                asset_index,
                market_id,
                size_q,
                exec_price,
                fee_bps,
                ..
            } => fill(
                0,
                a,
                b,
                asset_index,
                market_id,
                size_q,
                Some(exec_price),
                None,
                fee_bps,
            )?,
            Instruction::TradeCpi {
                account_a_portfolio_id: a,
                account_b_portfolio_id: b,
                asset_index,
                market_id,
                size_q,
                limit_price,
                fee_bps,
                ..
            } => fill(
                0,
                a,
                b,
                asset_index,
                market_id,
                size_q,
                None,
                Some(limit_price),
                fee_bps,
            )?,
            Instruction::BatchTradeNoCpi {
                account_a_portfolio_id: a,
                account_b_portfolio_id: b,
                legs,
                ..
            } => {
                for (j, l) in legs.iter().enumerate() {
                    fill(
                        j,
                        a,
                        b,
                        l.asset_index,
                        l.market_id,
                        l.size_q,
                        Some(l.exec_price),
                        None,
                        l.fee_bps,
                    )?;
                }
            }
            Instruction::BatchTradeCpi {
                account_a_portfolio_id: a,
                account_b_portfolio_id: b,
                legs,
                ..
            } => {
                for (j, l) in legs.iter().enumerate() {
                    fill(
                        j,
                        a,
                        b,
                        l.asset_index,
                        l.market_id,
                        l.size_q,
                        None,
                        Some(l.limit_price),
                        l.fee_bps,
                    )?;
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use percolator_prog::{
        error::PercolatorError,
        ix::{BatchTradeCpiLeg, BatchTradeLeg},
    };
    use serde_json::json;
    pub const PROGRAM: &str = "GaXu3DXKYcMs8WmZsgfWcHXQSDva2qe7Aguvn3YHKSHS";
    pub const MARKET: &str = "7p8DtcMHSjdCsM9xWvBwF6TebGBSL18wKvfM7BS1zyHq";
    const A: &str = "5XWHkzJhpp1hFjMrTwbARWqCJGGuRmrrDDqHW9EmYbzN";
    const B: &str = "GKJHRYBKwtswFHK5oyGknpcXxYEMjh7rDDR9f1KU9T5P";
    pub fn fixture(name: &str) -> Decoded {
        let text = std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        decode(v["signature"].as_str().unwrap(), &v["tx"], PROGRAM, MARKET).unwrap()
    }
    fn synthetic(ix: &Instruction) -> Value {
        let tag = ix.encode()[0];
        let accounts = if matches!(tag, 6 | 66) {
            json!([0, 1, 2, 3, 4])
        } else {
            json!([0, 2, 3, 4])
        };
        json!({"slot":100,"blockTime":500,"meta":{"err":null},"transaction":{"message":{
            "accountKeys":["11111111111111111111111111111111",B,MARKET,A,B,PROGRAM],
            "instructions":[{"programIdIndex":5,"accounts":accounts,"data":bs58::encode(ix.encode()).into_string()}]}}})
    }
    #[test]
    fn pins_error_enum_order() {
        assert_eq!(PercolatorError::EngineNonProgress as u64, 22);
        assert_eq!(error_name(22), Some("EngineNonProgress"));
        assert_eq!(PercolatorError::EngineStale as u64, 19);
        assert_eq!(error_name(19), Some("EngineStale"));
        assert_eq!(
            PercolatorError::RentExemptRequired as usize,
            ERRORS.len() - 1
        );
        assert_eq!(error_name(31), Some("RentExemptRequired"));
        assert_eq!(error_name(32), None);
    }
    #[test]
    fn encoder_round_trips() {
        let cases = vec![
            Instruction::PermissionlessCrank {
                now_slot: 100,
                observations: vec![],
            },
            Instruction::Deposit {
                portfolio_id: u64::MAX,
                expected_sequence: 0,
                amount: u128::MAX,
            },
            Instruction::Withdraw {
                portfolio_id: 7,
                expected_sequence: 1,
                amount: 123,
            },
            Instruction::TradeNoCpi {
                account_a_portfolio_id: 7,
                account_a_position_epoch: 0,
                account_b_portfolio_id: 8,
                account_b_position_epoch: 0,
                asset_index: 2,
                market_id: 9,
                size_q: -42,
                exec_price: 12345,
                fee_bps: 5,
                backing_fee_cap_bps: 0,
            },
            Instruction::TradeCpi {
                account_a_portfolio_id: 7,
                account_a_position_epoch: 0,
                account_b_portfolio_id: 8,
                account_b_position_epoch: 0,
                account_b_matcher_sequence: 0,
                asset_index: 2,
                market_id: 9,
                size_q: -42,
                fee_bps: 5,
                limit_price: 12345,
                backing_fee_cap_bps: 0,
            },
            Instruction::BatchTradeNoCpi {
                account_a_portfolio_id: 7,
                account_a_position_epoch: 0,
                account_b_portfolio_id: 8,
                account_b_position_epoch: 0,
                legs: vec![
                    BatchTradeLeg {
                        asset_index: 2,
                        market_id: 9,
                        size_q: -42,
                        exec_price: 12345,
                        fee_bps: 5,
                    },
                    BatchTradeLeg {
                        asset_index: 3,
                        market_id: 10,
                        size_q: 1,
                        exec_price: 99,
                        fee_bps: 2,
                    },
                ],
            },
            Instruction::BatchTradeCpi {
                account_a_portfolio_id: 7,
                account_a_position_epoch: 0,
                account_b_portfolio_id: 8,
                account_b_position_epoch: 0,
                account_b_matcher_sequence: 0,
                max_slippage_atoms: 0,
                max_fee_atoms: 0,
                legs: vec![
                    BatchTradeCpiLeg {
                        asset_index: 2,
                        market_id: 9,
                        size_q: -42,
                        limit_price: 12345,
                        fee_bps: 5,
                    },
                    BatchTradeCpiLeg {
                        asset_index: 3,
                        market_id: 10,
                        size_q: 1,
                        limit_price: 99,
                        fee_bps: 2,
                    },
                ],
            },
        ];
        for ix in cases {
            let out = decode("synthetic", &synthetic(&ix), PROGRAM, MARKET).unwrap();
            assert!(out.instructions[0].decode_error.is_none());
            assert_eq!(out.instructions[0].tag, Some(ix.encode()[0]));
            match ix {
                Instruction::PermissionlessCrank { .. } => {
                    assert_eq!(out.cranks.len(), 1);
                    assert_eq!(out.cranks[0].outcome, "ok");
                    assert_eq!(out.cranks[0].target_portfolio, A);
                }
                Instruction::Deposit {
                    amount,
                    portfolio_id,
                    ..
                }
                | Instruction::Withdraw {
                    amount,
                    portfolio_id,
                    ..
                } => {
                    assert_eq!(out.transfers[0].amount, amount.to_string());
                    assert_eq!(out.transfers[0].portfolio_id, portfolio_id);
                    assert_eq!(out.transfers[0].portfolio, A);
                }
                _ => {
                    let f = &out.fills[0];
                    assert_eq!(f.taker_portfolio, A);
                    assert_eq!(f.maker_portfolio, B);
                    assert_eq!(f.taker_portfolio_id, 7);
                    assert_eq!(f.maker_portfolio_id, 8);
                    assert_eq!(f.asset_index, 2);
                    assert_eq!(f.market_id, 9);
                    assert_eq!(f.size_q, "-42");
                    assert_eq!(f.requested_fee_bps, 5);
                    let cpi = matches!(
                        ix,
                        Instruction::TradeCpi { .. } | Instruction::BatchTradeCpi { .. }
                    );
                    assert_eq!(f.status, if cpi { "requested" } else { "executed" });
                    assert_eq!(f.reported_price, (!cpi).then_some(12345));
                    assert_eq!(f.limit_price, cpi.then_some(12345));
                    let batch = matches!(
                        ix,
                        Instruction::BatchTradeNoCpi { .. } | Instruction::BatchTradeCpi { .. }
                    );
                    assert_eq!(out.fills.len(), if batch { 2 } else { 1 });
                    if batch {
                        assert_eq!(out.fills[1].leg, 1);
                        assert_eq!(out.fills[1].size_q, "1");
                    }
                }
            }
        }
    }
    #[test]
    fn atomic_failures_and_error_attribution() {
        let ix = Instruction::PermissionlessCrank {
            now_slot: 100,
            observations: vec![],
        };
        let mut tx = synthetic(&ix);
        let original = tx["transaction"]["message"]["instructions"][0].clone();
        tx["transaction"]["message"]["instructions"]
            .as_array_mut()
            .unwrap()
            .push(original);
        for (code, outcome, name) in [
            (22, "idle", "EngineNonProgress"),
            (19, "failed", "EngineStale"),
        ] {
            tx["meta"]["err"] = json!({"InstructionError":[1,{"Custom":code}]});
            let d = decode("failed", &tx, PROGRAM, MARKET).unwrap();
            assert!(!d.ok);
            assert_eq!(d.error_name.as_deref(), Some(name));
            assert_eq!(d.failed_ix_index, Some(1));
            assert_eq!(d.cranks[0].outcome, "reverted");
            assert_eq!(d.cranks[0].error_code, None);
            assert_eq!(d.cranks[1].outcome, outcome);
            assert_eq!(d.cranks[1].error_code, Some(code));
        }
        let mut v: Value =
            serde_json::from_str(include_str!("../tests/fixtures/crank_trade.json")).unwrap();
        v["tx"]["meta"]["err"] = json!({"InstructionError":[2,{"Custom":22}]});
        let d = decode("atomic", &v["tx"], PROGRAM, MARKET).unwrap();
        assert!(d.fills.is_empty());
        assert_eq!(d.cranks[1].outcome, "reverted");
        tx["meta"]["err"] = json!("AccountNotFound");
        let d = decode("failed", &tx, PROGRAM, MARKET).unwrap();
        assert_eq!(d.err_json.as_deref(), Some("\"AccountNotFound\""));
    }
    #[test]
    fn loaded_addresses_filtering_and_undecodable() {
        let ix = Instruction::PermissionlessCrank {
            now_slot: 1,
            observations: vec![],
        };
        let mut tx = synthetic(&ix);
        tx["transaction"]["message"]["accountKeys"] =
            json!(["11111111111111111111111111111111", B]);
        tx["meta"]["loadedAddresses"] = json!({"writable":[MARKET,A,B],"readonly":[PROGRAM]});
        assert_eq!(decode("v0", &tx, PROGRAM, MARKET).unwrap().cranks.len(), 1);
        assert!(decode("other", &tx, PROGRAM, "unrelated-market")
            .unwrap()
            .instructions
            .is_empty());
        tx["meta"]["innerInstructions"] = json!([{"index":0,"instructions":[tx["transaction"]["message"]["instructions"][0].clone()]}]);
        assert_eq!(decode("cpi", &tx, PROGRAM, MARKET).unwrap().cranks.len(), 1);
        tx["transaction"]["message"]["instructions"][0]["data"] =
            json!(bs58::encode([5u8]).into_string());
        let d = decode("bad", &tx, PROGRAM, MARKET).unwrap();
        assert_eq!(d.instructions[0].tag, Some(5));
        assert!(d.instructions[0].decode_error.is_some());
        assert_eq!(d.cranks.len(), 1);
        // A valid instruction body with a rejected account list must not stall ingestion.
        tx["transaction"]["message"]["instructions"][0]["data"] =
            json!(bs58::encode(ix.encode()).into_string());
        tx["transaction"]["message"]["instructions"][0]["accounts"] = json!([2]);
        tx["meta"]["err"] = json!({"InstructionError":[0,"NotEnoughAccountKeys"]});
        let d = decode("bad_accounts", &tx, PROGRAM, MARKET).unwrap();
        assert!(d.instructions.is_empty());
        assert!(d.cranks.is_empty());
    }
    #[test]
    fn unknown_layout_is_generic_and_failed_marks_are_ignored() {
        let mark = Instruction::PushAuthMark {
            asset_index: 2,
            market_id: 9,
            now_slot: 100,
            mark_e6: u64::MAX,
            observation_sequence: 1,
            authority_epoch: 0,
        };
        let mut tx = synthetic(&mark);
        let d = decode("mark", &tx, PROGRAM, MARKET).unwrap();
        assert_eq!(d.marks[0].price, u64::MAX);
        assert_eq!(d.marks[0].asset_index, 2);
        tx["meta"]["err"] = json!({"InstructionError":[0,{"Custom":19}]});
        assert!(decode("failed", &tx, PROGRAM, MARKET)
            .unwrap()
            .marks
            .is_empty());
        tx["transaction"]["message"]["instructions"][0]["data"] =
            json!(bs58::encode([255u8]).into_string());
        let d = decode("unknown", &tx, PROGRAM, MARKET).unwrap();
        assert_eq!(d.instructions.len(), 1);
        assert!(
            d.fills.is_empty()
                && d.cranks.is_empty()
                && d.transfers.is_empty()
                && d.marks.is_empty()
        );
    }
    #[test]
    fn appended_market_does_not_match_handler_market() {
        let mut v: Value =
            serde_json::from_str(include_str!("../tests/fixtures/crank_trade.json")).unwrap();
        let message = &mut v["tx"]["transaction"]["message"];
        let mut trade = message["instructions"][4].clone();
        let accounts = trade["accounts"].as_array_mut().unwrap();
        let market = accounts[2].clone();
        accounts[2] = accounts[3].clone();
        accounts.push(market);
        message["instructions"] = json!([trade]);
        let d = decode("other", &v["tx"], PROGRAM, MARKET).unwrap();
        assert!(d.instructions.is_empty());
        assert!(d.fills.is_empty());
        assert!(d.cranks.is_empty());
        assert!(d.transfers.is_empty());
    }
    #[test]
    fn real_devnet_fixtures() {
        let d = fixture("crank_trade");
        assert_eq!(
            d.instructions.iter().map(|i| i.tag).collect::<Vec<_>>(),
            vec![Some(5), Some(5), Some(6)]
        );
        assert_eq!(d.cranks.len(), 2);
        assert!(d.cranks.iter().all(|c| c.outcome == "ok"));
        let f = &d.fills[0];
        assert_eq!(f.taker_portfolio, A);
        assert_eq!(f.maker_portfolio, B);
        // Values decoded from the supplied raw fixture, not an inferred market size/price.
        assert_eq!(f.taker_portfolio_id, 2);
        assert_eq!(f.maker_portfolio_id, 1);
        assert_eq!(f.asset_index, 0);
        assert_eq!(f.market_id, 1);
        assert_eq!(f.size_q, "10000000");
        assert_eq!(f.reported_price, Some(100038102));
        assert_eq!(f.requested_fee_bps, 12);
        assert_eq!(f.status, "executed");
        let dep = fixture("deposit");
        assert_eq!(dep.instructions[0].tag, Some(3));
        assert_eq!(dep.transfers[0].amount, "30000000000");
        assert_eq!(dep.transfers[0].portfolio_id, 2);
        assert_eq!(dep.transfers[0].portfolio, A);
        let crank = fixture("crank");
        assert_eq!(crank.instructions[0].tag, Some(5));
        assert_eq!(crank.cranks[0].outcome, "ok");
        assert_eq!(
            fixture("init_portfolio").instructions[0].name,
            "InitPortfolio"
        );
        assert_eq!(fixture("push_mark").instructions[0].name, "PushAuthMark");
        assert_eq!(fixture("push_mark").instructions[0].tag, Some(63));
    }
}

#[cfg(test)]
mod variant_tests {
    use super::tests::{MARKET, PROGRAM};
    use super::*;
    use serde_json::json;
    #[test]
    fn administrative_and_lifecycle_tags_keep_upstream_names() {
        let cases = vec![
            (
                0,
                Instruction::InitMarket {
                    max_portfolio_assets: 1,
                    h_min: 0,
                    h_max: 0,
                    initial_price: 1,
                    min_nonzero_mm_req: 0,
                    min_nonzero_im_req: 0,
                    maintenance_margin_bps: 0,
                    initial_margin_bps: 0,
                    max_trading_fee_bps: 0,
                    trade_fee_base_bps: 0,
                    liquidation_fee_bps: 0,
                    liquidation_fee_cap: 0,
                    min_liquidation_abs: 0,
                    max_price_move_bps_per_slot: 0,
                    max_accrual_dt_slots: 0,
                    max_abs_funding_e9_per_slot: 0,
                    min_funding_lifetime_slots: 0,
                    max_account_b_settlement_chunks: 0,
                    max_bankrupt_close_chunks: 0,
                    max_bankrupt_close_lifetime_slots: 0,
                    public_b_chunk_atoms: 0,
                    maintenance_fee_per_slot: 0,
                },
                "InitMarket",
            ),
            (1, Instruction::InitPortfolio, "InitPortfolio"),
            (
                8,
                Instruction::ClosePortfolio {
                    portfolio_id: 1,
                    expected_sequence: 0,
                    position_epoch: 0,
                },
                "ClosePortfolio",
            ),
            (
                9,
                Instruction::TopUpInsurance {
                    market_id: 1,
                    intent_id: 0,
                    authority_epoch: 0,
                    amount: 1,
                },
                "TopUpInsurance",
            ),
            (
                24,
                Instruction::TopUpBackingBucket {
                    domain: 0,
                    market_id: 1,
                    intent_id: 0,
                    authority_epoch: 0,
                    backing_fee_bps: 0,
                    insurance_share_bps: 0,
                    amount: 1,
                    expiry_slot: 2,
                },
                "TopUpBackingBucket",
            ),
            (
                30,
                Instruction::CloseResolved {
                    fee_rate_per_slot: 0,
                },
                "CloseResolved",
            ),
            (
                32,
                Instruction::UpdateAuthority {
                    authority_epoch: 0,
                    new_pubkey: [1; 32],
                },
                "UpdateAuthority",
            ),
            (
                38,
                Instruction::ConfigurePermissionlessResolve {
                    asset_generation_frontier: 0,
                    stale_slots: 0,
                    force_close_delay_slots: 0,
                    policy_sequence: 0,
                    authority_epoch: 0,
                },
                "ConfigurePermissionlessResolve",
            ),
            (
                46,
                Instruction::ClaimResolvedPayoutTopup,
                "ClaimResolvedPayoutTopup",
            ),
            (
                62,
                Instruction::ConfigureAuthMark {
                    asset_index: 0,
                    market_id: 1,
                    now_slot: 2,
                    initial_mark_e6: 1,
                    observation_sequence: 0,
                    authority_epoch: 0,
                },
                "ConfigureAuthMark",
            ),
            (
                63,
                Instruction::PushAuthMark {
                    asset_index: 0,
                    market_id: 1,
                    now_slot: 2,
                    mark_e6: 1,
                    observation_sequence: 0,
                    authority_epoch: 0,
                },
                "PushAuthMark",
            ),
            (
                65,
                Instruction::UpdateAssetAuthority {
                    asset_index: 0,
                    market_id: 1,
                    authority_epoch: 0,
                    kind: 0,
                    new_pubkey: [1; 32],
                },
                "UpdateAssetAuthority",
            ),
        ];
        for (tag, ix, name) in cases {
            let data = ix.encode();
            assert_eq!(data[0], tag);
            let tx = json!({"slot":1,"meta":{"err":null},"transaction":{"message":{"accountKeys":[PROGRAM,MARKET],"instructions":[{"programIdIndex":0,"accounts":[0,1,0],"data":bs58::encode(data).into_string()}]}}});
            let out = decode("name", &tx, PROGRAM, MARKET).unwrap();
            assert_eq!(out.instructions[0].name, name);
            assert!(out.instructions[0].decode_error.is_none());
        }
    }
}
