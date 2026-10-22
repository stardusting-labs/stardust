//! Tray pet skin, scale, and the face shown for a check.
//!
//! The astronaut sprite is 24×24 and the star sprite is 32×32. A scale of 2
//! draws them at 48px and 64px. A scale of 3 draws them at 72px and 96px.
//! The status mark is a 16×16 bubble scaled by the same factor and placed on
//! the pet's top-right corner. The normal face has no mark.

use crate::position::WindowSize;

/// Source edge of every status bubble, in pixels.
pub const BUBBLE_SOURCE_PX: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PetSkin {
    #[default]
    Astronaut,
    Star,
}

impl PetSkin {
    pub const ALL: [Self; 2] = [Self::Astronaut, Self::Star];

    pub fn label(self) -> &'static str {
        match self {
            Self::Astronaut => "Astronaut",
            Self::Star => "Star",
        }
    }

    pub fn menu_id(self) -> &'static str {
        match self {
            Self::Astronaut => "skin-astronaut",
            Self::Star => "skin-star",
        }
    }

    pub fn source_px(self) -> u32 {
        match self {
            Self::Astronaut => 24,
            Self::Star => 32,
        }
    }
}

pub fn skin_from_menu(id: &str) -> Option<PetSkin> {
    PetSkin::ALL.into_iter().find(|skin| skin.menu_id() == id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PetScale {
    #[default]
    X2,
    X3,
}

impl PetScale {
    pub const ALL: [Self; 2] = [Self::X2, Self::X3];

    pub fn factor(self) -> u32 {
        match self {
            Self::X2 => 2,
            Self::X3 => 3,
        }
    }

    pub fn from_factor(value: u8) -> Option<Self> {
        match value {
            2 => Some(Self::X2),
            3 => Some(Self::X3),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::X2 => "Size 2x",
            Self::X3 => "Size 3x",
        }
    }

    pub fn menu_id(self) -> &'static str {
        match self {
            Self::X2 => "scale-2",
            Self::X3 => "scale-3",
        }
    }
}

pub fn scale_from_menu(id: &str) -> Option<PetScale> {
    PetScale::ALL
        .into_iter()
        .find(|scale| scale.menu_id() == id)
}

/// Face drawn for the latest check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetPose {
    Normal,
    Alert,
    Error,
    Sleep,
    Happy,
}

impl PetPose {
    pub const ALL: [Self; 5] = [
        Self::Normal,
        Self::Alert,
        Self::Error,
        Self::Sleep,
        Self::Happy,
    ];
}

/// What the process has seen from checks so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ObservedCheck {
    #[default]
    Waiting,
    Failed,
    Succeeded {
        item_count: u32,
        high_severity: bool,
    },
}

/// Tray glyphs shipped as PNGs. Sleep and happy keep the normal glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayGlyph {
    Normal,
    Alert,
    Error,
}

impl TrayGlyph {
    pub const ALL: [Self; 3] = [Self::Normal, Self::Alert, Self::Error];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayEdge {
    Px16,
    Px32,
    Px64,
}

impl TrayEdge {
    pub const ALL: [Self; 3] = [Self::Px16, Self::Px32, Self::Px64];

    pub fn px(self) -> u32 {
        match self {
            Self::Px16 => 16,
            Self::Px32 => 32,
            Self::Px64 => 64,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlacedSprite {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PetFrame {
    pub canvas: WindowSize,
    pub pet: PlacedSprite,
    pub bubble: PlacedSprite,
}

pub fn pet_pose(observed: ObservedCheck, quiet: bool) -> PetPose {
    match observed {
        ObservedCheck::Failed => PetPose::Error,
        ObservedCheck::Succeeded {
            high_severity: true,
            ..
        } => PetPose::Alert,
        ObservedCheck::Waiting => {
            if quiet {
                PetPose::Sleep
            } else {
                PetPose::Normal
            }
        }
        ObservedCheck::Succeeded {
            item_count,
            high_severity: false,
        } => {
            if quiet {
                PetPose::Sleep
            } else if item_count == 0 {
                PetPose::Happy
            } else {
                PetPose::Alert
            }
        }
    }
}

pub fn tray_glyph(pose: PetPose) -> TrayGlyph {
    match pose {
        PetPose::Normal | PetPose::Sleep | PetPose::Happy => TrayGlyph::Normal,
        PetPose::Alert => TrayGlyph::Alert,
        PetPose::Error => TrayGlyph::Error,
    }
}

pub fn pet_asset(skin: PetSkin, pose: PetPose) -> &'static str {
    match (skin, pose) {
        (PetSkin::Astronaut, PetPose::Normal) => "pets/astronaut/normal.png",
        (PetSkin::Astronaut, PetPose::Alert) => "pets/astronaut/alert.png",
        (PetSkin::Astronaut, PetPose::Error) => "pets/astronaut/error.png",
        (PetSkin::Astronaut, PetPose::Sleep) => "pets/astronaut/sleep.png",
        (PetSkin::Astronaut, PetPose::Happy) => "pets/astronaut/happy.png",
        (PetSkin::Star, PetPose::Normal) => "pets/star/normal.png",
        (PetSkin::Star, PetPose::Alert) => "pets/star/alert.png",
        (PetSkin::Star, PetPose::Error) => "pets/star/error.png",
        (PetSkin::Star, PetPose::Sleep) => "pets/star/sleep.png",
        (PetSkin::Star, PetPose::Happy) => "pets/star/happy.png",
    }
}

pub fn bubble_asset(pose: PetPose) -> Option<&'static str> {
    match pose {
        PetPose::Normal => None,
        PetPose::Alert => Some("pets/bubbles/alert.png"),
        PetPose::Error => Some("pets/bubbles/error.png"),
        PetPose::Sleep => Some("pets/bubbles/sleep.png"),
        PetPose::Happy => Some("pets/bubbles/happy.png"),
    }
}

pub fn tray_asset(glyph: TrayGlyph, edge: TrayEdge) -> &'static str {
    match (glyph, edge) {
        (TrayGlyph::Normal, TrayEdge::Px16) => "tray/normal-16.png",
        (TrayGlyph::Normal, TrayEdge::Px32) => "tray/normal-32.png",
        (TrayGlyph::Normal, TrayEdge::Px64) => "tray/normal-64.png",
        (TrayGlyph::Alert, TrayEdge::Px16) => "tray/alert-16.png",
        (TrayGlyph::Alert, TrayEdge::Px32) => "tray/alert-32.png",
        (TrayGlyph::Alert, TrayEdge::Px64) => "tray/alert-64.png",
        (TrayGlyph::Error, TrayEdge::Px16) => "tray/error-16.png",
        (TrayGlyph::Error, TrayEdge::Px32) => "tray/error-32.png",
        (TrayGlyph::Error, TrayEdge::Px64) => "tray/error-64.png",
    }
}

/// Pick the tray PNG edge for a monitor scale factor.
///
/// Below 1.5 uses 16px, below 3 uses 32px, and 3 or above uses 64px.
pub fn tray_edge_px(scale_factor: f64) -> u32 {
    if !scale_factor.is_finite() || scale_factor < 1.5 {
        TrayEdge::Px16.px()
    } else if scale_factor < 3.0 {
        TrayEdge::Px32.px()
    } else {
        TrayEdge::Px64.px()
    }
}

pub fn tray_edge_for_px(px: u32) -> TrayEdge {
    match px {
        0..=23 => TrayEdge::Px16,
        24..=47 => TrayEdge::Px32,
        _ => TrayEdge::Px64,
    }
}

pub fn scaled_pet_px(skin: PetSkin, scale: PetScale) -> u32 {
    skin.source_px() * scale.factor()
}

/// Window layout for one skin and scale.
///
/// The bubble slot is always reserved, so showing or hiding the mark does not
/// move the pet. Half of the bubble overlaps the pet's top-right corner.
pub fn pet_frame(skin: PetSkin, scale: PetScale) -> PetFrame {
    let pet_edge = scaled_pet_px(skin, scale);
    let bubble_edge = BUBBLE_SOURCE_PX * scale.factor();
    let overlap = bubble_edge / 2;
    let bubble = PlacedSprite {
        x: pet_edge - overlap,
        y: 0,
        width: bubble_edge,
        height: bubble_edge,
    };
    let pet = PlacedSprite {
        x: 0,
        y: bubble_edge - overlap,
        width: pet_edge,
        height: pet_edge,
    };
    PetFrame {
        canvas: WindowSize {
            width: bubble.x + bubble.width,
            height: pet.y + pet.height,
        },
        pet,
        bubble,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bubble_asset, pet_asset, pet_frame, pet_pose, scale_from_menu, scaled_pet_px,
        skin_from_menu, tray_asset, tray_edge_for_px, tray_edge_px, tray_glyph, ObservedCheck,
        PetPose, PetScale, PetSkin, TrayEdge, TrayGlyph,
    };
    use crate::copy::find_disallowed_copy;

    #[test]
    fn check_state_selects_the_matching_face_and_bubble() {
        let cases = [
            (ObservedCheck::Waiting, false, PetPose::Normal, None),
            (
                ObservedCheck::Waiting,
                true,
                PetPose::Sleep,
                Some("pets/bubbles/sleep.png"),
            ),
            (
                ObservedCheck::Failed,
                false,
                PetPose::Error,
                Some("pets/bubbles/error.png"),
            ),
            (
                ObservedCheck::Failed,
                true,
                PetPose::Error,
                Some("pets/bubbles/error.png"),
            ),
            (
                ObservedCheck::Succeeded {
                    item_count: 0,
                    high_severity: false,
                },
                false,
                PetPose::Happy,
                Some("pets/bubbles/happy.png"),
            ),
            (
                ObservedCheck::Succeeded {
                    item_count: 0,
                    high_severity: false,
                },
                true,
                PetPose::Sleep,
                Some("pets/bubbles/sleep.png"),
            ),
            (
                ObservedCheck::Succeeded {
                    item_count: 2,
                    high_severity: false,
                },
                false,
                PetPose::Alert,
                Some("pets/bubbles/alert.png"),
            ),
            (
                ObservedCheck::Succeeded {
                    item_count: 2,
                    high_severity: false,
                },
                true,
                PetPose::Sleep,
                Some("pets/bubbles/sleep.png"),
            ),
            (
                ObservedCheck::Succeeded {
                    item_count: 1,
                    high_severity: true,
                },
                false,
                PetPose::Alert,
                Some("pets/bubbles/alert.png"),
            ),
            (
                ObservedCheck::Succeeded {
                    item_count: 1,
                    high_severity: true,
                },
                true,
                PetPose::Alert,
                Some("pets/bubbles/alert.png"),
            ),
        ];
        for (observed, quiet, pose, bubble) in cases {
            assert_eq!(
                pet_pose(observed, quiet),
                pose,
                "{observed:?} quiet={quiet}"
            );
            assert_eq!(bubble_asset(pose), bubble, "{pose:?}");
        }
        assert_eq!(bubble_asset(PetPose::Normal), None);
    }

    #[test]
    fn every_skin_and_pose_maps_to_one_sprite_and_one_tray_glyph() {
        for skin in PetSkin::ALL {
            for pose in PetPose::ALL {
                let path = pet_asset(skin, pose);
                assert!(path.starts_with(match skin {
                    PetSkin::Astronaut => "pets/astronaut/",
                    PetSkin::Star => "pets/star/",
                }));
                assert!(path.ends_with(".png"));
                let glyph = tray_glyph(pose);
                match pose {
                    PetPose::Alert => assert_eq!(glyph, TrayGlyph::Alert),
                    PetPose::Error => assert_eq!(glyph, TrayGlyph::Error),
                    PetPose::Normal | PetPose::Sleep | PetPose::Happy => {
                        assert_eq!(glyph, TrayGlyph::Normal);
                    }
                }
            }
        }
        assert_eq!(
            pet_asset(PetSkin::Astronaut, PetPose::Normal),
            "pets/astronaut/normal.png"
        );
        assert_eq!(
            pet_asset(PetSkin::Star, PetPose::Happy),
            "pets/star/happy.png"
        );
        for glyph in TrayGlyph::ALL {
            for edge in TrayEdge::ALL {
                let path = tray_asset(glyph, edge);
                assert!(path.contains(&edge.px().to_string()), "{path}");
            }
        }
    }

    #[test]
    fn scale_is_2x_or_3x_and_the_bubble_sits_on_the_top_right() {
        assert_eq!(scaled_pet_px(PetSkin::Astronaut, PetScale::X2), 48);
        assert_eq!(scaled_pet_px(PetSkin::Star, PetScale::X2), 64);
        assert_eq!(scaled_pet_px(PetSkin::Astronaut, PetScale::X3), 72);
        assert_eq!(scaled_pet_px(PetSkin::Star, PetScale::X3), 96);
        assert_eq!(PetScale::from_factor(2), Some(PetScale::X2));
        assert_eq!(PetScale::from_factor(3), Some(PetScale::X3));
        assert_eq!(PetScale::from_factor(1), None);
        assert_eq!(PetScale::from_factor(4), None);

        let frame = pet_frame(PetSkin::Astronaut, PetScale::X2);
        assert_eq!(frame.pet.width, 48);
        assert_eq!(frame.pet.height, 48);
        assert_eq!(frame.bubble.width, 32);
        assert_eq!(frame.bubble.height, 32);
        assert!(frame.bubble.x + frame.bubble.width / 2 >= frame.pet.x + frame.pet.width - 1);
        assert!(frame.bubble.y < frame.pet.y + frame.pet.height);
        assert!(frame.bubble.x + frame.bubble.width <= frame.canvas.width);
        assert!(frame.bubble.y + frame.bubble.height <= frame.canvas.height);
        assert!(frame.pet.y + frame.pet.height <= frame.canvas.height);

        let star = pet_frame(PetSkin::Star, PetScale::X2);
        assert_eq!(star.pet.width, 64);
        assert!(star.canvas.width >= 64);
        assert!(star.canvas.height >= 64);
    }

    #[test]
    fn tray_edge_follows_the_monitor_scale() {
        assert_eq!(tray_edge_px(1.0), 16);
        assert_eq!(tray_edge_px(1.49), 16);
        assert_eq!(tray_edge_px(1.5), 32);
        assert_eq!(tray_edge_px(2.0), 32);
        assert_eq!(tray_edge_px(2.99), 32);
        assert_eq!(tray_edge_px(3.0), 64);
        assert_eq!(tray_edge_px(4.0), 64);
        assert_eq!(tray_edge_px(f64::NAN), 16);
        assert_eq!(tray_edge_px(-1.0), 16);
        assert_eq!(tray_edge_for_px(16), TrayEdge::Px16);
        assert_eq!(tray_edge_for_px(32), TrayEdge::Px32);
        assert_eq!(tray_edge_for_px(64), TrayEdge::Px64);
        assert_eq!(tray_edge_for_px(tray_edge_px(2.0)).px(), 32);
    }

    #[test]
    fn menu_ids_round_trip_and_labels_stay_plain() {
        for skin in PetSkin::ALL {
            assert_eq!(skin_from_menu(skin.menu_id()), Some(skin));
            assert_eq!(find_disallowed_copy(skin.label()), None, "{}", skin.label());
        }
        for scale in PetScale::ALL {
            assert_eq!(scale_from_menu(scale.menu_id()), Some(scale));
            assert_eq!(
                find_disallowed_copy(scale.label()),
                None,
                "{}",
                scale.label()
            );
        }
        assert_eq!(skin_from_menu("skin-comet"), None);
        assert_eq!(scale_from_menu("scale-4"), None);
        assert_eq!(find_disallowed_copy("Pet"), None);
    }
}
