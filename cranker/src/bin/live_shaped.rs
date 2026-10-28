//! `live_shaped` load scenario against a local validator. Exits nonzero
//! when any assertion fails. See `percolator_cranker::scenario::live_shaped`.

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
