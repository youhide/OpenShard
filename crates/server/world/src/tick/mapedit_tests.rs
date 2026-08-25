//! The editor request at the tick boundary: authenticated authority and author,
//! exact-parent conflict, canonical batch compilation, durable commit and reply.

use std::path::{Path, PathBuf};

use super::tests::{enter_as, enter_gm, packets_for};
use super::*;
use openshard_map::grid::BlockExtent;
use openshard_map::map::{LandCell, StaticItem, WorldMap};
use openshard_map::snapshot::{MapRevision, MapSnapshot};
use openshard_protocol::chunks::WorldRevision;
use openshard_protocol::mapedit::{
    EditLandTile, EditStaticId, EditTile, EditX, EditY, EditZ, MapEditOp, MapEditOutcome, MapEditRefusal,
    MapEditReply, MapEditRequest,
};
use openshard_state::WorldHome;
use openshard_tiles::LandTileId;

const FACET: Facet = Facet(0);

fn land(value: u16) -> EditLandTile {
    EditLandTile::from_wire(value).expect("a fixture land tile")
}

fn at(x: u16, y: u16) -> EditTile {
    EditTile {
        x: EditX(x),
        y: EditY(y),
    }
}

fn base_set(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "openshard-wire-mapedit-{tag}-{}.osbase",
        std::process::id()
    ))
}

fn clean(path: &Path) {
    std::fs::remove_file(openshard_basemap::patches::log_path(path)).ok();
    std::fs::remove_file(path).ok();
}

/// A small owned facet, with one static at (3,4), loaded through the same base
/// set identity and home a real shard supplies to `mapedit::commit`.
fn owned_world(tag: &str) -> (World, PathBuf) {
    let path = base_set(tag);
    clean(&path);
    let mut map = WorldMap::from_blocks(BlockExtent { wide: 2, down: 2 }, |_, _| LandCell {
        tile: LandTileId(3),
        z: 0,
    });
    map.place_static(StaticItem {
        tile: Graphic(0x100),
        x: 3,
        y: 4,
        z: 0,
        hue: Hue::NONE,
    });
    openshard_basemap::write(
        &path,
        &MapSnapshot::new(FACET, map),
        openshard_basemap::Identity::Mint,
    )
    .expect("a writable temp dir");
    let loaded = openshard_basemap::load(&path).expect("the base set just written");
    let home = WorldHome {
        base_set: path.clone(),
        base: loaded.base,
        identity: openshard_basemap::identity_of(&path).expect("the base set just written"),
    };
    let world = World::new((8, 8)).with_facet(
        FACET,
        loaded.snapshot,
        None,
        FacetRules::classic(FACET),
        Some(home),
    );
    (world, path)
}

fn current(world: &World) -> WorldRevision {
    WorldRevision(
        world
            .state
            .facet_state(FACET)
            .ground()
            .snapshot()
            .expect("the fixture has ground")
            .revision()
            .get(),
    )
}

fn ask(world: &mut World, connection: ConnectionId, request: MapEditRequest) -> MapEditReply {
    world.queue(Command::CommitMapEdit { connection, request });
    world.tick(Instant::now());
    let replies: Vec<MapEditReply> = packets_for(world, connection)
        .iter()
        .filter_map(|bytes| ServerPacket::decode(bytes, ClientVersion::TOL).expect("the shard's own packet"))
        .filter_map(|packet| match packet {
            ServerPacket::MapEditReply(reply) => Some(reply),
            _ => None,
        })
        .collect();
    assert_eq!(replies.len(), 1, "one request has exactly one answer");
    replies[0]
}

#[test]
fn a_player_cannot_commit_and_supplies_neither_author_nor_revision_information() {
    let (mut world, path) = owned_world("player");
    let connection = enter_as(&mut world, super::tests::connection(), Instant::now());
    let _entry = packets_for(&mut world, connection);
    let before = current(&world);

    let reply = ask(
        &mut world,
        connection,
        MapEditRequest {
            facet: FACET,
            parent: before,
            ops: vec![MapEditOp::SetLand {
                at: at(3, 4),
                tile: land(9),
                z: EditZ(5),
            }],
        },
    );

    assert_eq!(
        reply.revision,
        WorldRevision(0),
        "do not disclose state to an unauthorized request"
    );
    assert_eq!(
        reply.outcome,
        MapEditOutcome::Refused(MapEditRefusal::NotAuthorized)
    );
    assert_eq!(current(&world), before, "authority failure changes nothing");
    assert!(
        openshard_basemap::patches::read(
            &openshard_basemap::patches::log_path(&path),
            FACET,
            MapRevision::INITIAL
        )
        .expect("an absent log is empty")
        .is_empty()
    );
    clean(&path);
}

#[test]
fn a_gm_batch_is_compiled_in_order_attributed_to_the_session_and_committed() {
    let (mut world, path) = owned_world("accepted");
    let connection = enter_gm(&mut world, Instant::now());
    let _entry = packets_for(&mut world, connection);
    let parent = current(&world);

    let reply = ask(
        &mut world,
        connection,
        MapEditRequest {
            facet: FACET,
            parent,
            ops: vec![
                // The second replacement must record the first one's cell as
                // `was`, not the parent snapshot's cell.
                MapEditOp::SetLand {
                    at: at(3, 4),
                    tile: land(8),
                    z: EditZ(1),
                },
                MapEditOp::SetLand {
                    at: at(3, 4),
                    tile: land(9),
                    z: EditZ(2),
                },
                // There was one static.  Add is ordinal one and the following
                // remove sees it, leaving the original untouched.
                MapEditOp::AddStatic {
                    at: at(3, 4),
                    graphic: Graphic(0x200),
                    z: EditZ(7),
                    hue: Hue(4),
                },
                MapEditOp::RemoveStatic {
                    at: at(3, 4),
                    which: EditStaticId(1),
                },
            ],
        },
    );

    assert_eq!(reply.outcome, MapEditOutcome::Accepted);
    assert_eq!(reply.revision, WorldRevision(parent.0 + 1));
    let snapshot = world.state.facet_state(FACET).ground().snapshot().unwrap();
    assert_eq!(
        snapshot.map().land(3, 4),
        Some(LandCell {
            tile: LandTileId(9),
            z: 2
        })
    );
    assert_eq!(snapshot.map().statics_at(3, 4).count(), 1);

    let patches = openshard_basemap::patches::read(
        &openshard_basemap::patches::log_path(&path),
        FACET,
        MapRevision::INITIAL,
    )
    .expect("the committed log reads");
    assert_eq!(patches.len(), 1);
    assert_eq!(
        patches[0].author().0,
        "admin",
        "the authenticated account, not a wire field"
    );
    assert_eq!(patches[0].ops().len(), 4);
    clean(&path);
}

#[test]
fn a_stale_parent_is_a_conflict_with_the_current_revision_and_no_log_entry() {
    let (mut world, path) = owned_world("conflict");
    let connection = enter_gm(&mut world, Instant::now());
    let _entry = packets_for(&mut world, connection);
    let holding = current(&world);

    let reply = ask(
        &mut world,
        connection,
        MapEditRequest {
            facet: FACET,
            parent: WorldRevision(holding.0 + 7),
            ops: vec![MapEditOp::SetLand {
                at: at(3, 4),
                tile: land(9),
                z: EditZ(5),
            }],
        },
    );

    assert_eq!(reply.revision, holding);
    assert_eq!(reply.outcome, MapEditOutcome::Refused(MapEditRefusal::Conflict));
    assert_eq!(current(&world), holding);
    assert!(
        openshard_basemap::patches::read(
            &openshard_basemap::patches::log_path(&path),
            FACET,
            MapRevision::INITIAL
        )
        .unwrap()
        .is_empty()
    );
    clean(&path);
}
