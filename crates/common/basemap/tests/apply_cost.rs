//! TEMPORARY — what `chunk::apply` costs when a chunk's item count moved.
//!
//! Run: cargo test -p openshard-basemap --release --test apply_cost -- --ignored --nocapture

use std::time::{Duration, Instant};

use openshard_map::chunk::{Chunk, ChunkCoord};
use openshard_map::map::StaticItem;
use openshard_map::patch::{Patch, PatchAuthor, PatchOp, PatchTime};
use openshard_protocol::wire::{Graphic, Hue};

fn ms(took: Duration) -> String {
    format!("{:.2} ms", took.as_secs_f64() * 1000.0)
}

const PATH: &str = "/home/sc/t/OpenShard/felucca.osbase";

#[test]
#[ignore]
fn what_a_grown_chunk_costs() {
    let world = openshard_basemap::read(PATH).expect("a base set");
    let facet = world.facet();
    println!(
        "{} blocks, {} statics",
        world.map().extent().count(),
        world.map().static_count()
    );
    drop(world);

    for (label, x, y) in [
        ("the middle of the facet", 1363u16, 1600u16),
        ("the north-west corner", 12u16, 12u16),
        ("the south-east corner", 7000u16, 4000u16),
    ] {
        let mut grown = openshard_basemap::read(PATH).expect("a base set");
        grown
            .publish(&Patch::new(
                facet,
                grown.revision(),
                PatchAuthor("measurement".to_owned()),
                PatchTime(0),
                vec![PatchOp::AddStatic {
                    item: StaticItem {
                        tile: Graphic(0x0dc0),
                        x,
                        y,
                        z: 20,
                        hue: Hue(0),
                    },
                }],
            ))
            .expect("the sample patch applies");

        let at = ChunkCoord::containing(x, y);
        let chunks = vec![Chunk::of(&grown, at).expect("the chunk the static landed in")];

        // A *fresh* world each round: applying the same chunk twice is
        // length-preserving the second time, which is the case with no tail
        // move in it and not what this is measuring.
        let mut best = Duration::MAX;
        let mut count = 0;
        for _ in 0..3 {
            let mut world = openshard_basemap::read(PATH).expect("a base set");
            let start = Instant::now();
            world.take_chunks(&chunks).expect("one chunk of this facet");
            best = best.min(start.elapsed());
            count = world.map().static_count();
        }
        println!("one more static, at {label}: {} — {count} statics", ms(best));
    }
}
