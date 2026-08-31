//! Scratch: what the facing detector sees in a graphic.
use openshard_client_render::facing;
use openshard_protocol::wire::Graphic;
use openshard_uofiles::art::Art;

fn main() {
    let dir = std::path::PathBuf::from(std::env::var_os("OPENSHARD_CLIENT").expect("client"));
    let art = Art::open(&dir).expect("art");
    for arg in std::env::args().skip(1) {
        let id: u16 = arg.parse().expect("graphic");
        let Ok(Some(image)) = art.static_art(Graphic(id)) else {
            println!("{id}: no art");
            continue;
        };
        println!(
            "{id} (0x{id:04X})  {}x{}  facing {:?}  hole {:?}",
            image.width(),
            image.height(),
            facing::facing_of(&image),
            facing::facing_of(&image).and_then(|f| facing::aperture_of(&image, f)),
        );
        // **Both edges, and the bottom one is the verdict's own input.**
        // `facing::facing_of` reads `base_edge`'s *bottom* row per column and
        // asks whether it is a straight 45° run confined to one half of the
        // tile's column — the top row is never looked at. This printed only the
        // top for a session, which is how a reader ends up explaining a `None`
        // from the wrong line.
        let mut tops = Vec::new();
        let mut bottoms = Vec::new();
        for x in 0..image.width() {
            let drawn = |y: &u16| image.pixel(x, *y).is_some_and(|px| px.0 != 0);
            let cell = |row: Option<u16>| {
                match row {
                    Some(y) => format!("{y:>3}"),
                    None => "  .".to_string(),
                }
            };
            tops.push(cell((0..image.height()).find(drawn)));
            bottoms.push(cell((0..image.height()).rev().find(drawn)));
        }
        println!("  top per column:    {}", tops.join(""));
        println!("  bottom per column: {}", bottoms.join(""));
    }
}
