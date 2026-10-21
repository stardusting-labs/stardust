pub const SCALE: u32 = 2;
pub const CELL_W: u32 = 6;
pub const CELL_H: u32 = 8;
pub const PAD: u32 = 12;

pub fn text_size(lines: &[String]) -> (u32, u32) {
    let rows = lines.len().clamp(1, 6) as u32;
    let cols = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(8)
        .clamp(8, 42) as u32;
    (
        PAD * 2 + cols * CELL_W * SCALE,
        PAD * 2 + rows * CELL_H * SCALE,
    )
}

pub fn draw_lines(buffer: &mut [u32], width: u32, lines: &[String], color: u32) {
    let shown = lines.iter().take(6);
    for (row, line) in shown.enumerate() {
        let y = PAD as i32 + row as i32 * (CELL_H * SCALE) as i32;
        draw_text(buffer, width, line, color, PAD as i32, y, SCALE);
    }
}

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
    let height = buffer.len() as u32 / width;
    for (column, ch) in text.chars().take(42).enumerate() {
        let rows = glyph(ch);
        let left = origin_x + column as i32 * (CELL_W * scale) as i32;
        for (row, bits) in rows.iter().enumerate() {
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
    use super::draw_text;

    #[test]
    fn letter_a_has_ink_and_space_does_not() {
        let mut ink = vec![0u32; 32 * 16];
        draw_text(&mut ink, 32, "A", 0x00FF_FFFF, 0, 0, 1);
        assert!(ink.iter().any(|pixel| *pixel != 0));
        let mut blank = vec![0u32; 32 * 16];
        draw_text(&mut blank, 32, " ", 0x00FF_FFFF, 0, 0, 1);
        assert!(blank.iter().all(|pixel| *pixel == 0));
    }
}
