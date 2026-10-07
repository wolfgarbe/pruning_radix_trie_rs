use std::path::PathBuf;
use std::hint::black_box;
use std::time::{Duration, Instant};

use pruning_radix_trie_rs::PruningRadixTrie;

fn format_duration(duration: Duration) -> (f64, &'static str) {
    if duration >= Duration::from_millis(1) {
        (duration.as_secs_f64() * 1_000.0, "ms")
    } else {
        (duration.as_secs_f64() * 1_000_000.0, "µs")
    }
}

fn data_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("benches")
        .join("test_data")
        .join(name)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut pruning_radix_trie = PruningRadixTrie::new();
    pruning_radix_trie.read_terms_from_file(data_path("terms.txt"))?;

    println!("Benchmark started ...");
    let rounds = 10_000;
    let query_string = "microsoft";

    for (index, character) in query_string.char_indices() {
        let prefix = &query_string[..index + character.len_utf8()];

        let ordinary_start = Instant::now();
        for _ in 0..rounds {
            black_box(pruning_radix_trie.get_top_k_terms_for_prefix(
                black_box(prefix),
                10,
                false,
            ));
        }
        let ordinary_elapsed = ordinary_start.elapsed();
        let (ordinary_time, ordinary_unit) =
            format_duration(ordinary_elapsed / rounds as u32);

        println!(
            "ordinary search {} in {:.2} {}",
            prefix, ordinary_time, ordinary_unit
        );

        let pruning_start = Instant::now();
        for _ in 0..rounds {
            black_box(pruning_radix_trie.get_top_k_terms_for_prefix(
                black_box(prefix),
                10,
                true,
            ));
        }
        let pruning_elapsed = pruning_start.elapsed();
        let (pruning_time, pruning_unit) =
            format_duration(pruning_elapsed / rounds as u32);
        println!(
            "pruning search {} in {:.2} {}",
            prefix, pruning_time, pruning_unit
        );

        println!(
            "{:.2} x faster",
            ordinary_elapsed.as_nanos() as f64 / pruning_elapsed.as_nanos() as f64
        );

        // compare to other crates



    }

    Ok(())
}