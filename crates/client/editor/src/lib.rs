//! UI-independent state for the map editor.
//!
//! This crate owns no window, renderer or connection. Its asset catalogue reads
//! the client's tile definitions once, searches their small text records, and
//! decodes art only when the presentation layer asks for one visible preview.

pub mod draft;
pub mod tools;

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use openshard_protocol::wire::Graphic;
use openshard_tiles::{LAND_TILE_COUNT, LandTileId, STATIC_TILE_COUNT, TileData};
use openshard_uofiles::art::{Art, ArtError};
use openshard_uofiles::image::Image;
use openshard_uofiles::tiledata::TileDataError;

/// Which half of the editor's asset catalogue an entry belongs to.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum AssetKind {
    /// Ground art and its land tile definition.
    Land,
    /// Art placed on top of the ground.
    Static,
}

/// An asset that can be painted or placed by the editor.
///
/// The enum keeps equal raw numbers in the two tiledata tables distinct. A
/// land tile can therefore never reach a static placement operation by
/// accident, even while an unfiltered catalogue displays both kinds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum AssetId {
    /// An entry in tiledata's land table.
    Land(LandTileId),
    /// An entry in tiledata's static table and static art index.
    Static(Graphic),
}

impl AssetId {
    /// Which catalogue and map operation this id belongs to.
    #[must_use]
    pub const fn kind(self) -> AssetKind {
        match self {
            Self::Land(_) => AssetKind::Land,
            Self::Static(_) => AssetKind::Static,
        }
    }

    /// The number shown by an asset palette.
    ///
    /// This deliberately does not convert back into another id type: callers
    /// use it for labels, while map operations retain the typed [`AssetId`].
    #[must_use]
    pub const fn raw(self) -> u16 {
        match self {
            Self::Land(id) => id.0,
            Self::Static(graphic) => graphic.0,
        }
    }
}

/// Which kinds an asset palette currently displays.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum KindFilter {
    /// Land and statics, in that order.
    #[default]
    All,
    /// Land only.
    Land,
    /// Statics only.
    Static,
}

impl KindFilter {
    const fn includes(self, kind: AssetKind) -> bool {
        matches!(self, Self::All)
            || matches!(
                (self, kind),
                (Self::Land, AssetKind::Land) | (Self::Static, AssetKind::Static)
            )
    }
}

/// UI-independent state retained by an asset palette.
///
/// An egui pane can edit [`Self::search_mut`] directly, recompute
/// [`Catalog::matching`] only when this state changes, and then request entries
/// and previews for the visible slice of the returned ids.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct PaletteState {
    search: String,
    filter: KindFilter,
    selected: Option<AssetId>,
    favorites: BTreeSet<AssetId>,
    favorites_only: bool,
}

impl PaletteState {
    /// The current search text.
    #[must_use]
    pub fn search(&self) -> &str {
        &self.search
    }

    /// Edit the search text in place, as a text widget expects.
    pub fn search_mut(&mut self) -> &mut String {
        &mut self.search
    }

    /// Replace the current search text.
    pub fn set_search(&mut self, search: String) {
        self.search = search;
    }

    /// The current land/static filter.
    #[must_use]
    pub const fn filter(&self) -> KindFilter {
        self.filter
    }

    /// Show one or both asset kinds.
    pub fn set_filter(&mut self, filter: KindFilter) {
        self.filter = filter;
    }

    /// The asset the next editor operation will use.
    #[must_use]
    pub const fn selected(&self) -> Option<AssetId> {
        self.selected
    }

    /// Select an asset for the active editor operation.
    pub fn select(&mut self, asset: AssetId) {
        self.selected = Some(asset);
    }

    /// Leave the active editor operation without an asset.
    pub fn clear_selection(&mut self) {
        self.selected = None;
    }

    /// Whether the palette is restricted to starred assets.
    #[must_use]
    pub const fn favorites_only(&self) -> bool {
        self.favorites_only
    }

    /// Restrict results to starred assets, or return to the full catalogue.
    pub fn set_favorites_only(&mut self, favorites_only: bool) {
        self.favorites_only = favorites_only;
    }

    /// Whether one asset is starred.
    #[must_use]
    pub fn is_favorite(&self, asset: AssetId) -> bool {
        self.favorites.contains(&asset)
    }

    /// Star an asset, or remove its star if it was already present.
    ///
    /// Returns whether the asset is starred after the change.
    pub fn toggle_favorite(&mut self, asset: AssetId) -> bool {
        if self.favorites.remove(&asset) {
            false
        } else {
            self.favorites.insert(asset);
            true
        }
    }
}

/// The searchable metadata for one catalogue row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CatalogEntry<'a> {
    /// Its typed tiledata/art id.
    pub id: AssetId,
    /// The client-supplied name, absent for empty and `NoName` records.
    pub name: Option<&'a str>,
}

/// Tile definitions and lazily decoded art from one client installation.
#[derive(Debug)]
pub struct Catalog {
    tiles: TileData,
    art: Art,
}

impl Catalog {
    /// Open the tile definitions and art archive in a client directory.
    pub fn open(client_dir: impl AsRef<Path>) -> Result<Self, CatalogError> {
        let client_dir = client_dir.as_ref();
        let tiles = openshard_uofiles::tiledata::load_tiles(client_dir.join("tiledata.mul"))?;
        let art = Art::open(client_dir)?;
        Ok(Self { tiles, art })
    }

    /// Build a catalogue from sources that were already loaded.
    ///
    /// This is useful when the application shares its boot-time files with the
    /// editor, and lets tests use the two sources' empty fixtures without an
    /// invented catalogue backend.
    #[must_use]
    pub const fn new(tiles: TileData, art: Art) -> Self {
        Self { tiles, art }
    }

    /// Metadata for one row, without touching the art archive.
    #[must_use]
    pub fn entry(&self, id: AssetId) -> CatalogEntry<'_> {
        let name = match id {
            AssetId::Land(id) => defined_name(&self.tiles.land(id.0).name),
            AssetId::Static(graphic) => self.tiles.item_name(graphic.0),
        };
        CatalogEntry { id, name }
    }

    /// IDs matching the palette state, in stable land-then-static order.
    ///
    /// This scans tiledata names only. It performs no archive lookup and
    /// decodes no image, so the caller can cache the small result vector until
    /// the palette state changes and virtualize its visible rows.
    #[must_use]
    pub fn matching(&self, state: &PaletteState) -> Vec<AssetId> {
        let query = Query::parse(state.search.trim());
        let capacity = match state.filter {
            KindFilter::All => LAND_TILE_COUNT + STATIC_TILE_COUNT,
            KindFilter::Land => LAND_TILE_COUNT,
            KindFilter::Static => STATIC_TILE_COUNT,
        };
        let mut matches = Vec::with_capacity(capacity);

        if state.filter.includes(AssetKind::Land) {
            for raw in 0..LAND_TILE_COUNT as u16 {
                let id = AssetId::Land(LandTileId(raw));
                if self.matches(state, query, id) {
                    matches.push(id);
                }
            }
        }
        if state.filter.includes(AssetKind::Static) {
            // The static id space contains every `u16`; an inclusive range is
            // the only range that can represent its final entry.
            for raw in u16::MIN..=u16::MAX {
                let id = AssetId::Static(Graphic(raw));
                if self.matches(state, query, id) {
                    matches.push(id);
                }
            }
        }

        matches
    }

    /// Decode one visible asset's preview from the archive.
    ///
    /// Missing art is ordinary and returns `Ok(None)`. No other method in this
    /// crate reads an art entry, so constructing and searching the full
    /// catalogue never decodes its roughly eighty thousand possible images.
    pub fn preview(&self, id: AssetId) -> Result<Option<Image>, ArtError> {
        match id {
            // Art's container uses one combined graphic index even though
            // tiledata keeps this half typed as `LandTileId`.
            AssetId::Land(id) => self.art.land(Graphic(id.0)),
            AssetId::Static(graphic) => self.art.static_art(graphic),
        }
    }

    fn matches(&self, state: &PaletteState, query: Query<'_>, id: AssetId) -> bool {
        if state.favorites_only && !state.favorites.contains(&id) {
            return false;
        }
        match query {
            Query::Everything => true,
            Query::Id(raw) => id.raw() == raw,
            Query::Name(needle) => self
                .entry(id)
                .name
                .is_some_and(|name| contains_ascii_case_insensitive(name, needle)),
            Query::InvalidId => false,
        }
    }
}

/// The client files required by the catalogue could not be opened.
#[derive(Debug)]
#[non_exhaustive]
pub enum CatalogError {
    /// `tiledata.mul` could not be loaded.
    TileData(TileDataError),
    /// `artLegacyMUL.uop` could not be opened.
    Art(ArtError),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TileData(source) => {
                write!(f, "cannot load the asset catalogue's tile definitions: {source}")
            }
            Self::Art(source) => write!(f, "cannot load the asset catalogue's art: {source}"),
        }
    }
}

impl std::error::Error for CatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TileData(source) => Some(source),
            Self::Art(source) => Some(source),
        }
    }
}

impl From<TileDataError> for CatalogError {
    fn from(source: TileDataError) -> Self {
        Self::TileData(source)
    }
}

impl From<ArtError> for CatalogError {
    fn from(source: ArtError) -> Self {
        Self::Art(source)
    }
}

#[derive(Clone, Copy)]
enum Query<'a> {
    Everything,
    Id(u16),
    Name(&'a str),
    InvalidId,
}

impl<'a> Query<'a> {
    fn parse(text: &'a str) -> Self {
        if text.is_empty() {
            return Self::Everything;
        }
        if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            return u16::from_str_radix(hex, 16).map_or(Self::InvalidId, Self::Id);
        }
        if text.bytes().all(|byte| byte.is_ascii_digit()) {
            return text.parse().map_or(Self::InvalidId, Self::Id);
        }
        Self::Name(text)
    }
}

fn defined_name(name: &str) -> Option<&str> {
    (!name.is_empty() && name != "NoName").then_some(name)
}

fn contains_ascii_case_insensitive(name: &str, needle: &str) -> bool {
    if needle.len() > name.len() {
        return false;
    }
    name.as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

#[cfg(test)]
mod tests {
    use openshard_tiles::{LandTile, StaticTile};

    use super::*;

    fn catalogue() -> Catalog {
        let mut tiles = TileData::empty();
        tiles.set_land_tile(
            42,
            LandTile {
                name: "Forest Floor".to_owned(),
                ..LandTile::default()
            },
        );
        tiles.set_land_tile(
            77,
            LandTile {
                name: "NoName".to_owned(),
                ..LandTile::default()
            },
        );
        tiles.set_static_tile(
            42,
            StaticTile {
                name: "Forest Arch".to_owned(),
                ..StaticTile::default()
            },
        );
        tiles.set_static_tile(
            500,
            StaticTile {
                name: "Stone Chair".to_owned(),
                ..StaticTile::default()
            },
        );
        Catalog::new(tiles, Art::empty())
    }

    #[test]
    fn equal_raw_ids_remain_distinct_assets() {
        let land = AssetId::Land(LandTileId(42));
        let static_art = AssetId::Static(Graphic(42));

        assert_ne!(
            land, static_art,
            "the two tiledata tables do not share an id space"
        );
        assert_eq!(land.kind(), AssetKind::Land);
        assert_eq!(static_art.kind(), AssetKind::Static);
    }

    #[test]
    fn decimal_and_prefixed_hex_queries_match_ids_exactly() {
        let catalogue = catalogue();
        let mut state = PaletteState::default();

        state.set_search("42".to_owned());
        assert_eq!(
            catalogue.matching(&state),
            vec![AssetId::Land(LandTileId(42)), AssetId::Static(Graphic(42))],
            "an all-kinds id query retains both typed matches"
        );

        state.set_search("0x01F4".to_owned());
        assert_eq!(
            catalogue.matching(&state),
            vec![AssetId::Land(LandTileId(500)), AssetId::Static(Graphic(500))],
            "a prefixed id is hexadecimal even when it contains digits only"
        );
    }

    #[test]
    fn an_id_outside_u16_matches_nothing_instead_of_becoming_a_name() {
        let catalogue = catalogue();
        let mut state = PaletteState::default();
        state.set_search("65536".to_owned());

        assert!(
            catalogue.matching(&state).is_empty(),
            "an invalid numeric query has no id match"
        );
    }

    #[test]
    fn the_last_static_id_is_searchable() {
        let catalogue = catalogue();
        let mut state = PaletteState::default();
        state.set_filter(KindFilter::Static);
        state.set_search("0XFFFF".to_owned());

        assert_eq!(
            catalogue.matching(&state),
            vec![AssetId::Static(Graphic(u16::MAX))],
            "the inclusive static id space retains its last entry"
        );
    }

    #[test]
    fn name_queries_are_ascii_case_insensitive_substrings() {
        let catalogue = catalogue();
        let mut state = PaletteState::default();
        state.set_search("fOrEsT".to_owned());

        assert_eq!(
            catalogue.matching(&state),
            vec![AssetId::Land(LandTileId(42)), AssetId::Static(Graphic(42))]
        );
    }

    #[test]
    fn kind_filter_is_applied_before_results_reach_the_palette() {
        let catalogue = catalogue();
        let mut state = PaletteState::default();
        state.set_search("forest".to_owned());
        state.set_filter(KindFilter::Static);

        assert_eq!(catalogue.matching(&state), vec![AssetId::Static(Graphic(42))]);
    }

    #[test]
    fn empty_and_noname_records_have_no_searchable_name() {
        let catalogue = catalogue();
        assert_eq!(catalogue.entry(AssetId::Land(LandTileId(77))).name, None);
        assert_eq!(catalogue.entry(AssetId::Static(Graphic(77))).name, None);

        let mut state = PaletteState::default();
        state.set_search("noname".to_owned());
        assert!(catalogue.matching(&state).is_empty());
    }

    #[test]
    fn favorites_filter_compounds_with_kind_and_text() {
        let catalogue = catalogue();
        let land = AssetId::Land(LandTileId(42));
        let static_art = AssetId::Static(Graphic(42));
        let mut state = PaletteState::default();
        assert!(state.toggle_favorite(land), "the first toggle stars an asset");
        assert!(
            state.toggle_favorite(static_art),
            "each typed id has its own star"
        );
        state.set_favorites_only(true);
        state.set_search("forest".to_owned());
        state.set_filter(KindFilter::Land);

        assert_eq!(catalogue.matching(&state), vec![land]);
        assert!(!state.toggle_favorite(land), "the second toggle removes the star");
        assert!(catalogue.matching(&state).is_empty());
    }

    #[test]
    fn selection_preserves_the_typed_asset() {
        let mut state = PaletteState::default();
        let selected = AssetId::Static(Graphic(500));
        state.select(selected);
        assert_eq!(state.selected(), Some(selected));

        state.clear_selection();
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn an_empty_art_source_decodes_nothing_during_search_or_preview() {
        let catalogue = catalogue();
        let mut state = PaletteState::default();
        state.set_search("forest".to_owned());
        assert_eq!(catalogue.matching(&state).len(), 2);

        assert!(
            catalogue
                .preview(AssetId::Land(LandTileId(42)))
                .unwrap()
                .is_none()
        );
        assert!(catalogue.preview(AssetId::Static(Graphic(42))).unwrap().is_none());
    }
}
