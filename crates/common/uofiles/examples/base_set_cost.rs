//! What a base set costs to write and to read, phase by phase.
//!
//! The numbers in `docs/map/new_map_representation/what_a_change_costs.md`'s S1
//! are about *size*; this is the other half, and it exists because a format that
//! trades bytes for a stall on the path a person waits on has not been measured
//! until somebody has run this.
//!
//! Needs a UO install: `OPENSHARD_CLIENT` points at it, like every other thing
//! in this crate that reads shipped files.
//!
//! ```sh
//! cargo run --example base_set_cost -p openshard-uofiles              # as played
//! cargo run --release --example base_set_cost -p openshard-uofiles    # the ceiling
//! ```
//!
//! The phases are the ones a version 2 file is made of, so that "the deflate" and
//! "the hash" can be answered separately rather than as one number nobody can
//! act on.

use std::path::PathBuf;
use std::time::Instant;

use openshard_map::chunk::{
    self,
    Chunk,
};
use openshard_map::codec;
use openshard_protocol::chunks::{
    DeflateLevel,
    InflatedLength,
    deflate,
    inflate,
};
use openshard_protocol::world::Facet;

/// The fastest of `runs` runs, in milliseconds.
///
/// The minimum and not the mean, because everything that moves a number here
/// moves it *up*: this workstation runs an indexer and a compiler while it is
/// measured, and a run that got the machine to itself is the one that says what
/// the code costs. A mean over a loaded box measures the box.
fn best_of<T>(runs: u32, mut work: impl FnMut() -> T) -> f64 {
    (0..runs)
        .map(|_| {
            let at = Instant::now();
            let value = work();
            let took = at.elapsed().as_secs_f64() * 1000.0;
            drop(value);
            took
        })
        .fold(f64::INFINITY, f64::min)
}

/// FNV-1a, 64 bits — the base set's own spelling, so that what is timed here is
/// what the file does rather than something like it.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn main() {
    let Some(dir) = std::env::var_os("OPENSHARD_CLIENT").map(PathBuf::from) else {
        eprintln!("set OPENSHARD_CLIENT to a UO client directory");
        return;
    };
    let facet = Facet(0);

    // Once: reading a facet twice measures the page cache the second time.
    let at = Instant::now();
    let snapshot = openshard_uofiles::map::load_facet(&dir, facet).expect("a readable facet 0");
    let load = at.elapsed().as_secs_f64() * 1000.0;

    // Every chunk's canonical record, once kept, so the phases below are timed
    // over the same bytes the file is made of.
    let encode = best_of(3, || {
        chunk::chunks_of(snapshot.map().extent())
            .map(|at| codec::encode(&Chunk::of(&snapshot, at).expect("a chunk of this facet")))
            .collect::<Vec<_>>()
    });
    let records: Vec<Vec<u8>> = chunk::chunks_of(snapshot.map().extent())
        .map(|at| codec::encode(&Chunk::of(&snapshot, at).expect("a chunk of this facet")))
        .collect();
    let record_bytes: usize = records.iter().map(Vec::len).sum();

    let mut mixed = 0u64;
    let hash = best_of(3, || {
        mixed = 0;
        for record in &records {
            mixed ^= fnv1a64(record);
        }
    });

    let deflating = best_of(3, || {
        records
            .iter()
            .map(|record| deflate(record, DeflateLevel::BASE_SET))
            .collect::<Vec<_>>()
    });
    let blobs: Vec<Vec<u8>> = records
        .iter()
        .map(|record| deflate(record, DeflateLevel::BASE_SET))
        .collect();
    let blob_bytes: usize = blobs.iter().map(Vec::len).sum();

    // What each level would cost and save, on the same records. This is the
    // measurement `DeflateLevel`'s two constants are chosen by, and re-running it
    // is what any argument for moving either of them has to do first.
    println!("deflate levels, over {record_bytes} bytes of records");
    for level in [1_u8, 2, 3, 4, 6] {
        let at = Instant::now();
        let bytes: usize = records
            .iter()
            .map(|record| miniz_oxide::deflate::compress_to_vec_zlib(record, level).len())
            .sum();
        let took = at.elapsed().as_secs_f64() * 1000.0;
        println!(
            "  level {level}          {bytes:>10} bytes ({:.1}%)   {took:8.1} ms",
            bytes as f64 / record_bytes as f64 * 100.0
        );
    }

    let inflating = best_of(3, || {
        for (blob, record) in blobs.iter().zip(&records) {
            let length = InflatedLength(u32::try_from(record.len()).expect("a record under four GiB"));
            inflate(blob, length).expect("a stream this run just wrote");
        }
    });

    let path = std::env::temp_dir().join(format!("openshard-cost-{}.osbase", std::process::id()));
    let mut written = None;
    let write = best_of(3, || {
        written = Some(
            openshard_basemap::write(&path, &snapshot, openshard_basemap::Identity::Mint)
                .expect("a writable temp dir"),
        );
    });
    let written = written.expect("three writes");

    let read = best_of(3, || {
        openshard_basemap::read(&path).expect("the base set just written")
    });

    println!("facet {} of {}", facet.0, dir.display());
    println!("  chunks              {}", written.chunks);
    println!("  statics             {}", written.statics);
    println!("  records             {record_bytes} bytes");
    println!(
        "  deflated            {blob_bytes} bytes ({:.1}%)",
        blob_bytes as f64 / record_bytes as f64 * 100.0
    );
    println!("  file                {} bytes", written.bytes);
    println!(
        "  world               {:016x} (mixed {mixed:016x})",
        written.world.0
    );
    println!("phases, milliseconds — the fastest of three, see `best_of`");
    println!("  load_facet          {load:8.1}");
    println!("  encode              {encode:8.1}");
    println!("  hash                {hash:8.1}");
    println!("  deflate             {deflating:8.1}");
    println!("  inflate             {inflating:8.1}");
    println!("whole calls, milliseconds");
    println!("  write               {write:8.1}");
    println!("  read                {read:8.1}");

    std::fs::remove_file(&path).ok();
}
