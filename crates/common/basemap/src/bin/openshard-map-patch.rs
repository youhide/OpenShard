//! Commit one change to a world of ours, or read back the ones already
//! committed.
//!
//! The smallest thing that can play the part direction F's editor will: it
//! resolves the world exactly as the shard does, builds one operation against
//! the revision in hand, publishes it to prove it applies, and only then
//! appends it to the log.
//!
//! **Reading the world first is not a convenience.** An op carries what it
//! replaces — the cell that was there, the static being taken away — and the
//! only honest place to get those is the world the patch is being made against.
//! A caller that typed them in could commit a patch that describes a place that
//! does not exist, and the whole point of the field is that such a patch is
//! refused.
//!
//! ```sh
//! openshard-map-patch --base-set felucca.osbase --author stas \
//!     set-land --x 1000 --y 1000 --tile 3 --z 5
//! openshard-map-patch --base-set felucca.osbase --author stas \
//!     add-static --graphic 0x0edd --x 1000 --y 1000 --z 5
//! openshard-map-patch --base-set felucca.osbase show
//! ```
//!
//! A committed patch makes every bake over the facet stale, and the navigation
//! graph is the one that stops a shard booting — so this says which command
//! rebuilds it rather than leaving the shard to.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use openshard_basemap::{Loaded, load, patches};
use openshard_map::map::{LandCell, LandTile, StaticItem};
use openshard_map::patch::{Patch, PatchAuthor, PatchOp, PatchTime, StaticId};
use openshard_map::snapshot::{MapRevision, MapSnapshot};
use openshard_protocol::wire::{Graphic, Hue};

#[derive(Debug, Parser)]
#[command(version, about = "Commit one change to a base set's world")]
struct Cli {
    /// The base set whose world is being changed.
    ///
    /// The log is the file beside it — same name, `.ospatch` for an extension.
    #[arg(long, value_name = "FILE")]
    base_set: PathBuf,
    /// Who is committing. Attribution, not authority.
    #[arg(long, default_value = "unnamed", value_name = "NAME")]
    author: String,
    /// Say what would be committed, and commit nothing.
    #[arg(long)]
    dry_run: bool,
    #[command(subcommand)]
    what: What,
}

#[derive(Debug, Subcommand)]
enum What {
    /// Replace the ground at one tile.
    SetLand {
        /// Where.
        #[arg(long)]
        x: u16,
        /// Where.
        #[arg(long)]
        y: u16,
        /// The land graphic to put there.
        #[arg(long)]
        tile: u16,
        /// The height to put it at.
        #[arg(long, allow_negative_numbers = true)]
        z: i8,
    },
    /// Put a static on a tile, after everything already standing on it.
    AddStatic {
        /// Where.
        #[arg(long)]
        x: u16,
        /// Where.
        #[arg(long)]
        y: u16,
        /// The static graphic.
        #[arg(long)]
        graphic: u16,
        /// Its base height.
        #[arg(long, allow_negative_numbers = true)]
        z: i8,
        /// Its colour, zero for none.
        #[arg(long, default_value_t = 0)]
        hue: u16,
    },
    /// Take one static off a tile.
    RemoveStatic {
        /// Where.
        #[arg(long)]
        x: u16,
        /// Where.
        #[arg(long)]
        y: u16,
        /// Which one on the tile, counted from zero in the order `list` prints.
        #[arg(long, default_value_t = 0)]
        nth: u16,
    },
    /// Print what stands on a tile, with the ordinal of each.
    ///
    /// The thing to run before `remove-static`, since an ordinal is only an
    /// identity against a stated revision.
    List {
        /// Where.
        #[arg(long)]
        x: u16,
        /// Where.
        #[arg(long)]
        y: u16,
    },
    /// Print the log: every patch committed over this base set, in order.
    Show,
}

fn main() -> ExitCode {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("map patch: {error}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let Loaded {
        snapshot,
        base,
        log: _,
        patches: applied,
    } = load(&cli.base_set)?;
    let log = patches::log_path(&cli.base_set);
    eprintln!(
        "map patch: facet {} at revision {} ({applied} patch(es) committed)",
        snapshot.facet().0,
        snapshot.revision().get()
    );

    match cli.what {
        What::Show => return show(&log, &snapshot, base),
        What::List { x, y } => return list(&snapshot, x, y),
        _ => {}
    }

    let op = build(&snapshot, &cli.what)?;
    let patch = Patch::new(
        snapshot.facet(),
        snapshot.revision(),
        PatchAuthor(cli.author.clone()),
        PatchTime(now()),
        vec![op],
    );

    // Published to a copy of the world before it is written down: a patch that
    // does not apply is not a patch, and a log is append-only, so the check has
    // to happen before the append rather than at the next boot.
    let mut world = snapshot;
    let revision = world.publish(&patch)?;
    let touched = patch.touched_chunks();

    if cli.dry_run {
        eprintln!(
            "map patch: would commit {op:?}, making revision {}; dry run, nothing written",
            revision.get()
        );
        return Ok(());
    }

    patches::append(&log, world.facet(), base, &patch)?;
    eprintln!(
        "map patch: committed to {}; facet {} is now revision {}, and chunk(s) {} changed",
        log.display(),
        world.facet().0,
        revision.get(),
        touched
            .iter()
            .map(|at| format!("({}, {})", at.x, at.y))
            .collect::<Vec<_>>()
            .join(", ")
    );
    eprintln!(
        "map patch: every bake over this facet is now stale. Rebuild the navigation graph with:\n  \
         cargo run --release -p openshard-movement --bin openshard-navigation-bake -- \
         --facet {} --base-set {:?}",
        world.facet().0,
        cli.base_set.display()
    );
    Ok(())
}

/// The op, read against the world it is being made against.
fn build(world: &MapSnapshot, what: &What) -> Result<PatchOp, Box<dyn std::error::Error>> {
    let map = world.map();
    Ok(match *what {
        What::SetLand { x, y, tile, z } => {
            let was = map
                .land(x, y)
                .ok_or_else(|| format!("({x}, {y}) is not on this facet"))?;
            PatchOp::SetLand {
                x,
                y,
                was,
                now: LandCell {
                    tile: LandTile(tile),
                    z,
                },
            }
        }
        What::AddStatic {
            x,
            y,
            graphic,
            z,
            hue,
        } => {
            if !map.contains(x, y) {
                return Err(format!("({x}, {y}) is not on this facet").into());
            }
            PatchOp::AddStatic {
                item: StaticItem {
                    tile: Graphic(graphic),
                    x,
                    y,
                    z,
                    hue: Hue(hue),
                },
            }
        }
        What::RemoveStatic { x, y, nth } => {
            let was = *map.statics_at(x, y).nth(nth as usize).ok_or_else(|| {
                format!(
                    "({x}, {y}) has {} statics on it, and there is no number {nth}",
                    map.statics_at(x, y).count()
                )
            })?;
            PatchOp::RemoveStatic {
                which: StaticId(nth),
                was,
            }
        }
        What::List { .. } | What::Show => unreachable!("handled before the world is read"),
    })
}

/// What stands on one tile, with the ordinal each would be removed by.
fn list(world: &MapSnapshot, x: u16, y: u16) -> Result<(), Box<dyn std::error::Error>> {
    let map = world.map();
    match map.land(x, y) {
        Some(cell) => println!("land: graphic {} at z {}", cell.tile.0, cell.z),
        None => return Err(format!("({x}, {y}) is not on this facet").into()),
    }
    for (nth, item) in map.statics_at(x, y).enumerate() {
        println!(
            "{nth}: graphic {} at z {}, hue {}",
            item.tile.0, item.z, item.hue.0
        );
    }
    Ok(())
}

/// The log, as a history a person can read.
fn show(
    log: &std::path::Path,
    world: &MapSnapshot,
    base: MapRevision,
) -> Result<(), Box<dyn std::error::Error>> {
    // Read back off disk rather than reported out of the resolve above: what
    // this prints should be what is written down, not what a loader remembered.
    let committed = patches::read(log, world.facet(), base)?;
    if committed.is_empty() {
        println!("no patches committed over {}", log.display());
        return Ok(());
    }
    for patch in &committed {
        println!(
            "revision {} <- {} by {} at {}, {} op(s)",
            patch.revision().get(),
            patch.parent().get(),
            patch.author().0,
            patch.at().0,
            patch.ops().len()
        );
        for op in patch.ops() {
            println!("    {op:?}");
        }
    }
    Ok(())
}

/// Seconds since the Unix epoch, or zero on a clock before it.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}
