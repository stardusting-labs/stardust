//! Screen position for the character and the speech bubble.
//!
//! The stored point is the exact position the user left. Display uses a
//! clamped copy when that point is off a visible monitor. Clamping does not
//! rewrite the stored point.

pub const BUBBLE_GAP_PX: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BubbleLayout {
    pub origin: ScreenPoint,
    pub show_character: bool,
    pub character_origin: Option<ScreenPoint>,
}

pub struct LayoutRequest<'a> {
    pub character_visible: bool,
    pub stored_position: ScreenPoint,
    pub monitors: &'a [ScreenRect],
    pub character_size: WindowSize,
    pub bubble_size: WindowSize,
}

pub fn clamp_to_visible(
    point: ScreenPoint,
    window: WindowSize,
    monitors: &[ScreenRect],
) -> ScreenPoint {
    let Some(monitor) = choose_monitor(point, monitors) else {
        return point;
    };
    clamp_inside(point, window, monitor)
}

pub fn attached_bubble_point(character_origin: ScreenPoint, bubble: WindowSize) -> ScreenPoint {
    ScreenPoint {
        x: character_origin.x,
        y: character_origin
            .y
            .saturating_sub(as_i32(bubble.height))
            .saturating_sub(BUBBLE_GAP_PX),
    }
}

pub fn layout_bubble(request: &LayoutRequest<'_>) -> BubbleLayout {
    if request.character_visible {
        let character_origin = clamp_to_visible(
            request.stored_position,
            request.character_size,
            request.monitors,
        );
        let anchor = attached_bubble_point(character_origin, request.bubble_size);
        BubbleLayout {
            origin: clamp_to_visible(anchor, request.bubble_size, request.monitors),
            show_character: true,
            character_origin: Some(character_origin),
        }
    } else {
        BubbleLayout {
            origin: clamp_to_visible(
                request.stored_position,
                request.bubble_size,
                request.monitors,
            ),
            show_character: false,
            character_origin: None,
        }
    }
}

fn choose_monitor(point: ScreenPoint, monitors: &[ScreenRect]) -> Option<&ScreenRect> {
    let mut best: Option<(i64, usize, &ScreenRect)> = None;
    for (index, monitor) in monitors.iter().enumerate() {
        if monitor.width == 0 || monitor.height == 0 {
            continue;
        }
        let distance = if contains(monitor, point) {
            0
        } else {
            distance_sq(point, monitor)
        };
        match best {
            Some((best_distance, _, _)) if distance >= best_distance => {}
            _ => best = Some((distance, index, monitor)),
        }
    }
    best.map(|(_, _, monitor)| monitor)
}

fn contains(monitor: &ScreenRect, point: ScreenPoint) -> bool {
    let right = monitor.x.saturating_add(as_i32(monitor.width));
    let bottom = monitor.y.saturating_add(as_i32(monitor.height));
    point.x >= monitor.x && point.x < right && point.y >= monitor.y && point.y < bottom
}

fn distance_sq(point: ScreenPoint, monitor: &ScreenRect) -> i64 {
    let right = monitor
        .x
        .saturating_add(as_i32(monitor.width).saturating_sub(1))
        .max(monitor.x);
    let bottom = monitor
        .y
        .saturating_add(as_i32(monitor.height).saturating_sub(1))
        .max(monitor.y);
    let nearest_x = point.x.clamp(monitor.x, right);
    let nearest_y = point.y.clamp(monitor.y, bottom);
    let dx = i64::from(point.x) - i64::from(nearest_x);
    let dy = i64::from(point.y) - i64::from(nearest_y);
    dx * dx + dy * dy
}

fn clamp_inside(point: ScreenPoint, window: WindowSize, monitor: &ScreenRect) -> ScreenPoint {
    let win_w = as_i32(window.width).max(1);
    let win_h = as_i32(window.height).max(1);
    let mon_w = as_i32(monitor.width).max(1);
    let mon_h = as_i32(monitor.height).max(1);
    let max_x = if win_w >= mon_w {
        monitor.x
    } else {
        monitor.x.saturating_add(mon_w - win_w)
    };
    let max_y = if win_h >= mon_h {
        monitor.y
    } else {
        monitor.y.saturating_add(mon_h - win_h)
    };
    ScreenPoint {
        x: point.x.clamp(monitor.x, max_x),
        y: point.y.clamp(monitor.y, max_y),
    }
}

fn as_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        attached_bubble_point, clamp_to_visible, layout_bubble, BubbleLayout, LayoutRequest,
        ScreenPoint, ScreenRect, WindowSize,
    };

    const CHARACTER: WindowSize = WindowSize {
        width: 80,
        height: 80,
    };
    const BUBBLE: WindowSize = WindowSize {
        width: 200,
        height: 60,
    };

    fn point(x: i32, y: i32) -> ScreenPoint {
        ScreenPoint { x, y }
    }

    fn layout(visible: bool, stored: ScreenPoint, monitors: &[ScreenRect]) -> BubbleLayout {
        layout_bubble(&LayoutRequest {
            character_visible: visible,
            stored_position: stored,
            monitors,
            character_size: CHARACTER,
            bubble_size: BUBBLE,
        })
    }

    #[test]
    fn hidden_bubble_uses_the_stored_point_and_omits_the_character() {
        let monitors = [ScreenRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }];
        let stored = point(100, 500);
        let shown = layout(false, stored, &monitors);
        assert!(!shown.show_character);
        assert_eq!(shown.character_origin, None);
        assert_eq!(shown.origin, stored);
    }

    #[test]
    fn visible_bubble_attaches_above_the_character() {
        let monitors = [ScreenRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }];
        let stored = point(100, 500);
        let shown = layout(true, stored, &monitors);
        assert!(shown.show_character);
        assert_eq!(shown.character_origin, Some(stored));
        assert_eq!(shown.origin, attached_bubble_point(stored, BUBBLE));
        assert_eq!(shown.origin, point(100, 500 - 60 - 8));
    }

    #[test]
    fn offscreen_position_is_clamped_for_display_only() {
        let monitors = [ScreenRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }];
        let stored = point(5_000, 5_000);
        let shown = layout(false, stored, &monitors);
        assert_eq!(shown.origin, point(1920 - 200, 1080 - 60));
        assert_eq!(stored, point(5_000, 5_000));
        assert!(!shown.show_character);
    }

    #[test]
    fn clamps_each_edge_and_a_window_larger_than_the_monitor() {
        let monitor = ScreenRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(
            clamp_to_visible(point(-40, -20), BUBBLE, &[monitor]),
            point(0, 0)
        );
        assert_eq!(
            clamp_to_visible(
                point(1900, 100),
                WindowSize {
                    width: 100,
                    height: 40
                },
                &[monitor]
            ),
            point(1820, 100)
        );
        assert_eq!(
            clamp_to_visible(
                point(50, 50),
                WindowSize {
                    width: 200,
                    height: 40
                },
                &[ScreenRect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                }]
            ),
            point(0, 50)
        );
    }

    #[test]
    fn chooses_the_nearest_monitor_including_one_on_the_left() {
        let left = ScreenRect {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let right = ScreenRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let window = WindowSize {
            width: 100,
            height: 40,
        };
        assert_eq!(
            clamp_to_visible(point(-100, 40), window, &[left, right]),
            point(-100, 40)
        );
        assert_eq!(
            clamp_to_visible(point(-50, 40), window, &[left, right]),
            point(-100, 40)
        );
        assert_eq!(
            clamp_to_visible(point(-5_000, 40), window, &[left, right]),
            point(-1920, 40)
        );

        let gap_left = ScreenRect {
            x: 0,
            y: 0,
            width: 1000,
            height: 800,
        };
        let gap_right = ScreenRect {
            x: 2000,
            y: 0,
            width: 1000,
            height: 800,
        };
        assert_eq!(
            clamp_to_visible(point(1700, 100), window, &[gap_left, gap_right]),
            point(2000, 100)
        );
        assert_eq!(
            clamp_to_visible(point(1400, 100), window, &[gap_left, gap_right]),
            point(900, 100)
        );
    }

    #[test]
    fn empty_monitor_list_keeps_the_point() {
        let stored = point(-20, 15);
        assert_eq!(clamp_to_visible(stored, BUBBLE, &[]), stored);
        assert_eq!(layout(false, stored, &[]).origin, stored);
    }
}
