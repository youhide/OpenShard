//! Whether a mobile can actually stand somewhere.
//!
//! `MapTerrain` reads only the map and `tiledata.mul`, which the client also
//! holds, so it lives in `openshard-movement` beside `find_path` — a client's
//! click-to-walk planner and this crate's tick both need the same answer, and a
//! client may not depend on `openshard-world`. This module keeps only the tests
//! that need something a client crate does not have.

pub use openshard_movement::{MAX_STEP_UP, MapTerrain, PLAYER_HEIGHT};

#[cfg(test)]
mod tests {
    use openshard_movement::MapTerrain;
    use openshard_protocol::wire::Layer;
    use openshard_tiles::TileData;

    /// Point `OPENSHARD_CLIENT` at a UO client install to run this. See
    /// `openshard_movement::terrain`'s own tests for the rest of the coverage —
    /// this one is here only because it needs `openshard-state`'s layer
    /// constants, which `openshard-movement` may not depend on.
    ///
    /// Only the table is read here, but it is read *through a terrain*, because
    /// what this pins is what a shard's own ground would answer.
    struct Install {
        map: openshard_map::map::WorldMap,
        tiles: TileData,
        /// A terrain borrows one — see [`MapTerrain::new`]. Nothing in this
        /// file reads a surface, but a terrain is what the table is read
        /// *through*, and that is the point of the fixture.
        spans: openshard_movement::spans::SpanIndex,
    }

    impl Install {
        fn terrain(&self) -> MapTerrain<'_> {
            MapTerrain::new(&self.map, &self.tiles, &self.spans)
        }
    }

    fn real_install() -> Option<Install> {
        let dir = std::path::PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
        if !dir.join("tiledata.mul").exists() {
            return None;
        }
        let map = openshard_uofiles::map::read_facet(&dir, 0).expect("the client's map0 should load");
        let tiles =
            openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata should load");
        let spans = openshard_movement::spans::SpanIndex::build(&map, &tiles);
        Some(Install { map, tiles, spans })
    }

    #[test]
    fn the_layer_byte_reads_the_hand_a_weapon_is_held_in() {
        // The quality byte sits between the weight and the height, both of which
        // are already read, so an off-by-one here would report a plausible layer
        // for every item in the game. Pinned against a real file: a halberd,
        // bardiche, quarter staff and spear take both hands, a katana and a dagger
        // one. The file is *wrong* about the bow (it files it one-handed), which is
        // the reason `weapon::weapon_layer` lets six classes override it.
        let Some(install) = real_install() else {
            return;
        };
        let terrain = install.terrain();
        assert_eq!(
            Layer(terrain.tiles().static_tile(0x13B2).layer),
            openshard_state::weapon::LAYER_ONE_HANDED,
            "the bow, which is why the override exists"
        );
        for graphic in [0x143E, 0x0F4D, 0x0E89, 0x0F62] {
            assert_eq!(
                Layer(terrain.tiles().static_tile(graphic).layer),
                openshard_state::weapon::LAYER_TWO_HANDED,
                "0x{graphic:04X} should be two-handed"
            );
        }
        for graphic in [0x13FF, 0x0F52, 0x0F61, 0x0F5C] {
            assert_eq!(
                Layer(terrain.tiles().static_tile(graphic).layer),
                openshard_state::weapon::LAYER_ONE_HANDED,
                "0x{graphic:04X} should be one-handed"
            );
        }
    }
}
