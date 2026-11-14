//! Material Symbols shipped as Apache-2.0 SVG files.
//!
//! See `assets/icons/NOTICE`. These are not YouTube Music icons.
use crate::Message;
use crate::theme::{self, ICON};
use iced::widget::{container, svg};
use iced::{Color, Element, Length};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    SkipNext,
    SkipPrevious,
    Shuffle,
    Repeat,
    RepeatOne,
    ThumbUp,
    ThumbUpFilled,
    ThumbDown,
    ThumbDownFilled,
    More,
    Download,
    DownloadDone,
    Check,
    Queue,
    PlaylistAdd,
    Expand,
    Home,
    Explore,
    Library,
    Search,
    Volume,
    Share,
    Drag,
}

impl Icon {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Play => include_bytes!("../assets/icons/play_arrow.svg"),
            Self::Pause => include_bytes!("../assets/icons/pause.svg"),
            Self::SkipNext => include_bytes!("../assets/icons/skip_next.svg"),
            Self::SkipPrevious => include_bytes!("../assets/icons/skip_previous.svg"),
            Self::Shuffle => include_bytes!("../assets/icons/shuffle.svg"),
            Self::Repeat => include_bytes!("../assets/icons/repeat.svg"),
            Self::RepeatOne => include_bytes!("../assets/icons/repeat_one.svg"),
            Self::ThumbUp => include_bytes!("../assets/icons/thumb_up.svg"),
            Self::ThumbUpFilled => include_bytes!("../assets/icons/thumb_up_filled.svg"),
            Self::ThumbDown => include_bytes!("../assets/icons/thumb_down.svg"),
            Self::ThumbDownFilled => include_bytes!("../assets/icons/thumb_down_filled.svg"),
            Self::More => include_bytes!("../assets/icons/more_vert.svg"),
            Self::Download => include_bytes!("../assets/icons/download.svg"),
            Self::DownloadDone => include_bytes!("../assets/icons/download_done.svg"),
            Self::Check => include_bytes!("../assets/icons/check_circle.svg"),
            Self::Queue => include_bytes!("../assets/icons/queue_music.svg"),
            Self::PlaylistAdd => include_bytes!("../assets/icons/playlist_add.svg"),
            Self::Expand => include_bytes!("../assets/icons/expand_more.svg"),
            Self::Home => include_bytes!("../assets/icons/home.svg"),
            Self::Explore => include_bytes!("../assets/icons/explore.svg"),
            Self::Library => include_bytes!("../assets/icons/library_music.svg"),
            Self::Search => include_bytes!("../assets/icons/search.svg"),
            Self::Volume => include_bytes!("../assets/icons/volume_up.svg"),
            Self::Share => include_bytes!("../assets/icons/share.svg"),
            Self::Drag => include_bytes!("../assets/icons/drag_indicator.svg"),
        }
    }
}

pub fn glyph<'a>(icon: Icon, color: Color, opacity: f32) -> Element<'a, Message> {
    let color = color.scale_alpha(opacity.clamp(0.0, 1.0));
    svg(svg::Handle::from_memory(icon.bytes()))
        .width(ICON)
        .height(ICON)
        .style(move |_theme, _status| svg::Style { color: Some(color) })
        .opacity(opacity.clamp(0.0, 1.0))
        .into()
}

pub fn icon_button<'a>(
    icon: Icon,
    size: f32,
    color: Color,
    message: Option<Message>,
) -> iced::widget::Button<'a, Message> {
    let mut button = iced::widget::button(
        container(glyph(icon, color, 1.0))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(size)
    .height(size)
    .padding(0)
    .style(theme::icon_button_style);
    if let Some(message) = message {
        button = button.on_press(message);
    }
    button
}

pub fn text_button<'a>(
    label: impl Into<Element<'a, Message>>,
) -> iced::widget::Button<'a, Message> {
    iced::widget::button(label)
        .padding([8, 12])
        .style(theme::text_button_style)
}
