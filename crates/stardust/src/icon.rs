use crate::sprites;
use stardust_core::pet::{tray_edge_for_px, TrayGlyph};

/// Tray glyph for one pet state, at 16, 32, or 64 pixels.
pub fn tray_rgba(glyph: TrayGlyph, edge_px: u32) -> (Vec<u8>, u32, u32) {
    let image = sprites::tray_image(glyph, tray_edge_for_px(edge_px).px());
    (image.pixels, image.width, image.height)
}
