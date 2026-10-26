//! Colours and readability checks for windows the app draws itself.
//!
//! Contrast follows the WCAG 2.2 relative-luminance formula. Text pairs
//! must reach 4.5:1 (level AA for normal text). The focus fill must reach
//! 3:1 against the window background (non-text contrast).

/// Cosmic Latte, the settings window background.
pub const SETTINGS_BACKGROUND: u32 = 0x00FF_F8E7;
pub const SETTINGS_TEXT: u32 = 0x001C_2430;
pub const SETTINGS_FOCUS_FILL: u32 = 0x001C_2430;
pub const SETTINGS_FOCUS_TEXT: u32 = 0x00FF_F8E7;
pub const SETTINGS_MUTED_TEXT: u32 = 0x004A_5260;
pub const BUBBLE_BACKGROUND: u32 = 0x001C_2430;
pub const BUBBLE_TEXT: u32 = 0x00F4_F1EA;

pub const TEXT_MIN_CONTRAST: f64 = 4.5;
pub const NON_TEXT_MIN_CONTRAST: f64 = 3.0;

fn channel(value: u32) -> f64 {
    let srgb = f64::from(value & 0xFF) / 255.0;
    if srgb <= 0.04045 {
        srgb / 12.92
    } else {
        ((srgb + 0.055) / 1.055).powf(2.4)
    }
}

/// Relative luminance of a `0x00RRGGBB` colour.
pub fn luminance(rgb: u32) -> f64 {
    0.2126 * channel(rgb >> 16) + 0.7152 * channel(rgb >> 8) + 0.0722 * channel(rgb)
}

pub fn contrast_ratio(first: u32, second: u32) -> f64 {
    let (a, b) = (luminance(first), luminance(second));
    let (light, dark) = if a >= b { (a, b) } else { (b, a) };
    (light + 0.05) / (dark + 0.05)
}

/// Integer text scale for a monitor scale factor. The bitmap letters are
/// 7 pixels tall, so scale 2 is the smallest that stays legible at 1x.
pub fn text_scale(scale_factor: f64) -> u32 {
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return 2;
    }
    let scaled = (scale_factor * 2.0).round();
    if scaled < 2.0 {
        2
    } else if scaled > 8.0 {
        8
    } else {
        scaled as u32
    }
}

#[cfg(test)]
mod tests {
    use super::{
        contrast_ratio, text_scale, BUBBLE_BACKGROUND, BUBBLE_TEXT, NON_TEXT_MIN_CONTRAST,
        SETTINGS_BACKGROUND, SETTINGS_FOCUS_FILL, SETTINGS_FOCUS_TEXT, SETTINGS_MUTED_TEXT,
        SETTINGS_TEXT, TEXT_MIN_CONTRAST,
    };

    #[test]
    fn contrast_matches_known_values() {
        assert!((contrast_ratio(0x00FF_FFFF, 0x0000_0000) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(0x0077_7777, 0x00FF_FFFF) - 4.48).abs() < 0.01);
        assert!((contrast_ratio(0x0012_3456, 0x0012_3456) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn drawn_text_pairs_meet_wcag_aa() {
        for (fg, bg) in [
            (SETTINGS_TEXT, SETTINGS_BACKGROUND),
            (SETTINGS_MUTED_TEXT, SETTINGS_BACKGROUND),
            (SETTINGS_FOCUS_TEXT, SETTINGS_FOCUS_FILL),
            (BUBBLE_TEXT, BUBBLE_BACKGROUND),
        ] {
            let ratio = contrast_ratio(fg, bg);
            assert!(
                ratio >= TEXT_MIN_CONTRAST,
                "{fg:06x} on {bg:06x}: {ratio:.2}"
            );
        }
        assert!(contrast_ratio(SETTINGS_FOCUS_FILL, SETTINGS_BACKGROUND) >= NON_TEXT_MIN_CONTRAST);
        assert_eq!(SETTINGS_BACKGROUND, 0x00FF_F8E7);
    }

    #[test]
    fn text_scale_follows_the_monitor() {
        assert_eq!(text_scale(1.0), 2);
        assert_eq!(text_scale(1.5), 3);
        assert_eq!(text_scale(2.0), 4);
        assert_eq!(text_scale(0.5), 2);
        assert_eq!(text_scale(f64::NAN), 2);
        assert_eq!(text_scale(10.0), 8);
    }
}
