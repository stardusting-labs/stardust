//! Letters for the windows the app draws itself.
//!
//! The built-in letters are an original 5x7 bitmap that covers ASCII
//! letters, digits, and a few marks. Stardust text is English only; any
//! other letter is drawn as a hollow box.

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

#[cfg(test)]
fn bitmap_has(ch: char) -> bool {
    ch == ' ' || glyph(ch) != glyph('\u{FFFF}')
}

pub fn text_columns(text: &str) -> u32 {
    text.chars().count() as u32
}

/// Where and how to draw: top-left corner, colour, and bitmap scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pen {
    pub x: i32,
    pub y: i32,
    pub color: u32,
    pub scale: u32,
}

/// Draws text with the built-in bitmap.
pub struct Painter;

impl Painter {
    pub fn new() -> Self {
        Self
    }

    /// Draw `text` with its top-left at the pen. Each letter is
    /// `CELL_W * scale` wide.
    pub fn draw(&self, buffer: &mut [u32], width: u32, text: &str, pen: Pen, max_columns: u32) {
        if width == 0 || pen.scale == 0 {
            return;
        }
        for (column, ch) in text.chars().take(max_columns as usize).enumerate() {
            let at = Pen {
                x: pen.x + (column as u32 * CELL_W * pen.scale) as i32,
                ..pen
            };
            draw_bitmap_char(buffer, width, ch, at);
        }
    }

    /// [`Painter::draw`] with lowercase letters and URL marks as well, so an
    /// address can be read and typed. Only the sign-in window uses this; the
    /// other windows keep the letters they have always drawn.
    pub fn draw_extended(
        &self,
        buffer: &mut [u32],
        width: u32,
        text: &str,
        pen: Pen,
        max_columns: u32,
    ) {
        if width == 0 || pen.scale == 0 {
            return;
        }
        for (column, ch) in text.chars().take(max_columns as usize).enumerate() {
            let at = Pen {
                x: pen.x + (column as u32 * CELL_W * pen.scale) as i32,
                ..pen
            };
            draw_bits(buffer, width, &extended_glyph(ch), at);
        }
    }
}

fn draw_bitmap_char(buffer: &mut [u32], width: u32, ch: char, pen: Pen) {
    draw_bits(buffer, width, &glyph(ch), pen);
}

fn draw_bits(buffer: &mut [u32], width: u32, bits: &[u8], pen: Pen) {
    let Pen {
        x: left,
        y: origin_y,
        color,
        scale,
    } = pen;
    let height = buffer.len() as u32 / width;
    for (row, bits) in bits.iter().enumerate() {
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

/// Letters for [`Painter::draw_extended`]: the shared table, plus lowercase
/// and the marks an address can hold. This is a second table so the shared one
/// stays as it is, because adding `>` there would change how the settings
/// window looks with the sign-in window feature off.
///
/// Eight rows, so `g`, `j`, `p`, `q`, and `y` hang below the line. Otherwise
/// `g` reads as `9`, which matters in an address someone may type by hand.
fn extended_glyph(ch: char) -> [u8; 8] {
    match ch {
        'g' => [
            0b00000, 0b00000, 0b01111, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110,
        ],
        'j' => [
            0b00010, 0b00000, 0b00110, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100,
        ],
        'p' => [
            0b00000, 0b00000, 0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000,
        ],
        'q' => [
            0b00000, 0b00000, 0b01111, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001,
        ],
        'y' => [
            0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110,
        ],
        other => {
            let [a, b, c, d, e, f, g] = upright_glyph(other);
            [a, b, c, d, e, f, g, 0]
        }
    }
}

/// The letters that sit on the line.
fn upright_glyph(ch: char) -> [u8; 7] {
    match ch {
        'a' => [
            0b00000, 0b00000, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111,
        ],
        'b' => [
            0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'c' => [
            0b00000, 0b00000, 0b01110, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'd' => [
            0b00001, 0b00001, 0b01111, 0b10001, 0b10001, 0b10001, 0b01111,
        ],
        'e' => [
            0b00000, 0b00000, 0b01110, 0b10001, 0b11111, 0b10000, 0b01110,
        ],
        'f' => [
            0b00110, 0b01001, 0b01000, 0b11100, 0b01000, 0b01000, 0b01000,
        ],
        'h' => [
            0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001,
        ],
        'i' => [
            0b00100, 0b00000, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'k' => [
            0b10000, 0b10000, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010,
        ],
        'l' => [
            0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'm' => [
            0b00000, 0b00000, 0b11010, 0b10101, 0b10101, 0b10001, 0b10001,
        ],
        'n' => [
            0b00000, 0b00000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001,
        ],
        'o' => [
            0b00000, 0b00000, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'r' => [
            0b00000, 0b00000, 0b10110, 0b11001, 0b10000, 0b10000, 0b10000,
        ],
        's' => [
            0b00000, 0b00000, 0b01110, 0b10000, 0b01110, 0b00001, 0b11110,
        ],
        't' => [
            0b01000, 0b01000, 0b11100, 0b01000, 0b01000, 0b01001, 0b00110,
        ],
        'u' => [
            0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b10011, 0b01101,
        ],
        'v' => [
            0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'w' => [
            0b00000, 0b00000, 0b10001, 0b10001, 0b10101, 0b10101, 0b01010,
        ],
        'x' => [
            0b00000, 0b00000, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001,
        ],
        'z' => [
            0b00000, 0b00000, 0b11111, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        '\\' => [
            0b10000, 0b01000, 0b01000, 0b00100, 0b00010, 0b00010, 0b00001,
        ],
        '?' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100,
        ],
        '=' => [0, 0, 0b11111, 0, 0b11111, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        '>' => [
            0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000,
        ],
        '<' => [
            0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010,
        ],
        '(' => [
            0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010,
        ],
        ')' => [
            0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000,
        ],
        '[' => [
            0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110,
        ],
        ']' => [
            0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110,
        ],
        '{' => [
            0b00010, 0b00100, 0b00100, 0b01000, 0b00100, 0b00100, 0b00010,
        ],
        '}' => [
            0b01000, 0b00100, 0b00100, 0b00010, 0b00100, 0b00100, 0b01000,
        ],
        '%' => [
            0b11000, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b00011,
        ],
        '&' => [
            0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101,
        ],
        '~' => [0, 0, 0b01000, 0b10101, 0b00010, 0, 0],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        '#' => [
            0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010,
        ],
        ';' => [0, 0b01100, 0b01100, 0, 0b01100, 0b00100, 0b01000],
        '\'' => [0b01100, 0b00100, 0b01000, 0, 0, 0, 0],
        '"' => [0b01010, 0b01010, 0b01010, 0, 0, 0, 0],
        '`' => [0b01000, 0b00100, 0b00010, 0, 0, 0, 0],
        '!' => [
            0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100,
        ],
        '*' => [0, 0b00100, 0b10101, 0b01110, 0b10101, 0b00100, 0],
        '$' => [
            0b00100, 0b01111, 0b10100, 0b01110, 0b00101, 0b11110, 0b00100,
        ],
        '@' => [
            0b01110, 0b10001, 0b00001, 0b01101, 0b10101, 0b10101, 0b01110,
        ],
        '^' => [0b00100, 0b01010, 0b10001, 0, 0, 0, 0],
        '|' => [
            0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        other => glyph(other),
    }
}

#[cfg(test)]
mod tests {
    use super::{bitmap_has, draw_text, extended_glyph, glyph, text_columns, Painter, Pen, CELL_W};

    const BOX: char = '\u{FFFF}';

    /// A shared-table letter as the eight rows the sign-in window draws.
    fn eight_rows(ch: char) -> [u8; 8] {
        let [a, b, c, d, e, f, g] = glyph(ch);
        [a, b, c, d, e, f, g, 0]
    }

    #[test]
    fn the_sign_in_letters_cover_printable_ascii_and_keep_case_apart() {
        for ch in '!'..='~' {
            assert_ne!(extended_glyph(ch), eight_rows(BOX), "{ch}");
        }
        for (lower, upper) in ('a'..='z').zip('A'..='Z') {
            assert_ne!(extended_glyph(lower), extended_glyph(upper), "{lower}");
        }
        for ch in ['A', 'Z', '0', '9', '.', ':', '-', ',', ' '] {
            assert_eq!(extended_glyph(ch), eight_rows(ch), "{ch}");
        }
        assert_eq!(
            extended_glyph(BOX),
            eight_rows(BOX),
            "anything else is a box"
        );
    }

    #[test]
    fn letters_that_hang_below_the_line_are_not_mistaken_for_digits() {
        assert_ne!(extended_glyph('g'), extended_glyph('9'));
        for ch in ['g', 'j', 'p', 'q', 'y'] {
            assert_ne!(extended_glyph(ch)[7], 0, "{ch} uses the eighth row");
        }
        for ch in ['a', 'A', '9', '/', 'x'] {
            assert_eq!(extended_glyph(ch)[7], 0, "{ch} sits on the line");
        }
    }

    #[test]
    fn the_shared_table_is_unchanged_so_other_windows_look_the_same() {
        for ch in ['/', '?', '=', '_', '>', '<', '(', ')', '%', '&', '~', '+'] {
            assert_eq!(
                glyph(ch),
                glyph(BOX),
                "{ch} stays a box in the shared table"
            );
        }
        assert_eq!(glyph('a'), glyph('A'), "the shared table has no lowercase");
    }

    #[test]
    fn the_extended_draw_stops_at_the_column_limit() {
        let painter = Painter::new();
        let width = 200;
        let mut buffer = vec![0u32; (width * 20) as usize];
        let pen = Pen {
            x: 0,
            y: 0,
            color: 0x00FF_FFFF,
            scale: 1,
        };
        painter.draw_extended(&mut buffer, width, "ab", pen, 1);
        let right_of_first = (0..20)
            .flat_map(|y| (CELL_W..width).map(move |x| (y * width + x) as usize))
            .filter(|index| buffer[*index] != 0)
            .count();
        assert_eq!(right_of_first, 0);
        assert!(buffer.iter().any(|pixel| *pixel != 0));
        let mut none = vec![0u32; 16];
        painter.draw_extended(&mut none, 0, "a", pen, 1);
        assert!(none.iter().all(|pixel| *pixel == 0));
    }

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
    fn ascii_is_in_the_bitmap_and_one_column_wide() {
        assert!(bitmap_has('A'));
        assert!(bitmap_has(' '));
        assert!(!bitmap_has('\u{FFFF}'));
        assert_eq!(text_columns("ORBIT sign-in"), 13);
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
}
