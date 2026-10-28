use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Write;

pub fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

pub fn render(s: &Value) -> String {
    let mut out = String::new();
    let mut families = BTreeSet::new();
    let market = s["market"].as_str().unwrap_or("");
    let mut emit = |name: &str, labels: Vec<(&str, String)>, value: &Value, counter: bool| {
        if value.is_null() {
            return;
        }
        let name = format!("percolator_{name}");
        if families.insert(name.clone()) {
            writeln!(
                out,
                "# HELP {name} {}\n# TYPE {name} {}",
                name.trim_start_matches("percolator_").replace('_', " "),
                if counter { "counter" } else { "gauge" }
            )
            .unwrap();
        }
        write!(out, "{name}{{market=\"{}\"", escape(market)).unwrap();
        for (k, v) in labels {
            write!(out, ",{k}=\"{}\"", escape(&v)).unwrap();
        }
        writeln!(
            out,
            "}} {}",
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string())
        )
        .unwrap();
    };
    for (name, key) in [
        ("chain_slot", "chain_slot"),
        ("monitor_chain_read_ok", "chain_read_ok"),
        ("monitor_snapshot_unix_seconds", "snapshot_unix_seconds"),
    ] {
        emit(name, vec![], &s[key], false);
    }
    let c = &s["accounts"];
    emit("accounts_total", vec![], &c["total"], false);
    emit("accounts_discovered", vec![], &c["discovered"], false);
    emit("accounts_decode_errors", vec![], &c["decode_errors"], false);
    if let Some(o) = c["out_of_date"].as_object() {
        for (k, v) in o {
            emit("accounts_out_of_date", vec![("kind", k.clone())], v, false);
        }
    }
    if let Some(rows) = c["asset_kf_stale"].as_array() {
        for r in rows {
            emit(
                "asset_kf_stale_accounts",
                vec![
                    ("asset", r["asset"].to_string()),
                    ("side", r["side"].as_str().unwrap().into()),
                ],
                &r["count"],
                false,
            );
        }
    }
    let b = &s["buckets"];
    if let Some(o) = b["counts"].as_object() {
        for (k, v) in o {
            emit("backing_buckets", vec![("status", k.clone())], v, false);
        }
    }
    for key in ["expiring", "expiring_with_liens"] {
        emit(
            &format!("backing_buckets_{key}"),
            vec![("window_slots", b["window_slots"].to_string())],
            &b[key],
            false,
        );
    }
    if let Some(mode) = s["mode"].as_str() {
        for m in ["live", "resolved", "recovery"] {
            emit(
                "market_mode",
                vec![("mode", m.into())],
                &Value::from(u64::from(mode == m)),
                false,
            );
        }
    }
    // One loop per family: Prometheus text format needs each family's samples together.
    if let Some(rows) = s["assets"].as_array() {
        for r in rows {
            emit(
                "asset_lifecycle",
                vec![
                    ("asset", r["asset"].to_string()),
                    ("lifecycle", r["lifecycle"].as_str().unwrap().into()),
                ],
                &Value::from(1),
                false,
            );
        }
        for r in rows {
            emit(
                "asset_funding_last_accrual_slot",
                vec![("asset", r["asset"].to_string())],
                &r["funding_last_accrual_slot"],
                false,
            );
        }
    }
    let r = &s["resolved_claims"];
    emit(
        "resolved_claimants_unpaid",
        vec![],
        &r["claimants_unpaid"],
        false,
    );
    if let Some(halted) = r["payout_halted"].as_bool() {
        emit("resolved_payout_halted", vec![], &Value::from(u64::from(halted)), false);
    }
    if let Some(o) = r["owed_atoms"].as_object() {
        for (k, v) in o {
            emit(
                "resolved_owed_atoms",
                vec![("component", k.clone())],
                v,
                false,
            );
        }
    }
    let c = &s["collateral"];
    for k in [
        "unowned_collateral_atoms",
        "residual_atoms",
        "senior_shortfall_atoms",
    ] {
        emit(k, vec![], &c[k], false);
    }
    if let Some(o) = c["terms"].as_object() {
        for (k, v) in o {
            emit("engine_header_atoms", vec![("term", k.clone())], v, false);
        }
    }
    let k = &s["keeper"];
    if !k.is_null() {
        emit(
            "keeper_log_ok",
            vec![],
            &Value::from(u64::from(k["log_ok"].as_bool().unwrap_or(false))),
            false,
        );
        for field in [
            "log_age_seconds",
            "last_cycle",
            "last_cycle_cu",
            "last_success_slot",
            "slots_since_last_success",
        ] {
            emit(&format!("keeper_{field}"), vec![], &k[field], false);
        }
        for field in ["last_cycle_attempts", "attempts_total", "cycles_total"] {
            if let Some(o) = k[field].as_object() {
                for (outcome, n) in o {
                    emit(
                        &format!("keeper_{field}"),
                        vec![("outcome", outcome.clone())],
                        n,
                        field != "last_cycle_attempts",
                    );
                }
            }
        }
        if let Some(o) = k["crank_cu"].as_object() {
            for (stat, n) in o {
                emit("keeper_crank_cu", vec![("stat", stat.clone())], n, false);
            }
        }
        if let Some(rows) = k["quarantined_accounts"].as_array() {
            emit(
                "keeper_quarantined_accounts",
                vec![],
                &Value::from(rows.len()),
                false,
            );
            for r in rows {
                emit(
                    "keeper_account_quarantined_slots",
                    vec![
                        ("account", r["account"].as_str().unwrap_or("").into()),
                        ("error", r["error"].as_str().unwrap_or("").into()),
                        ("strike", r["strike"].to_string()),
                    ],
                    &r["slots"],
                    false,
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn escapes_labels_single_family_and_exact_integer_text() {
        let s = json!({"market":"a\\b\"c\nd","chain_read_ok":1,"collateral":{"unowned_collateral_atoms":"-12345678901234567890"},
            "keeper":{"log_ok":true,"attempts_total":{"progress":2,"idle":1},"quarantined_accounts":[]}});
        let text = render(&s);
        assert!(text.contains("market=\"a\\\\b\\\"c\\nd\""));
        assert!(text.contains("} -12345678901234567890\n"));
        assert_eq!(
            text.matches("# HELP percolator_keeper_attempts_total ")
                .count(),
            1
        );
        assert_eq!(
            text.matches("# TYPE percolator_keeper_attempts_total counter")
                .count(),
            1
        );
        assert!(!render(&json!({"market":"m"})).contains("keeper_log_ok"));
    }
}
