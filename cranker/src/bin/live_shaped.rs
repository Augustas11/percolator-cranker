//! `live_shaped` load scenario against a local validator. Exits 1 when any
//! check fails that is not on `CRANKER_SCENARIO_ALLOW_FAIL`, 2 when the
//! scenario itself errors (setup, RPC, an evidence read). See
//! `percolator_cranker::scenario::live_shaped`.

fn main() {
    match percolator_cranker::scenario::live_shaped::run() {
        Ok(report) => {
            print!("{}", report.render());
            if !report.passed() {
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("live_shaped scenario error: {e:#}");
            std::process::exit(2);
        }
    }
}
