//! The tool-free craft catalogue request and its OpenShard-only data stream.
//!
//! A craft tool is still what *makes* an item, but it is a poor affordance for
//! learning the game: a player without tongs cannot even see what tongs would
//! let them make. This private `0xBF` request opens the read-only catalogue.

use crate::access::OPENSHARD_SUBCOMMANDS;
use crate::codec::{
    PacketReader,
    PacketWriter,
};
use crate::error::DecodeError;
use crate::gump::GumpId;
use crate::item_kind::{
    ItemKindId,
    MaterialId,
};
use crate::packet::{
    DecodePacket,
    EncodePacket,
    PacketLength,
    frame_body,
};
use crate::wire::{
    ClilocId,
    Graphic,
    Hue,
};

/// `0xBF.0xE015` — open the craft catalogue without selecting a tool.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpenCraftCatalogue;

impl OpenCraftCatalogue {
    /// The first private subcommand after the turn request.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 21;

    /// Read the empty body. Extra bytes are refused so a future extension must
    /// name its versioned shape instead of silently changing this request.
    pub(crate) fn decode_body(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        if reader.remaining() != 0 {
            return Err(DecodeError::UnknownValue {
                field: "craft catalogue body byte count",
                value: u32::try_from(reader.remaining()).unwrap_or(u32::MAX),
            });
        }
        Ok(Self)
    }

    /// Encode the complete extended request.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        frame_body(0xBF, PacketLength::Variable, |out| out.u16(Self::SUBCOMMAND))
    }
}

/// One material cell in a catalogue row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CraftCatalogueComponent {
    /// Dense selector in the shared static catalogue.
    pub stock_key: CraftKey,
    /// Durable input type for a migrated recipe row.
    pub item_kind: Option<ItemKindId>,
    /// Required material when that input is materialized.
    pub material:  Option<MaterialId>,
    pub graphic:   Graphic,
    pub hue:       Hue,
    /// Localized material name for a human-readable tooltip.
    pub name:      ClilocId,
    pub amount:    u16,
}

/// Dense input selector shared by generated recipes and compact stock snapshots.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CraftKey(pub u16);

/// Largest recursive source admitted to one realtime craft operation.
pub const MAX_CRAFT_SOURCE_ITEMS: usize = 125;

/// Find the dense key which represents one already-resolved recipe input.
#[must_use]
pub fn craft_key_for(
    kind: Option<(ItemKindId, Option<MaterialId>)>,
    graphic: Graphic,
    hue: Hue,
) -> Option<CraftKey> {
    CRAFT_STOCK_SELECTORS
        .iter()
        .position(|selector| {
            selector.graphic == graphic
                && selector.hue == hue
                && match kind {
                    Some((kind, material)) => selector.kind == Some(kind) && selector.material == material,
                    None => selector.kind.is_none(),
                }
        })
        .and_then(|index| u16::try_from(index).ok())
        .map(CraftKey)
}

/// What one dense key counts. A semantic selector also accepts an untyped item
/// with the exact legacy presentation pair during catalogue migration.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct CraftStockSelector {
    pub kind:     Option<ItemKindId>,
    pub material: Option<MaterialId>,
    pub graphic:  Graphic,
    pub hue:      Hue,
}

/// One skill gate used by client-owned readiness evaluation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CraftSkillRequirement {
    pub skill:   u8,
    pub minimum: u16,
}

/// Presentation row plus the compact facts which determine its readiness.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CraftCatalogueDefinitionRow {
    pub row:                CraftCatalogueRow,
    pub skill_requirements: Vec<CraftSkillRequirement>,
    pub needs:              u8,
}

include!(concat!(env!("OUT_DIR"), "/craft_catalogue.rs"));

/// Combat family used by an item which can be wielded. The catalogue keeps
/// this compact presentation data beside its recipe rather than requiring the
/// client to depend on server combat definitions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CraftWeaponKind {
    Slashing,
    Piercing,
    Bashing,
    Axe,
    Polearm,
    Staff,
    Ranged,
}

/// The concise combat facts a player needs while comparing crafted weapons.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CraftWeaponProperties {
    pub combat_skill: ClilocId,
    pub kind:         CraftWeaponKind,
    pub damage_min:   u16,
    pub damage_max:   u16,
    /// Milliseconds would be needless precision; the authoritative ML number
    /// is centiseconds and remains that unit on the wire.
    pub speed_centis: u16,
    /// A ranged weapon's distance in tiles. `None` means melee.
    pub range:        Option<u8>,
}

/// One locally-scrollable catalogue row.  It contains data, not coordinates:
/// the OpenShard client owns the table geometry, fitting and scroll position.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CraftCatalogueRow {
    /// The normal craft-gump reply id for opening this recipe's details.
    pub button:           u32,
    /// Staff-only reply id which creates the recipe result immediately. The
    /// client hides it from ordinary players and the shard rechecks authority.
    pub admin_button:     u32,
    pub result:           Graphic,
    pub result_hue:       Hue,
    /// Durable output type for a migrated recipe row.
    pub result_item_kind: Option<ItemKindId>,
    pub name:             ClilocId,
    pub skill:            ClilocId,
    /// The lowest effective skill value allowed to attempt the primary skill
    /// check, in tenths of a percent.
    pub skill_min:        u16,
    pub ready:            bool,
    pub weapon:           Option<CraftWeaponProperties>,
    pub components:       Vec<CraftCatalogueComponent>,
}

/// `0xBF.0xE016` — the complete compact data model for a craft catalogue.
///
/// This deliberately travels outside `0xB0`: a gump layout is capped at a
/// `u16` byte count, while a full catalogue expressed as ordinary gump rows
/// would overflow it before the client could scroll locally.
#[derive(Clone, Debug)]
pub struct CraftCatalogue {
    /// The gump shell this data belongs to.
    pub gump_id: GumpId,
    /// Monotonic per-connection open identity; stale worker results are dropped.
    pub request_id: u32,
    pub catalogue_revision: u64,
    pub craft_projection_revision: u64,
    pub backpack_revision: u64,
    pub has_pack: bool,
    /// Present facilities as the forge/anvil/heat/oven/mill/water bitset.
    pub facilities: u8,
    /// `(Skill::id, effective tenths)` for the skills referenced by recipes.
    pub skills: Vec<(u8, u16)>,
    /// Totals in [`CRAFT_STOCK_SELECTORS`] order.
    pub amounts: Vec<u32>,
    /// Materialized locally after decoding; never serialized by this packet.
    pub rows: Vec<CraftCatalogueRow>,
}

impl PartialEq for CraftCatalogue {
    fn eq(&self, other: &Self) -> bool {
        self.gump_id == other.gump_id
            && self.request_id == other.request_id
            && self.catalogue_revision == other.catalogue_revision
            && self.craft_projection_revision == other.craft_projection_revision
            && self.backpack_revision == other.backpack_revision
            && self.has_pack == other.has_pack
            && self.facilities == other.facilities
            && self.skills == other.skills
            && self.amounts == other.amounts
    }
}

impl Eq for CraftCatalogue {
}

impl CraftCatalogue {
    fn materialize_rows(&mut self) {
        let skill = |id| {
            self.skills
                .iter()
                .find_map(|&(found, value)| (found == id).then_some(value))
                .unwrap_or_default()
        };
        self.rows = craft_catalogue_definitions()
            .into_iter()
            .map(|mut definition| {
                let skills_ready = definition
                    .skill_requirements
                    .iter()
                    .all(|requirement| skill(requirement.skill) >= requirement.minimum);
                let facilities_ready = self.facilities & definition.needs == definition.needs;
                let mut wanted = std::collections::BTreeMap::<CraftKey, u32>::new();
                for component in &definition.row.components {
                    *wanted.entry(component.stock_key).or_insert(0) += u32::from(component.amount);
                }
                let materials_ready = self.has_pack
                    && wanted.into_iter().all(|(key, amount)| {
                        self.amounts.get(usize::from(key.0)).copied().unwrap_or_default() >= amount
                    });
                definition.row.ready = skills_ready && facilities_ready && materials_ready;
                definition.row
            })
            .collect();
    }
}

/// A localized label used by the interactive craft workbench.
///
/// Most crafting data is a cliloc, but a few of ServUO's craft rows are
/// literal strings.  Keeping that distinction on the wire means the egui
/// client never has to guess whether `0` is a missing label or actual text.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CraftText {
    Cliloc(ClilocId),
    Literal(String),
}

/// One material or result cell in the interactive workbench.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CraftWorkbenchComponent {
    /// Durable definition identity when the recipe row has been migrated.
    /// Art below remains a rendering projection.
    pub item_kind: Option<ItemKindId>,
    pub graphic:   Graphic,
    pub hue:       Hue,
    pub name:      CraftText,
    pub amount:    u16,
    /// The amount currently in the player's pack. `None` is used for a result.
    pub carried:   Option<u32>,
}

/// A category button owned by the server's existing craft-gump reply scheme.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CraftWorkbenchGroup {
    pub button:   u32,
    pub name:     CraftText,
    pub selected: bool,
}

/// A material-axis choice, including its live pack count.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CraftWorkbenchMaterial {
    pub button:    u32,
    /// Resource kind this axis selects (for example ingot, board or leather).
    /// `None` keeps the packet compatible with an unaudited legacy axis.
    pub item_kind: Option<ItemKindId>,
    /// Durable material identity. `None` is an unregistered legacy axis row;
    /// graphic/hue below remain only its rendering projection.
    pub material:  Option<MaterialId>,
    pub graphic:   Graphic,
    pub hue:       Hue,
    pub name:      CraftText,
    pub carried:   u32,
    pub selected:  bool,
}

/// A craft recipe as presented by a tool-specific workbench.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CraftWorkbenchRecipe {
    pub make_button:       Option<u32>,
    pub details_button:    Option<u32>,
    /// Present only when the server says this viewer may use the immediate
    /// administrator construction path.
    pub admin_button:      Option<u32>,
    pub result:            CraftWorkbenchComponent,
    pub skills:            Vec<(CraftText, u16)>,
    pub components:        Vec<CraftWorkbenchComponent>,
    pub use_all_resources: bool,
    pub markable:          bool,
}

/// The page the interactive workbench is currently showing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CraftWorkbenchPage {
    Items {
        recipes: Vec<CraftWorkbenchRecipe>,
    },
    Resources {
        materials: Vec<CraftWorkbenchMaterial>,
    },
    Details {
        recipe:                CraftWorkbenchRecipe,
        success_per_mille:     u16,
        exceptional_per_mille: Option<u16>,
    },
}

/// A compact, client-owned representation of a normal craft-tool window.
///
/// Buttons remain raw `0xB1` reply ids. The server therefore continues to own
/// every validation and state transition while egui owns geometry, scrolling,
/// and presentation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CraftWorkbench {
    pub gump_id:             GumpId,
    pub title:               CraftText,
    pub groups:              Vec<CraftWorkbenchGroup>,
    pub selected_material:   Option<CraftWorkbenchMaterial>,
    pub tool_uses:           Option<u16>,
    pub tool_carried:        bool,
    /// Bit set: forge, anvil, fire, oven, mill, water respectively.
    pub required_facilities: u8,
    pub present_facilities:  u8,
    pub notice:              Option<CraftText>,
    pub materials_button:    Option<u32>,
    pub refresh_button:      u32,
    pub cancel_button:       u32,
    pub page:                CraftWorkbenchPage,
}

impl CraftWorkbench {
    pub const ID: u8 = 0xBF;
    /// Kept adjacent to the catalogue stream: both are client-owned views of
    /// a server-owned craft context.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 23;
}

fn write_craft_text(out: &mut PacketWriter, text: &CraftText) {
    match text {
        CraftText::Cliloc(id) => {
            out.u8(0);
            out.u32(id.0);
        }
        CraftText::Literal(value) => {
            out.u8(1);
            out.null_terminated_string(value);
        }
    }
}

fn read_craft_text(reader: &mut PacketReader<'_>) -> Result<CraftText, DecodeError> {
    match reader.u8()? {
        0 => Ok(CraftText::Cliloc(ClilocId(reader.u32()?))),
        1 => Ok(CraftText::Literal(reader.null_terminated_string()?)),
        value => {
            Err(DecodeError::UnknownValue {
                field: "craft text kind",
                value: u32::from(value),
            })
        }
    }
}

fn write_component(out: &mut PacketWriter, component: &CraftWorkbenchComponent) {
    out.u32(component.item_kind.map_or(0, |kind| kind.0));
    out.u16(component.graphic.0);
    out.u16(component.hue.0);
    write_craft_text(out, &component.name);
    out.u16(component.amount);
    match component.carried {
        Some(amount) => {
            out.u8(1);
            out.u32(amount);
        }
        None => out.u8(0),
    }
}

fn read_component(reader: &mut PacketReader<'_>) -> Result<CraftWorkbenchComponent, DecodeError> {
    let item_kind = ItemKindId::new(reader.u32()?);
    let graphic = Graphic(reader.u16()?);
    let hue = Hue(reader.u16()?);
    let name = read_craft_text(reader)?;
    let amount = reader.u16()?;
    let carried = match reader.u8()? {
        0 => None,
        1 => Some(reader.u32()?),
        value => {
            return Err(DecodeError::UnknownValue {
                field: "craft component carried presence",
                value: u32::from(value),
            });
        }
    };
    Ok(CraftWorkbenchComponent {
        item_kind,
        graphic,
        hue,
        name,
        amount,
        carried,
    })
}

fn write_material(out: &mut PacketWriter, material: &CraftWorkbenchMaterial) {
    out.u32(material.button);
    out.u32(material.item_kind.map_or(0, |kind| kind.0));
    out.u16(material.material.map_or(0, |material| material.0));
    out.u16(material.graphic.0);
    out.u16(material.hue.0);
    write_craft_text(out, &material.name);
    out.u32(material.carried);
    out.bool(material.selected);
}

fn read_material(reader: &mut PacketReader<'_>) -> Result<CraftWorkbenchMaterial, DecodeError> {
    Ok(CraftWorkbenchMaterial {
        button:    reader.u32()?,
        item_kind: ItemKindId::new(reader.u32()?),
        material:  MaterialId::new(reader.u16()?),
        graphic:   Graphic(reader.u16()?),
        hue:       Hue(reader.u16()?),
        name:      read_craft_text(reader)?,
        carried:   reader.u32()?,
        selected:  reader.bool()?,
    })
}

fn write_recipe(out: &mut PacketWriter, recipe: &CraftWorkbenchRecipe) {
    match recipe.make_button {
        Some(button) => {
            out.u8(1);
            out.u32(button);
        }
        None => out.u8(0),
    }
    match recipe.details_button {
        Some(button) => {
            out.u8(1);
            out.u32(button);
        }
        None => out.u8(0),
    }
    match recipe.admin_button {
        Some(button) => {
            out.u8(1);
            out.u32(button);
        }
        None => out.u8(0),
    }
    write_component(out, &recipe.result);
    out.u8(u8::try_from(recipe.skills.len()).expect("craft recipe has at most 255 skills"));
    for (name, minimum) in &recipe.skills {
        write_craft_text(out, name);
        out.u16(*minimum);
    }
    out.u8(u8::try_from(recipe.components.len()).expect("craft recipe has at most 255 components"));
    for component in &recipe.components {
        write_component(out, component);
    }
    out.bool(recipe.use_all_resources);
    out.bool(recipe.markable);
}

fn read_recipe(reader: &mut PacketReader<'_>) -> Result<CraftWorkbenchRecipe, DecodeError> {
    let make_button = match reader.u8()? {
        0 => None,
        1 => Some(reader.u32()?),
        value => {
            return Err(DecodeError::UnknownValue {
                field: "craft make-button presence",
                value: u32::from(value),
            });
        }
    };
    let details_button = match reader.u8()? {
        0 => None,
        1 => Some(reader.u32()?),
        value => {
            return Err(DecodeError::UnknownValue {
                field: "craft details-button presence",
                value: u32::from(value),
            });
        }
    };
    let admin_button = match reader.u8()? {
        0 => None,
        1 => Some(reader.u32()?),
        value => {
            return Err(DecodeError::UnknownValue {
                field: "craft admin-button presence",
                value: u32::from(value),
            });
        }
    };
    let result = read_component(reader)?;
    let skills = (0..reader.u8()?)
        .map(|_| Ok((read_craft_text(reader)?, reader.u16()?)))
        .collect::<Result<Vec<_>, DecodeError>>()?;
    let components = (0..reader.u8()?)
        .map(|_| read_component(reader))
        .collect::<Result<Vec<_>, DecodeError>>()?;
    Ok(CraftWorkbenchRecipe {
        make_button,
        details_button,
        admin_button,
        result,
        skills,
        components,
        use_all_resources: reader.bool()?,
        markable: reader.bool()?,
    })
}

impl EncodePacket for CraftWorkbench {
    const ID: u8 = Self::ID;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _: crate::version::ClientVersion) {
        out.u16(Self::SUBCOMMAND);
        out.u32(self.gump_id.0);
        write_craft_text(out, &self.title);
        out.u8(u8::try_from(self.groups.len()).expect("craft system has at most 255 groups"));
        for group in &self.groups {
            out.u32(group.button);
            write_craft_text(out, &group.name);
            out.bool(group.selected);
        }
        match &self.selected_material {
            Some(material) => {
                out.u8(1);
                write_material(out, material);
            }
            None => out.u8(0),
        }
        match self.tool_uses {
            Some(uses) => {
                out.u8(1);
                out.u16(uses);
            }
            None => out.u8(0),
        }
        out.bool(self.tool_carried);
        out.u8(self.required_facilities);
        out.u8(self.present_facilities);
        match &self.notice {
            Some(notice) => {
                out.u8(1);
                write_craft_text(out, notice);
            }
            None => out.u8(0),
        }
        match self.materials_button {
            Some(button) => {
                out.u8(1);
                out.u32(button);
            }
            None => out.u8(0),
        }
        out.u32(self.refresh_button);
        out.u32(self.cancel_button);
        match &self.page {
            CraftWorkbenchPage::Items { recipes } => {
                out.u8(0);
                out.u16(u16::try_from(recipes.len()).expect("craft item list fits a u16"));
                for recipe in recipes {
                    write_recipe(out, recipe);
                }
            }
            CraftWorkbenchPage::Resources { materials } => {
                out.u8(1);
                out.u8(u8::try_from(materials.len()).expect("craft material list fits a u8"));
                for material in materials {
                    write_material(out, material);
                }
            }
            CraftWorkbenchPage::Details {
                recipe,
                success_per_mille,
                exceptional_per_mille,
            } => {
                out.u8(2);
                write_recipe(out, recipe);
                out.u16(*success_per_mille);
                match exceptional_per_mille {
                    Some(chance) => {
                        out.u8(1);
                        out.u16(*chance);
                    }
                    None => out.u8(0),
                }
            }
        }
    }
}

impl DecodePacket for CraftWorkbench {
    const ID: u8 = Self::ID;
    fn decode_body(
        reader: &mut PacketReader<'_>,
        _: crate::version::ClientVersion,
    ) -> Result<Self, DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(DecodeError::UnknownValue {
                field: "0xBF subcommand for a craft workbench",
                value: u32::from(subcommand),
            });
        }
        let gump_id = GumpId(reader.u32()?);
        let title = read_craft_text(reader)?;
        let groups = (0..reader.u8()?)
            .map(|_| {
                Ok(CraftWorkbenchGroup {
                    button:   reader.u32()?,
                    name:     read_craft_text(reader)?,
                    selected: reader.bool()?,
                })
            })
            .collect::<Result<Vec<_>, DecodeError>>()?;
        let selected_material = match reader.u8()? {
            0 => None,
            1 => Some(read_material(reader)?),
            value => {
                return Err(DecodeError::UnknownValue {
                    field: "craft selected material presence",
                    value: u32::from(value),
                });
            }
        };
        let tool_uses = match reader.u8()? {
            0 => None,
            1 => Some(reader.u16()?),
            value => {
                return Err(DecodeError::UnknownValue {
                    field: "craft tool presence",
                    value: u32::from(value),
                });
            }
        };
        let tool_carried = reader.bool()?;
        let required_facilities = reader.u8()?;
        let present_facilities = reader.u8()?;
        let notice = match reader.u8()? {
            0 => None,
            1 => Some(read_craft_text(reader)?),
            value => {
                return Err(DecodeError::UnknownValue {
                    field: "craft notice presence",
                    value: u32::from(value),
                });
            }
        };
        let materials_button = match reader.u8()? {
            0 => None,
            1 => Some(reader.u32()?),
            value => {
                return Err(DecodeError::UnknownValue {
                    field: "craft materials-button presence",
                    value: u32::from(value),
                });
            }
        };
        let refresh_button = reader.u32()?;
        let cancel_button = reader.u32()?;
        let page = match reader.u8()? {
            0 => {
                CraftWorkbenchPage::Items {
                    recipes: (0..reader.u16()?)
                        .map(|_| read_recipe(reader))
                        .collect::<Result<Vec<_>, DecodeError>>()?,
                }
            }
            1 => {
                CraftWorkbenchPage::Resources {
                    materials: (0..reader.u8()?)
                        .map(|_| read_material(reader))
                        .collect::<Result<Vec<_>, DecodeError>>()?,
                }
            }
            2 => {
                let recipe = read_recipe(reader)?;
                let success_per_mille = reader.u16()?;
                let exceptional_per_mille = match reader.u8()? {
                    0 => None,
                    1 => Some(reader.u16()?),
                    value => {
                        return Err(DecodeError::UnknownValue {
                            field: "craft exceptional chance presence",
                            value: u32::from(value),
                        });
                    }
                };
                CraftWorkbenchPage::Details {
                    recipe,
                    success_per_mille,
                    exceptional_per_mille,
                }
            }
            value => {
                return Err(DecodeError::UnknownValue {
                    field: "craft workbench page",
                    value: u32::from(value),
                });
            }
        };
        Ok(Self {
            gump_id,
            title,
            groups,
            selected_material,
            tool_uses,
            tool_carried,
            required_facilities,
            present_facilities,
            notice,
            materials_button,
            refresh_button,
            cancel_button,
            page,
        })
    }
}

impl CraftCatalogue {
    pub const ID: u8 = 0xBF;
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 22;
}

impl EncodePacket for CraftCatalogue {
    const ID: u8 = Self::ID;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: crate::version::ClientVersion) {
        out.u16(Self::SUBCOMMAND);
        out.u32(self.gump_id.0);
        out.u32(self.request_id);
        out.u64(self.catalogue_revision);
        out.u64(self.craft_projection_revision);
        out.u64(self.backpack_revision);
        out.u8(u8::from(self.has_pack));
        out.u8(self.facilities);
        out.u8(u8::try_from(self.skills.len()).expect("a craft skill context fits a u8 count"));
        for &(skill, value) in &self.skills {
            out.u8(skill);
            out.u16(value);
        }
        out.u16(u16::try_from(self.amounts.len()).expect("a craft stock context fits a u16 count"));
        for &amount in &self.amounts {
            out.u32(amount);
        }
    }
}

impl DecodePacket for CraftCatalogue {
    const ID: u8 = Self::ID;

    fn decode_body(
        reader: &mut PacketReader<'_>,
        _version: crate::version::ClientVersion,
    ) -> Result<Self, DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(DecodeError::UnknownValue {
                field: "0xBF subcommand for a craft catalogue",
                value: u32::from(subcommand),
            });
        }
        let gump_id = GumpId(reader.u32()?);
        let request_id = reader.u32()?;
        let catalogue_revision = reader.u64()?;
        if catalogue_revision != CRAFT_CATALOGUE_REVISION {
            return Err(DecodeError::UnknownValue {
                field: "craft catalogue revision",
                value: catalogue_revision as u32,
            });
        }
        let craft_projection_revision = reader.u64()?;
        let backpack_revision = reader.u64()?;
        let has_pack = reader.u8()? != 0;
        let facilities = reader.u8()?;
        let skills = (0..reader.u8()?)
            .map(|_| Ok((reader.u8()?, reader.u16()?)))
            .collect::<Result<Vec<_>, DecodeError>>()?;
        let amounts = (0..reader.u16()?)
            .map(|_| reader.u32().map_err(DecodeError::from))
            .collect::<Result<Vec<_>, DecodeError>>()?;
        if amounts.len() != CRAFT_KEY_COUNT {
            return Err(DecodeError::UnknownValue {
                field: "craft stock key count",
                value: u32::try_from(amounts.len()).unwrap_or(u32::MAX),
            });
        }
        let mut catalogue = Self {
            gump_id,
            request_id,
            catalogue_revision,
            craft_projection_revision,
            backpack_revision,
            has_pack,
            facilities,
            skills,
            amounts,
            rows: Vec::new(),
        };
        catalogue.materialize_rows();
        Ok(catalogue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extended::ExtendedRequest;
    use crate::gump::GumpId;
    use crate::packet::encode_packet;
    use crate::server_packet::ServerPacket;
    use crate::version::ClientVersion;
    use crate::wire::{
        ClilocId,
        Graphic,
        Hue,
    };

    #[test]
    fn the_catalogue_request_round_trips_through_the_extended_envelope() {
        assert_eq!(
            ExtendedRequest::decode(&OpenCraftCatalogue.encode()).unwrap(),
            ExtendedRequest::CraftCatalogue(OpenCraftCatalogue)
        );
    }

    #[test]
    fn compact_catalogue_context_materializes_the_static_rows_locally() {
        let sent = CraftCatalogue {
            gump_id: GumpId(0x00AD_0001),
            request_id: 17,
            catalogue_revision: CRAFT_CATALOGUE_REVISION,
            craft_projection_revision: 0,
            backpack_revision: 23,
            has_pack: true,
            facilities: 3,
            skills: vec![(7, 300)],
            amounts: vec![0; CRAFT_KEY_COUNT],
            rows: vec![CraftCatalogueRow {
                button:           8,
                admin_button:     9,
                result:           Graphic(0x13EB),
                result_hue:       Hue::NONE,
                result_item_kind: Some(ItemKindId(4)),
                name:             ClilocId(1_022_036),
                skill:            ClilocId(1_044_067),
                skill_min:        300,
                ready:            true,
                weapon:           Some(CraftWeaponProperties {
                    combat_skill: ClilocId(1_044_100),
                    kind:         CraftWeaponKind::Slashing,
                    damage_min:   11,
                    damage_max:   14,
                    speed_centis: 350,
                    range:        None,
                }),
                components:       vec![CraftCatalogueComponent {
                    stock_key: CraftKey(0),
                    item_kind: Some(ItemKindId(1)),
                    material:  Some(MaterialId(1)),
                    graphic:   Graphic(0x1BF2),
                    hue:       Hue::NONE,
                    name:      ClilocId(1_045_000),
                    amount:    3,
                }],
            }],
        };
        let bytes = encode_packet(&sent, ClientVersion::TOL);
        assert!(
            bytes.len() < 512,
            "opening the catalogue sends context, not 492 rows"
        );
        let Some(ServerPacket::CraftCatalogue(found)) =
            ServerPacket::decode(&bytes, ClientVersion::TOL).unwrap()
        else {
            panic!("the compact catalogue packet must decode");
        };
        assert_eq!(found, sent, "the wire-owned context round-trips");
        assert_eq!(found.rows.len(), CRAFT_RECIPE_LOCATIONS.len());
        assert_eq!(found.rows.len(), 492);
        assert!(
            found.rows.iter().all(|row| !row.ready),
            "zero stock keeps every locally materialized recipe unavailable"
        );
    }

    #[test]
    fn workbench_pages_round_trip_through_the_extended_envelope() {
        let sent = CraftWorkbench {
            gump_id:             GumpId(0x00AD_0001),
            title:               CraftText::Literal("Blacksmithy".to_owned()),
            groups:              vec![CraftWorkbenchGroup {
                button:   1,
                name:     CraftText::Cliloc(ClilocId(1_044_010)),
                selected: true,
            }],
            selected_material:   Some(CraftWorkbenchMaterial {
                button:    36,
                item_kind: Some(ItemKindId(1)),
                material:  Some(MaterialId(1)),
                graphic:   Graphic(0x1BF2),
                hue:       Hue::NONE,
                name:      CraftText::Literal("Iron".to_owned()),
                carried:   42,
                selected:  true,
            }),
            tool_uses:           Some(50),
            tool_carried:        true,
            required_facilities: 3,
            present_facilities:  1,
            notice:              Some(CraftText::Literal("An anvil is required.".to_owned())),
            materials_button:    Some(7),
            refresh_button:      14,
            cancel_button:       84,
            page:                CraftWorkbenchPage::Details {
                recipe:                CraftWorkbenchRecipe {
                    make_button:       Some(1),
                    details_button:    None,
                    admin_button:      Some(2),
                    result:            CraftWorkbenchComponent {
                        item_kind: Some(ItemKindId(4)),
                        graphic:   Graphic(0x13EB),
                        hue:       Hue::NONE,
                        name:      CraftText::Literal("Longsword".to_owned()),
                        amount:    1,
                        carried:   None,
                    },
                    skills:            vec![(CraftText::Literal("Blacksmithy".to_owned()), 300)],
                    components:        vec![CraftWorkbenchComponent {
                        item_kind: Some(ItemKindId(1)),
                        graphic:   Graphic(0x1BF2),
                        hue:       Hue::NONE,
                        name:      CraftText::Literal("Iron".to_owned()),
                        amount:    12,
                        carried:   Some(42),
                    }],
                    use_all_resources: false,
                    markable:          true,
                },
                success_per_mille:     675,
                exceptional_per_mille: Some(75),
            },
        };
        let bytes = encode_packet(&sent, ClientVersion::TOL);
        assert!(matches!(
            ServerPacket::decode(&bytes, ClientVersion::TOL),
            Ok(Some(ServerPacket::CraftWorkbench(found))) if found == sent
        ));
    }
}
