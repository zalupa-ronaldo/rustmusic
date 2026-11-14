//! YouTube Music-like dark tokens for the desktop shell.
//!
//! Colors and sizes are plain values. The UI uses the system sans font,
//! which is SF Pro on macOS. No YouTube font or brand artwork is bundled.
use iced::{
    Background, Border, Color, Shadow, Theme,
    animation::{Animation, Easing},
    widget::{button, container, slider},
};
use std::time::Duration;

pub const CONTENT: Color = Color::from_rgb(0.0, 0.0, 0.0);
pub const SURFACE: Color = Color::from_rgb(
    0x1D as f32 / 255.0,
    0x1D as f32 / 255.0,
    0x1D as f32 / 255.0,
);
pub const PLAYER: Color = Color::from_rgb(
    0x21 as f32 / 255.0,
    0x21 as f32 / 255.0,
    0x21 as f32 / 255.0,
);
pub const MENU: Color = Color::from_rgb(
    0x32 as f32 / 255.0,
    0x32 as f32 / 255.0,
    0x32 as f32 / 255.0,
);
pub const TEXT: Color = Color::from_rgb(
    0xFF as f32 / 255.0,
    0xFF as f32 / 255.0,
    0xFE as f32 / 255.0,
);
pub const SECONDARY: Color = Color::from_rgb(
    0xAA as f32 / 255.0,
    0xAA as f32 / 255.0,
    0xAA as f32 / 255.0,
);
pub const TERTIARY: Color = Color::from_rgb(
    0x90 as f32 / 255.0,
    0x90 as f32 / 255.0,
    0x90 as f32 / 255.0,
);
pub const HOVER: Color = Color::from_rgba(1.0, 1.0, 1.0, 0x1A as f32 / 255.0);
pub const DIVIDER: Color = Color::from_rgba(1.0, 1.0, 1.0, 0x14 as f32 / 255.0);
pub const LINK: Color = Color::from_rgb(
    0x3E as f32 / 255.0,
    0xA6 as f32 / 255.0,
    0xFF as f32 / 255.0,
);
pub const ACTIVE: Color = Color::from_rgb(
    0x0B as f32 / 255.0,
    0x83 as f32 / 255.0,
    0xE5 as f32 / 255.0,
);
pub const BRAND: Color = Color::from_rgb(1.0, 0.0, 0x33 as f32 / 255.0);

pub const SIDEBAR_WIDTH: f32 = 240.0;
pub const PLAYER_HEIGHT: f32 = 76.0;
pub const TRACK_ROW: f32 = 56.0;
pub const COVER_ROW: f32 = 40.0;
pub const ICON: f32 = 24.0;
pub const PLAY_BUTTON: f32 = 72.0;
pub const TRANSPORT_BUTTON: f32 = 48.0;
pub const ROW_BUTTON: f32 = 40.0;

pub fn bold() -> iced::Font {
    iced::Font {
        weight: iced::font::Weight::Bold,
        ..iced::Font::DEFAULT
    }
}

/// CSS cubic-bezier(0.2, 0, 0, 1).
pub fn ease_standard(t: f32) -> f32 {
    cubic_bezier(0.2, 0.0, 0.0, 1.0, t)
}

/// CSS cubic-bezier(0.4, 0, 0.2, 1).
pub fn ease_expand(t: f32) -> f32 {
    cubic_bezier(0.4, 0.0, 0.2, 1.0, t)
}

pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let mut t = x;
    for _ in 0..8 {
        let dx = bezier_1d(t, x1, x2) - x;
        let slope = bezier_slope(t, x1, x2);
        if slope.abs() < 1e-4 {
            break;
        }
        t = (t - dx / slope).clamp(0.0, 1.0);
    }
    bezier_1d(t, y1, y2)
}

fn bezier_1d(t: f32, p1: f32, p2: f32) -> f32 {
    let u = 1.0 - t;
    3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t
}

fn bezier_slope(t: f32, p1: f32, p2: f32) -> f32 {
    let u = 1.0 - t;
    3.0 * u * u * p1 + 6.0 * u * t * (p2 - p1) + 3.0 * t * t * (1.0 - p2)
}

/// Now-playing copy fades in after 42% of the slide.
pub fn content_fade(travel: f32) -> f32 {
    ((travel - 0.42) / 0.58).clamp(0.0, 1.0)
}

pub fn expand_animation() -> Animation<bool> {
    Animation::new(false)
        .duration(Duration::from_millis(300))
        .easing(Easing::Custom(ease_expand))
}

pub fn standard_animation(ms: u64) -> Animation<bool> {
    Animation::new(false)
        .duration(Duration::from_millis(ms))
        .easing(Easing::Custom(ease_standard))
}

pub fn mix(from: Color, to: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color {
        r: from.r + (to.r - from.r) * t,
        g: from.g + (to.g - from.g) * t,
        b: from.b + (to.b - from.b) * t,
        a: from.a + (to.a - from.a) * t,
    }
}

/// Most common saturated color in a decoded cover. Near-black and gray pixels
/// are ignored so a dark thumbnail still yields a stage color.
pub fn dominant_color(rgba: &[u8]) -> Option<Color> {
    if rgba.len() < 4 {
        return None;
    }
    let mut bins = [0u32; 64];
    let mut sum = [(0u32, 0u32, 0u32); 64];
    for px in rgba.chunks_exact(4) {
        let r = px[0] as f32;
        let g = px[1] as f32;
        let b = px[2] as f32;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        if max < 36.0 || max - min < 18.0 {
            continue;
        }
        let key = ((px[0] / 64) << 4 | (px[1] / 64) << 2 | (px[2] / 64)) as usize;
        bins[key] += 1;
        sum[key].0 += px[0] as u32;
        sum[key].1 += px[1] as u32;
        sum[key].2 += px[2] as u32;
    }
    let (key, count) = bins
        .iter()
        .copied()
        .enumerate()
        .max_by_key(|(_, count)| *count)?;
    if count == 0 {
        return None;
    }
    Some(Color::from_rgb(
        sum[key].0 as f32 / count as f32 / 255.0,
        sum[key].1 as f32 / count as f32 / 255.0,
        sum[key].2 as f32 / count as f32 / 255.0,
    ))
}

fn flat(status: button::Status, radius: f32, fill: Option<Color>) -> button::Style {
    let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let background = if hot {
        Some(Background::from(HOVER))
    } else {
        fill.map(Background::from)
    };
    button::Style {
        background,
        text_color: TEXT,
        border: Border::default().rounded(radius),
        shadow: Shadow::default(),
        snap: true,
    }
}

pub fn icon_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    flat(status, 999.0, None)
}

pub fn nav_style(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: (hot || active).then(|| Background::from(HOVER)),
            text_color: if active { TEXT } else { SECONDARY },
            border: Border::default().rounded(8.0),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

pub fn text_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    flat(status, 8.0, None)
}

pub fn menu_item_style(_theme: &Theme, status: button::Status) -> button::Style {
    flat(status, 8.0, None)
}

pub fn surface(_theme: &Theme) -> container::Style {
    container::Style::default().background(SURFACE)
}

pub fn content_bg(_theme: &Theme) -> container::Style {
    container::Style::default().background(CONTENT)
}

pub fn player_bg(_theme: &Theme) -> container::Style {
    container::Style::default().background(PLAYER)
}

pub fn menu_bg(_theme: &Theme) -> container::Style {
    container::Style::default()
        .background(MENU)
        .border(Border::default().rounded(12.0))
}

pub fn row_bg(hot: bool) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: hot.then(|| Background::from(HOVER)),
        border: Border::default().rounded(4.0),
        ..container::Style::default()
    }
}

pub fn art_clip(radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style::default().border(Border::default().rounded(radius))
}

pub fn seek_style(thumb: f32) -> impl Fn(&Theme, slider::Status) -> slider::Style {
    move |_theme, _status| slider::Style {
        rail: slider::Rail {
            backgrounds: (Background::from(TEXT), Background::from(DIVIDER)),
            width: 2.0,
            border: Border::default().rounded(2.0),
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle {
                radius: thumb / 2.0,
            },
            background: Background::from(TEXT),
            border_width: 0.0,
            border_color: TEXT,
        },
    }
}

pub fn volume_style(_theme: &Theme, _status: slider::Status) -> slider::Style {
    slider::Style {
        rail: slider::Rail {
            backgrounds: (Background::from(TEXT), Background::from(DIVIDER)),
            width: 4.0,
            border: Border::default().rounded(2.0),
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius: 6.0 },
            background: Background::from(TEXT),
            border_width: 0.0,
            border_color: TEXT,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_match_the_desktop_spec() {
        let content = std::hint::black_box(CONTENT);
        let text = std::hint::black_box(TEXT);
        let brand = std::hint::black_box(BRAND);
        let hover = std::hint::black_box(HOVER);
        let sidebar = std::hint::black_box(SIDEBAR_WIDTH);
        let row = std::hint::black_box(TRACK_ROW);
        let play = std::hint::black_box(PLAY_BUTTON);
        let transport = std::hint::black_box(TRANSPORT_BUTTON);
        let cover = std::hint::black_box(COVER_ROW);
        assert!(content.r == 0.0 && content.g == 0.0 && content.b == 0.0);
        assert!(text.r > 0.99 && text.g > 0.99);
        assert!(brand.r > 0.9 && brand.g == 0.0 && brand.b > 0.15);
        assert!((hover.a - 0x1A as f32 / 255.0).abs() < 0.001);
        assert!(sidebar > row && play > transport);
        assert!(row - cover > 8.0);
    }

    #[test]
    fn easing_stays_in_range_and_expand_is_not_linear() {
        assert!((ease_standard(0.0) - 0.0).abs() < 0.02);
        assert!((ease_standard(1.0) - 1.0).abs() < 0.02);
        assert!(ease_standard(0.5) > 0.5);
        assert!((ease_expand(0.0)).abs() < 0.02);
        assert!((ease_expand(1.0) - 1.0).abs() < 0.02);
    }

    #[test]
    fn now_playing_copy_waits_for_forty_two_percent() {
        assert_eq!(content_fade(0.0), 0.0);
        assert_eq!(content_fade(0.42), 0.0);
        assert!(content_fade(0.71) > 0.4);
        assert_eq!(content_fade(1.0), 1.0);
    }

    #[test]
    fn dominant_color_prefers_the_saturated_pixels() {
        let mut pixels = vec![8, 8, 8, 255];
        pixels.extend(std::iter::repeat_n([220, 30, 40, 255], 40).flatten());
        let color = dominant_color(&pixels).unwrap();
        assert!(color.r > color.g && color.r > color.b);
        assert!(dominant_color(&[0, 0, 0, 255]).is_none());
    }
}
