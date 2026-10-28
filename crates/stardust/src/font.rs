//! Letters for the windows the app draws itself.
//!
//! The built-in letters are an original 5x7 bitmap that covers ASCII
//! letters, digits, and a few marks. With the `system-font` feature, a
//! letter outside that set (Hangul, for example) is drawn from a font that
//! is already installed on the computer. Without it, such a letter is a
//! hollow box, and the settings window stays in English (see
//! `stardust_core::i18n::drawn_language`).

pub const SCALE: u32 = 2;
pub const CELL_W: u32 = 6;
pub const CELL_H: u32 = 8;
pub const PAD: u32 = 12;
/// Widest bubble line, in bitmap cells.
pub const MAX_COLUMNS: u32 = 42;

pub fn text_size(lines: &[String]) -> (u32, u32) {
    let rows = lines.len().clamp(1, 6) as u32;
    let cols = lines
        .iter()
        .map(|line| text_columns(line))
        .max()
        .unwrap_or(8)
        .clamp(8, MAX_COLUMNS);
    (
        PAD * 2 + cols * CELL_W * SCALE,
        PAD * 2 + rows * CELL_H * SCALE,
    )
}

/// Bitmap-only drawing, used by tests to compare with [`Painter::draw`].
#[cfg(test)]
pub fn draw_text(
    buffer: &mut [u32],
    width: u32,
    text: &str,
    color: u32,
    origin_x: i32,
    origin_y: i32,
    scale: u32,
) {
    if width == 0 || scale == 0 {
        return;
    }
    for (column, ch) in text.chars().take(MAX_COLUMNS as usize).enumerate() {
        let pen = Pen {
            x: origin_x + column as i32 * (CELL_W * scale) as i32,
            y: origin_y,
            color,
            scale,
        };
        draw_bitmap_char(buffer, width, ch, pen);
    }
}

fn fill_cell(buffer: &mut [u32], width: u32, height: u32, x: i32, y: i32, scale: u32, color: u32) {
    for dy in 0..scale as i32 {
        for dx in 0..scale as i32 {
            let px = x + dx;
            let py = y + dy;
            if px < 0 || py < 0 || px as u32 >= width || py as u32 >= height {
                continue;
            }
            let index = py as u32 * width + px as u32;
            buffer[index as usize] = color;
        }
    }
}

/// True when the bitmap has a real glyph for `ch`.
pub fn bitmap_has(ch: char) -> bool {
    ch == ' ' || glyph(ch) != glyph('\u{FFFF}')
}

/// Width in bitmap cells: two for East Asian wide letters, one otherwise.
pub fn cell_columns(ch: char) -> u32 {
    let code = u32::from(ch);
    let wide = matches!(code,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6);
    if wide {
        2
    } else {
        1
    }
}

pub fn text_columns(text: &str) -> u32 {
    text.chars().map(cell_columns).sum()
}

/// Where and how to draw: top-left corner, colour, and bitmap scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pen {
    pub x: i32,
    pub y: i32,
    pub color: u32,
    pub scale: u32,
}

/// Draws text with the bitmap and, when available, a system font fallback.
pub struct Painter {
    #[cfg(feature = "system-font")]
    fallback: std::cell::OnceCell<Option<ab_glyph::FontVec>>,
}

impl Painter {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "system-font")]
            fallback: std::cell::OnceCell::new(),
        }
    }

    /// True when Hangul can be drawn. Loads the system font on first use.
    pub fn has_hangul(&self) -> bool {
        #[cfg(feature = "system-font")]
        {
            self.fallback_font()
                .is_some_and(|font| system::covers(font, '\u{AC00}'))
        }
        #[cfg(not(feature = "system-font"))]
        {
            false
        }
    }

    #[cfg(feature = "system-font")]
    fn fallback_font(&self) -> Option<&ab_glyph::FontVec> {
        self.fallback
            .get_or_init(|| system::load(std::env::var("STARDUST_FONT").ok().as_deref()))
            .as_ref()
    }

    /// Draw `text` with its top-left at the pen. Each bitmap cell is
    /// `CELL_W * scale` wide; wide letters take two cells.
    pub fn draw(&self, buffer: &mut [u32], width: u32, text: &str, pen: Pen, max_columns: u32) {
        if width == 0 || pen.scale == 0 {
            return;
        }
        let mut column = 0u32;
        for ch in text.chars() {
            let span = cell_columns(ch);
            if column + span > max_columns {
                break;
            }
            let at = Pen {
                x: pen.x + (column * CELL_W * pen.scale) as i32,
                ..pen
            };
            if bitmap_has(ch) || !self.draw_fallback(buffer, width, ch, at) {
                draw_bitmap_char(buffer, width, ch, at);
            }
            column += span;
        }
    }

    #[cfg(feature = "system-font")]
    fn draw_fallback(&self, buffer: &mut [u32], width: u32, ch: char, pen: Pen) -> bool {
        let Some(font) = self.fallback_font() else {
            return false;
        };
        system::draw_char(font, buffer, width, ch, pen, CELL_H * pen.scale)
    }

    #[cfg(not(feature = "system-font"))]
    fn draw_fallback(&self, _buffer: &mut [u32], _width: u32, _ch: char, _pen: Pen) -> bool {
        false
    }
}

#[cfg(feature = "system-font")]
mod system {
    use super::Pen;
    use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};

    /// Fonts that ship with each OS and cover Hangul. The first readable one
    /// wins. `STARDUST_FONT` overrides the list.
    pub const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\malgun.ttf",
        "/System/Library/Fonts/AppleSDGothicNeo.ttc",
        "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
    ];

    pub fn load(override_path: Option<&str>) -> Option<FontVec> {
        let paths = override_path
            .filter(|path| !path.is_empty())
            .into_iter()
            .chain(CANDIDATES.iter().copied());
        for path in paths {
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            if let Ok(font) = FontVec::try_from_vec_and_index(bytes, 0) {
                if covers(&font, '\u{AC00}') {
                    return Some(font);
                }
            }
        }
        None
    }

    pub fn covers(font: &FontVec, ch: char) -> bool {
        font.glyph_id(ch).0 != 0
    }

    fn blend(under: u32, over: u32, coverage: f32) -> u32 {
        let mix = |shift: u32| {
            let a = f32::from(((under >> shift) & 0xFF) as u8);
            let b = f32::from(((over >> shift) & 0xFF) as u8);
            ((a + (b - a) * coverage).round().clamp(0.0, 255.0) as u32) << shift
        };
        mix(16) | mix(8) | mix(0)
    }

    pub fn draw_char(
        font: &FontVec,
        buffer: &mut [u32],
        width: u32,
        ch: char,
        pen: Pen,
        cell_height: u32,
    ) -> bool {
        let (left, top, color) = (pen.x, pen.y, pen.color);
        if !covers(font, ch) {
            return false;
        }
        let height = buffer.len() as u32 / width.max(1);
        let px = PxScale::from(cell_height as f32);
        let scaled = font.as_scaled(px);
        let glyph = font
            .glyph_id(ch)
            .with_scale_and_position(px, point(left as f32, top as f32 + scaled.ascent()));
        let Some(outline) = font.outline_glyph(glyph) else {
            return true;
        };
        let bounds = outline.px_bounds();
        outline.draw(|gx, gy, coverage| {
            let x = bounds.min.x as i32 + gx as i32;
            let y = bounds.min.y as i32 + gy as i32;
            if coverage <= 0.0 || x < 0 || y < 0 || x as u32 >= width || y as u32 >= height {
                return;
            }
            let index = (y as u32 * width + x as u32) as usize;
            buffer[index] = blend(buffer[index], color, coverage.min(1.0));
        });
        true
    }
}

fn draw_bitmap_char(buffer: &mut [u32], width: u32, ch: char, pen: Pen) {
    let Pen {
        x: left,
        y: origin_y,
        color,
        scale,
    } = pen;
    let height = buffer.len() as u32 / width;
    for (row, bits) in glyph(ch).iter().enumerate() {
        for bit in 0..5 {
            if bits & (1 << (4 - bit)) == 0 {
                continue;
            }
            fill_cell(
                buffer,
                width,
                height,
                left + bit * scale as i32,
                origin_y + row as i32 * scale as i32,
                scale,
                color,
            );
        }
    }
}

fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        '.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        ':' => [0, 0b01100, 0b01100, 0, 0b01100, 0b01100, 0],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        ',' => [0, 0, 0, 0, 0b01100, 0b01100, 0b01000],
        _ => [
            0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::{bitmap_has, cell_columns, draw_text, text_columns, Painter, Pen, CELL_W};

    #[test]
    fn letter_a_has_ink_and_space_does_not() {
        let mut ink = vec![0u32; 32 * 16];
        draw_text(&mut ink, 32, "A", 0x00FF_FFFF, 0, 0, 1);
        assert!(ink.iter().any(|pixel| *pixel != 0));
        let mut blank = vec![0u32; 32 * 16];
        draw_text(&mut blank, 32, " ", 0x00FF_FFFF, 0, 0, 1);
        assert!(blank.iter().all(|pixel| *pixel == 0));
    }

    #[test]
    fn hangul_is_wide_and_not_in_the_bitmap() {
        assert!(bitmap_has('A'));
        assert!(bitmap_has(' '));
        assert!(!bitmap_has('가'));
        assert_eq!(cell_columns('가'), 2);
        assert_eq!(cell_columns('A'), 1);
        assert_eq!(text_columns("ORBIT 로그인"), 6 + 6);
    }

    #[test]
    fn painter_draws_ascii_and_stops_at_the_column_limit() {
        let painter = Painter::new();
        let width = 200;
        let mut buffer = vec![0u32; (width * 20) as usize];
        let pen = Pen {
            x: 0,
            y: 0,
            color: 0x00FF_FFFF,
            scale: 1,
        };
        painter.draw(&mut buffer, width, "AB", pen, 1);
        let right_of_first: usize = (0..20)
            .flat_map(|y| (CELL_W..width).map(move |x| (y * width + x) as usize))
            .filter(|index| buffer[*index] != 0)
            .count();
        assert_eq!(right_of_first, 0);
        assert!(buffer.iter().any(|pixel| *pixel != 0));
    }

    #[cfg(feature = "system-font")]
    #[test]
    fn system_font_draws_hangul_when_one_is_installed() {
        let painter = Painter::new();
        let found = super::system::CANDIDATES
            .iter()
            .any(|path| std::path::Path::new(path).is_file())
            || std::env::var("STARDUST_FONT")
                .is_ok_and(|path| std::path::Path::new(&path).is_file());
        assert_eq!(painter.has_hangul(), found);
        let width = 64;
        let mut buffer = vec![0u32; (width * 32) as usize];
        let pen = Pen {
            x: 0,
            y: 0,
            color: 0x00FF_FFFF,
            scale: 2,
        };
        painter.draw(&mut buffer, width, "가", pen, 4);
        let inked = buffer.iter().filter(|pixel| **pixel != 0).count();
        if found {
            assert!(inked > 20, "inked {inked}");
            let mut boxed = vec![0u32; (width * 32) as usize];
            draw_text(&mut boxed, width, "가", 0x00FF_FFFF, 0, 0, 2);
            assert_ne!(buffer, boxed);
        } else {
            assert!(inked > 0);
        }
    }
}
