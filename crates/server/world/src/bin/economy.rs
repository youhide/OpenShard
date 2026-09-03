//! Print the economy reachability report.
//!
//! ```sh
//! cargo run -p openshard-world --bin economy
//! ```
//!
//! The analysis is [`openshard_world::economy`]; this is a page of paper for it.
//! Both expansions are printed, because they are two different worlds: before
//! Mondain's Legacy a tree gives one kind of log, and a hole that exists in only
//! one era is still a hole.
//!
//! Nothing here loads a map, a save or a config — the graph is built from the
//! shipped tables alone, which is what makes it runnable in a bare checkout and
//! answerable in milliseconds.

use openshard_world::economy::Economy;

fn main() {
    for ml in [true, false] {
        let era = if ml { "Mondain's Legacy" } else { "pre-ML" };
        let economy = Economy::of(ml);
        let report = economy.report();
        println!("=== {era} ===");
        println!(
            "{} steps, {} resources reachable\n",
            economy.steps.len(),
            economy.reachable.len()
        );
        print!("{report}");
        println!(
            "\nverdict: {}\n",
            if report.is_closed() {
                "the economy closes"
            } else {
                "the economy does not close"
            }
        );
    }
}
