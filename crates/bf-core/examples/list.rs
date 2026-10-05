//! Reads and sorts one directory. Prints the entry count and the timings.
//!
//! Usage: `cargo run --release -p bf-core --example list -- <dir>`

use std::path::PathBuf;
use std::time::Instant;

use bf_core::sort::{SortOptions, sort_entries};

fn main() {
    let dir = std::env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from("."), PathBuf::from);

    let start = Instant::now();
    let mut entries = match bf_core::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("Cannot open {}: {err}", dir.display());
            std::process::exit(1);
        }
    };
    let read = start.elapsed();

    let start = Instant::now();
    sort_entries(&mut entries, SortOptions::default());
    let sort = start.elapsed();

    println!(
        "{} entries, read {:.1} ms, sort {:.1} ms",
        entries.len(),
        read.as_secs_f64() * 1000.0,
        sort.as_secs_f64() * 1000.0
    );
}
