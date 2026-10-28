//! Embedded pet, bubble, and tray PNGs.
//!
//! Bytes are compiled in with `include_bytes!` and decoded once. Drawing uses
//! integer nearest-neighbour scaling: each source pixel becomes a solid block.

use png::{BitDepth, ColorType, Decoder, Transformations};
use stardust_core::pet::{
    pet_frame, tray_edge_for_px, PetPose, PetScale, PetSkin, TrayEdge, TrayGlyph,
};
use std::io::Cursor;
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

struct FaceSet {
    normal: RgbaImage,
    alert: RgbaImage,
    error: RgbaImage,
    sleep: RgbaImage,
    happy: RgbaImage,
}

impl FaceSet {
    fn get(&self, pose: PetPose) -> &RgbaImage {
        match pose {
            PetPose::Normal => &self.normal,
            PetPose::Alert => &self.alert,
            PetPose::Error => &self.error,
            PetPose::Sleep => &self.sleep,
            PetPose::Happy => &self.happy,
        }
    }
}

struct TraySet {
    normal_16: RgbaImage,
    normal_32: RgbaImage,
    normal_64: RgbaImage,
    alert_16: RgbaImage,
    alert_32: RgbaImage,
    alert_64: RgbaImage,
    error_16: RgbaImage,
    error_32: RgbaImage,
    error_64: RgbaImage,
}

impl TraySet {
    fn get(&self, glyph: TrayGlyph, edge: TrayEdge) -> &RgbaImage {
        match (glyph, edge) {
            (TrayGlyph::Normal, TrayEdge::Px16) => &self.normal_16,
            (TrayGlyph::Normal, TrayEdge::Px32) => &self.normal_32,
            (TrayGlyph::Normal, TrayEdge::Px64) => &self.normal_64,
            (TrayGlyph::Alert, TrayEdge::Px16) => &self.alert_16,
            (TrayGlyph::Alert, TrayEdge::Px32) => &self.alert_32,
            (TrayGlyph::Alert, TrayEdge::Px64) => &self.alert_64,
            (TrayGlyph::Error, TrayEdge::Px16) => &self.error_16,
            (TrayGlyph::Error, TrayEdge::Px32) => &self.error_32,
            (TrayGlyph::Error, TrayEdge::Px64) => &self.error_64,
        }
    }
}

struct Sources {
    astronaut: FaceSet,
    star: FaceSet,
    bubble_alert: RgbaImage,
    bubble_error: RgbaImage,
    bubble_sleep: RgbaImage,
    bubble_happy: RgbaImage,
    tray: TraySet,
}

impl Sources {
    fn pet(&self, skin: PetSkin, pose: PetPose) -> &RgbaImage {
        match skin {
            PetSkin::Astronaut => self.astronaut.get(pose),
            PetSkin::Star => self.star.get(pose),
        }
    }

    fn bubble(&self, pose: PetPose) -> Option<&RgbaImage> {
        match pose {
            PetPose::Normal => None,
            PetPose::Alert => Some(&self.bubble_alert),
            PetPose::Error => Some(&self.bubble_error),
            PetPose::Sleep => Some(&self.bubble_sleep),
            PetPose::Happy => Some(&self.bubble_happy),
        }
    }
}

pub fn render_pet(skin: PetSkin, pose: PetPose, scale: PetScale) -> RgbaImage {
    let frame = pet_frame(skin, scale);
    let mut canvas = RgbaImage::clear(frame.canvas.width, frame.canvas.height);
    let sources = sources();
    let pet = scale_nearest(sources.pet(skin, pose), scale.factor());
    blit(&mut canvas, &pet, frame.pet.x, frame.pet.y);
    if let Some(bubble) = sources.bubble(pose) {
        let bubble = scale_nearest(bubble, scale.factor());
        blit(&mut canvas, &bubble, frame.bubble.x, frame.bubble.y);
    }
    canvas
}

pub fn tray_image(glyph: TrayGlyph, edge_px: u32) -> RgbaImage {
    let edge = tray_edge_for_px(edge_px);
    sources().tray.get(glyph, edge).clone()
}

pub fn decode_png(bytes: &[u8]) -> Result<RgbaImage, String> {
    let mut decoder = Decoder::new(Cursor::new(bytes));
    decoder.set_limits(png::Limits { bytes: 1024 * 1024 });
    decoder.set_transformations(
        Transformations::EXPAND | Transformations::STRIP_16 | Transformations::ALPHA,
    );
    let mut reader = decoder.read_info().map_err(|err| err.to_string())?;
    let buffer_size = reader
        .output_buffer_size()
        .ok_or_else(|| "png is too large to decode".to_string())?;
    let mut buffer = vec![0; buffer_size];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|err| err.to_string())?;
    if info.bit_depth != BitDepth::Eight {
        return Err("png sample is not 8-bit".to_string());
    }
    let samples = info.color_type.samples();
    if samples == 0 || info.line_size < info.width as usize * samples {
        return Err("png row is shorter than its width".to_string());
    }
    let mut pixels = Vec::with_capacity((info.width * info.height * 4) as usize);
    for y in 0..info.height {
        let start = y as usize * info.line_size;
        let row = &buffer[start..start + info.width as usize * samples];
        match info.color_type {
            ColorType::Rgba => pixels.extend_from_slice(row),
            ColorType::Rgb => {
                for px in row.as_chunks::<3>().0 {
                    pixels.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
            }
            ColorType::GrayscaleAlpha => {
                for px in row.as_chunks::<2>().0 {
                    pixels.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
                }
            }
            ColorType::Grayscale | ColorType::Indexed => {
                for sample in row {
                    pixels.extend_from_slice(&[*sample, *sample, *sample, 255]);
                }
            }
        }
    }
    let expected = (info.width * info.height * 4) as usize;
    if pixels.len() != expected {
        return Err("png expanded to an unexpected size".to_string());
    }
    Ok(RgbaImage {
        width: info.width,
        height: info.height,
        pixels,
    })
}

#[cfg(test)]
fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut encoded = Vec::new();
    let mut encoder = png::Encoder::new(&mut encoded, image.width, image.height);
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|err| err.to_string())?;
    writer
        .write_image_data(&image.pixels)
        .map_err(|err| err.to_string())?;
    writer.finish().map_err(|err| err.to_string())?;
    Ok(encoded)
}

impl RgbaImage {
    fn clear(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height * 4) as usize],
        }
    }
}

fn scale_nearest(src: &RgbaImage, factor: u32) -> RgbaImage {
    if factor <= 1 {
        return src.clone();
    }
    let width = src.width * factor;
    let height = src.height * factor;
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        let sy = y / factor;
        for x in 0..width {
            let sx = x / factor;
            let from = ((sy * src.width + sx) * 4) as usize;
            let to = ((y * width + x) * 4) as usize;
            pixels[to..to + 4].copy_from_slice(&src.pixels[from..from + 4]);
        }
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

fn blit(dst: &mut RgbaImage, src: &RgbaImage, origin_x: u32, origin_y: u32) {
    for y in 0..src.height {
        let dy = origin_y + y;
        if dy >= dst.height {
            break;
        }
        for x in 0..src.width {
            let dx = origin_x + x;
            if dx >= dst.width {
                break;
            }
            let from = ((y * src.width + x) * 4) as usize;
            let src_px = [
                src.pixels[from],
                src.pixels[from + 1],
                src.pixels[from + 2],
                src.pixels[from + 3],
            ];
            let to = ((dy * dst.width + dx) * 4) as usize;
            let dst_px = [
                dst.pixels[to],
                dst.pixels[to + 1],
                dst.pixels[to + 2],
                dst.pixels[to + 3],
            ];
            let out = over(dst_px, src_px);
            dst.pixels[to..to + 4].copy_from_slice(&out);
        }
    }
}

fn over(dst: [u8; 4], src: [u8; 4]) -> [u8; 4] {
    let sa = u32::from(src[3]);
    if sa == 0 {
        return dst;
    }
    if sa == 255 || dst[3] == 0 {
        return src;
    }
    let da = u32::from(dst[3]);
    let inv = 255 - sa;
    let out_a = sa + da * inv / 255;
    if out_a == 0 {
        return [0, 0, 0, 0];
    }
    let mut out = [0u8; 4];
    for channel in 0..3 {
        let mixed = u32::from(src[channel]) * sa + u32::from(dst[channel]) * da * inv / 255;
        out[channel] = u8::try_from(mixed / out_a).unwrap_or(255);
    }
    out[3] = u8::try_from(out_a).unwrap_or(255);
    out
}

fn sources() -> &'static Sources {
    static SOURCES: OnceLock<Sources> = OnceLock::new();
    SOURCES.get_or_init(|| Sources {
        astronaut: FaceSet {
            normal: decode_square(pet_png(PetSkin::Astronaut, PetPose::Normal), 24),
            alert: decode_square(pet_png(PetSkin::Astronaut, PetPose::Alert), 24),
            error: decode_square(pet_png(PetSkin::Astronaut, PetPose::Error), 24),
            sleep: decode_square(pet_png(PetSkin::Astronaut, PetPose::Sleep), 24),
            happy: decode_square(pet_png(PetSkin::Astronaut, PetPose::Happy), 24),
        },
        star: FaceSet {
            normal: decode_square(pet_png(PetSkin::Star, PetPose::Normal), 32),
            alert: decode_square(pet_png(PetSkin::Star, PetPose::Alert), 32),
            error: decode_square(pet_png(PetSkin::Star, PetPose::Error), 32),
            sleep: decode_square(pet_png(PetSkin::Star, PetPose::Sleep), 32),
            happy: decode_square(pet_png(PetSkin::Star, PetPose::Happy), 32),
        },
        bubble_alert: decode_square(bubble_png(PetPose::Alert).expect("alert bubble"), 16),
        bubble_error: decode_square(bubble_png(PetPose::Error).expect("error bubble"), 16),
        bubble_sleep: decode_square(bubble_png(PetPose::Sleep).expect("sleep bubble"), 16),
        bubble_happy: decode_square(bubble_png(PetPose::Happy).expect("happy bubble"), 16),
        tray: TraySet {
            normal_16: decode_square(tray_png(TrayGlyph::Normal, TrayEdge::Px16), 16),
            normal_32: decode_square(tray_png(TrayGlyph::Normal, TrayEdge::Px32), 32),
            normal_64: decode_square(tray_png(TrayGlyph::Normal, TrayEdge::Px64), 64),
            alert_16: decode_square(tray_png(TrayGlyph::Alert, TrayEdge::Px16), 16),
            alert_32: decode_square(tray_png(TrayGlyph::Alert, TrayEdge::Px32), 32),
            alert_64: decode_square(tray_png(TrayGlyph::Alert, TrayEdge::Px64), 64),
            error_16: decode_square(tray_png(TrayGlyph::Error, TrayEdge::Px16), 16),
            error_32: decode_square(tray_png(TrayGlyph::Error, TrayEdge::Px32), 32),
            error_64: decode_square(tray_png(TrayGlyph::Error, TrayEdge::Px64), 64),
        },
    })
}

fn decode_square(bytes: &'static [u8], edge: u32) -> RgbaImage {
    let image = decode_png(bytes).expect("embedded png");
    if image.width != edge || image.height != edge {
        panic!(
            "embedded png is {}x{}, expected {edge}",
            image.width, image.height
        );
    }
    image
}

macro_rules! asset {
    ($path:literal) => {
        include_bytes!(concat!("../../../assets/", $path))
    };
}

fn pet_png(skin: PetSkin, pose: PetPose) -> &'static [u8] {
    match (skin, pose) {
        (PetSkin::Astronaut, PetPose::Normal) => asset!("pets/astronaut/normal.png"),
        (PetSkin::Astronaut, PetPose::Alert) => asset!("pets/astronaut/alert.png"),
        (PetSkin::Astronaut, PetPose::Error) => asset!("pets/astronaut/error.png"),
        (PetSkin::Astronaut, PetPose::Sleep) => asset!("pets/astronaut/sleep.png"),
        (PetSkin::Astronaut, PetPose::Happy) => asset!("pets/astronaut/happy.png"),
        (PetSkin::Star, PetPose::Normal) => asset!("pets/star/normal.png"),
        (PetSkin::Star, PetPose::Alert) => asset!("pets/star/alert.png"),
        (PetSkin::Star, PetPose::Error) => asset!("pets/star/error.png"),
        (PetSkin::Star, PetPose::Sleep) => asset!("pets/star/sleep.png"),
        (PetSkin::Star, PetPose::Happy) => asset!("pets/star/happy.png"),
    }
}

fn bubble_png(pose: PetPose) -> Option<&'static [u8]> {
    match pose {
        PetPose::Normal => None,
        PetPose::Alert => Some(asset!("pets/bubbles/alert.png")),
        PetPose::Error => Some(asset!("pets/bubbles/error.png")),
        PetPose::Sleep => Some(asset!("pets/bubbles/sleep.png")),
        PetPose::Happy => Some(asset!("pets/bubbles/happy.png")),
    }
}

fn tray_png(glyph: TrayGlyph, edge: TrayEdge) -> &'static [u8] {
    match (glyph, edge) {
        (TrayGlyph::Normal, TrayEdge::Px16) => asset!("tray/normal-16.png"),
        (TrayGlyph::Normal, TrayEdge::Px32) => asset!("tray/normal-32.png"),
        (TrayGlyph::Normal, TrayEdge::Px64) => asset!("tray/normal-64.png"),
        (TrayGlyph::Alert, TrayEdge::Px16) => asset!("tray/alert-16.png"),
        (TrayGlyph::Alert, TrayEdge::Px32) => asset!("tray/alert-32.png"),
        (TrayGlyph::Alert, TrayEdge::Px64) => asset!("tray/alert-64.png"),
        (TrayGlyph::Error, TrayEdge::Px16) => asset!("tray/error-16.png"),
        (TrayGlyph::Error, TrayEdge::Px32) => asset!("tray/error-32.png"),
        (TrayGlyph::Error, TrayEdge::Px64) => asset!("tray/error-64.png"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        blit, decode_png, encode_png, over, render_pet, scale_nearest, sources, RgbaImage,
    };
    use stardust_core::pet::{
        bubble_asset, pet_asset, pet_frame, tray_asset, PetPose, PetScale, PetSkin, TrayEdge,
        TrayGlyph,
    };

    #[test]
    fn integer_scale_repeats_each_source_pixel_and_over_keeps_opaque_src() {
        let src = RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![1, 2, 3, 255, 4, 5, 6, 128],
        };
        let scaled = scale_nearest(&src, 2);
        assert_eq!((scaled.width, scaled.height), (4, 2));
        assert_eq!(&scaled.pixels[0..4], &[1, 2, 3, 255]);
        assert_eq!(&scaled.pixels[4..8], &[1, 2, 3, 255]);
        assert_eq!(&scaled.pixels[8..12], &[4, 5, 6, 128]);
        assert_eq!(&scaled.pixels[16..20], &[1, 2, 3, 255]);
        assert_eq!(over([9, 9, 9, 255], [1, 2, 3, 255]), [1, 2, 3, 255]);
        assert_eq!(over([9, 9, 9, 255], [1, 2, 3, 0]), [9, 9, 9, 255]);

        let mut dst = RgbaImage {
            width: 1,
            height: 1,
            pixels: vec![0, 0, 0, 0],
        };
        blit(
            &mut dst,
            &RgbaImage {
                width: 1,
                height: 1,
                pixels: vec![7, 8, 9, 255],
            },
            0,
            0,
        );
        assert_eq!(dst.pixels, vec![7, 8, 9, 255]);
    }

    #[test]
    fn embedded_sprites_match_the_state_map_and_scale() {
        let decoded = sources();
        for skin in PetSkin::ALL {
            for pose in PetPose::ALL {
                let image = decoded.pet(skin, pose);
                assert_eq!(image.width, skin.source_px(), "{}", pet_asset(skin, pose));
                assert_eq!(image.height, skin.source_px());
                let frame = render_pet(skin, pose, PetScale::X2);
                let layout = pet_frame(skin, PetScale::X2);
                assert_eq!(frame.width, layout.canvas.width);
                assert_eq!(frame.height, layout.canvas.height);
                assert_eq!(layout.pet.width, skin.source_px() * 2);
                let outside = bubble_corner_alpha(&frame, &layout);
                match bubble_asset(pose) {
                    None => assert_eq!(outside, 0, "{pose:?} should have no bubble"),
                    Some(_) => assert!(outside > 0, "{pose:?} bubble should be visible"),
                }
            }
        }
        assert_ne!(
            decoded.pet(PetSkin::Astronaut, PetPose::Normal).pixels,
            decoded.pet(PetSkin::Astronaut, PetPose::Alert).pixels
        );
        assert_ne!(
            decoded.pet(PetSkin::Star, PetPose::Sleep).pixels,
            decoded.pet(PetSkin::Star, PetPose::Happy).pixels
        );
        for glyph in TrayGlyph::ALL {
            for edge in TrayEdge::ALL {
                let image = decoded.tray.get(glyph, edge);
                assert_eq!(image.width, edge.px(), "{}", tray_asset(glyph, edge));
                assert_eq!(image.height, edge.px());
            }
        }
        let star = render_pet(PetSkin::Star, PetPose::Normal, PetScale::X2);
        assert_eq!(pet_frame(PetSkin::Star, PetScale::X2).pet.width, 64);
        assert!(star.width >= 64 && star.height >= 64);
        let astronaut = render_pet(PetSkin::Astronaut, PetPose::Happy, PetScale::X3);
        assert_eq!(pet_frame(PetSkin::Astronaut, PetScale::X3).pet.width, 72);
        assert_eq!(
            astronaut.width,
            pet_frame(PetSkin::Astronaut, PetScale::X3).canvas.width
        );

        let raw = decode_png(super::pet_png(PetSkin::Astronaut, PetPose::Normal)).unwrap();
        assert_eq!((raw.width, raw.height), (24, 24));
        let encoded = encode_png(&raw).unwrap();
        let round = decode_png(&encoded).unwrap();
        assert_eq!(round, raw);
    }

    fn bubble_corner_alpha(image: &RgbaImage, layout: &stardust_core::pet::PetFrame) -> u32 {
        let mut total = 0u32;
        let x0 = layout.bubble.x;
        let y0 = layout.bubble.y;
        let x1 = layout.bubble.x + layout.bubble.width;
        let y1 = layout.pet.y.min(layout.bubble.y + layout.bubble.height);
        for y in y0..y1 {
            for x in x0..x1 {
                let index = ((y * image.width + x) * 4 + 3) as usize;
                total += u32::from(image.pixels[index]);
            }
        }
        total
    }

    #[test]
    fn preview_pngs_when_a_directory_is_set() {
        let Ok(dir) = std::env::var("STARDUST_PREVIEW_DIR") else {
            return;
        };
        let root = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&root).unwrap();
        for skin in PetSkin::ALL {
            for pose in PetPose::ALL {
                for scale in PetScale::ALL {
                    let frame = render_pet(skin, pose, scale);
                    let shown = on_navy(&frame);
                    let name = format!(
                        "{}-{}-{}x.png",
                        match skin {
                            PetSkin::Astronaut => "astronaut",
                            PetSkin::Star => "star",
                        },
                        match pose {
                            PetPose::Normal => "normal",
                            PetPose::Alert => "alert",
                            PetPose::Error => "error",
                            PetPose::Sleep => "sleep",
                            PetPose::Happy => "happy",
                        },
                        scale.factor()
                    );
                    let bytes = encode_png(&shown).unwrap();
                    std::fs::write(root.join(name), bytes).unwrap();
                }
            }
        }
    }

    fn on_navy(image: &RgbaImage) -> RgbaImage {
        let mut out = image.clone();
        for px in out.pixels.as_chunks_mut::<4>().0 {
            let mixed = over([0x1C, 0x24, 0x30, 255], [px[0], px[1], px[2], px[3]]);
            px.copy_from_slice(&mixed);
        }
        out
    }
}
