/// 32×32 tray glyph drawn in code. No image file is bundled.
pub fn tray_rgba() -> (Vec<u8>, u32, u32) {
    const SIZE: i32 = 32;
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    let center = 15.5f32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            if dx.abs() / 13.0 + dy.abs() / 13.0 <= 1.0 {
                let index = ((y * SIZE + x) * 4) as usize;
                rgba[index] = 0xF2;
                rgba[index + 1] = 0xC1;
                rgba[index + 2] = 0x4E;
                rgba[index + 3] = 0xFF;
            }
        }
    }
    (rgba, SIZE as u32, SIZE as u32)
}
