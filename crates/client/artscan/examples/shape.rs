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
        // The silhouette's top edge per column: where the picture starts.
        let mut tops = Vec::new();
        for x in 0..image.width() {
            let top = (0..image.height()).find(|y| image.pixel(x, *y).is_some_and(|px| px.0 != 0));
            tops.push(match top {
                Some(y) => format!("{y:>3}"),
                None => "  .".to_string(),
            });
        }
        println!("  top per column: {}", tops.join(""));
    }
}
