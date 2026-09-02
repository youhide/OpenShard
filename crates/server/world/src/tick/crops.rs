use openshard_state::components::Crop;

use super::*;

impl World {
    /// Keep every crop field standing in plants. Once per tick, and cheap for the
    /// same reason [`maintain_spawners`](World::maintain_spawners) is: a field not
    /// yet due is one counter check, and only a due one that is short a plant does
    /// the work of counting.
    ///
    /// One plant per field per pass, so a field picked bare regrows at its own
    /// pace rather than snapping back full. Deterministic — the tile and the art
    /// both draw on the world's seeded rng.
    pub(super) fn maintain_crops(&mut self) {
        let now = self.state.ticks;
        if !self.crop_fields.iter().any(|field| now >= field.next_plant) {
            return;
        }
        let lod = self.state.gameplay.lod;
        let lod_radius = self.state.gameplay.lod_radius;
        for index in 0..self.crop_fields.len() {
            let field = &self.crop_fields[index];
            if now < field.next_plant || self.standing_in(index).len() >= usize::from(field.max_count) {
                continue;
            }
            // Level of detail: a field nobody is near need not be kept planted.
            // Its timer is held rather than advanced, so it fills when a player
            // arrives — the spawn region's rule, and the same reach.
            if lod {
                let area = field.area;
                let centre = Point::new(
                    area.x.wrapping_add(area.width / 2),
                    area.y.wrapping_add(area.height / 2),
                    0,
                );
                let reach = lod_radius + u32::from(area.width.max(area.height));
                if !self.state.any_player_near(centre, reach, area.facet) {
                    continue;
                }
            }
            let delay = self.crop_fields[index].respawn_delay;
            if self.plant_one(index) {
                self.crop_fields[index].next_plant = now + delay;
            }
        }
    }

    /// Put one plant on a free tile of a field. Returns whether one went in.
    ///
    /// A tile already carrying a plant is left alone and nothing is planted this
    /// pass — a field is a few plants in a few hundred tiles, so the collision is
    /// rare, and re-drawing until a free tile turns up would be an unbounded loop
    /// inside the tick for a case that costs one pass to fix.
    fn plant_one(&mut self, index: usize) -> bool {
        let field = &self.crop_fields[index];
        let area = field.area;
        let crop = field.crop;
        let taken = self.standing_in(index);
        let dx = self.state.rng.below(u32::from(area.width.max(1)));
        let dy = self.state.rng.below(u32::from(area.height.max(1)));
        let x = area.x.wrapping_add(dx as u16);
        let y = area.y.wrapping_add(dy as u16);
        if taken.iter().any(|at| at.x == x && at.y == y) {
            return false;
        }
        // The ground, as the spawn region does it: a field is farmland and a
        // plant belongs on the soil, not on the storey above it. A flat default
        // where there is no map at all.
        let z = self
            .state
            .map_terrain(area.facet)
            .and_then(|terrain| terrain.ground_z(Tile::new(x, y)))
            .unwrap_or(0);
        let planted = items::plant(&mut self.state, crop, Point::new(x, y, z), area.facet).is_some();
        if planted {
            debug!(field = %self.crop_fields[index].name, x, y, "crop planted");
        }
        planted
    }

    /// Where this field's crop is standing right now.
    ///
    /// By *box and crop* rather than by a tag on the plant: a plant is scenery
    /// the world re-lays rather than a spawn that is saved, so there is no id for
    /// it to carry. Exact as long as no two fields of one crop overlap, which
    /// `build.rs` refuses to let the data do.
    fn standing_in(&self, index: usize) -> Vec<Point> {
        let field = &self.crop_fields[index];
        let area = field.area;
        let east = area.x.saturating_add(area.width);
        let south = area.y.saturating_add(area.height);
        self.state
            .registry
            .query::<Crop>()
            .filter(|(_, crop)| matches!(crop, Crop::Standing(kind) if *kind == field.crop))
            .filter(|(entity, _)| self.state.facet_of(*entity) == area.facet)
            .filter_map(|(entity, _)| self.state.registry.get::<Position>(entity).map(|at| at.0))
            .filter(|at| (area.x..east).contains(&at.x) && (area.y..south).contains(&at.y))
            .collect()
    }

    /// Register a crop field, unless the same field is already standing.
    ///
    /// A fresh field is planted **full**, which is ServUO's own `Respawn` on a
    /// region loading: a shard that has just laid its world should not hand the
    /// first player to reach the farm an empty patch of soil. A field already
    /// standing keeps its plants and its timer, so a second `populate:` — the one
    /// every boot runs — neither doubles the ceiling nor re-sows the field.
    pub(super) fn register_crop_field(&mut self, field: crate::crops::CropField) {
        if self
            .crop_fields
            .iter()
            .any(|standing| standing.is_the_same_field(&field))
        {
            return;
        }
        let ceiling = field.max_count;
        let delay = field.respawn_delay;
        let index = self.crop_fields.len();
        self.crop_fields.push(field);
        // One attempt per plant rather than a search: a tile collision leaves the
        // field one short and the ordinary regrowth fills it within a pass.
        for _ in 0..ceiling {
            self.plant_one(index);
        }
        // And the pace starts here, because the field has just planted. Leaving
        // the timer at zero would make the *next* pick regrow within the tick it
        // happened in, whatever the data says the delay is.
        self.crop_fields[index].next_plant = self.state.ticks + delay;
    }

    /// Drop every crop field and every plant standing in one — the crop half of
    /// the "clear spawns" reset, which clears content the same `populate:` laid.
    pub(super) fn clear_crop_fields(&mut self) {
        self.crop_fields.clear();
        let plants: Vec<EntityId> = self
            .state
            .registry
            .query::<Crop>()
            .map(|(entity, _)| entity)
            .collect();
        for plant in plants {
            let Some(serial) = self.state.registry.serial_of(plant) else {
                continue;
            };
            items::remove_ground_item(&mut self.state, plant, serial);
        }
    }
}
