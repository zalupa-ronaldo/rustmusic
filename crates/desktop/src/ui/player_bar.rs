use crate::icons::{self, Icon};
use crate::theme::{self, PLAY_BUTTON, PLAYER_HEIGHT, TEXT, TRANSPORT_BUTTON};
use crate::{App, DownloadPhase, Message, Repeat};
use iced::widget::{column, container, row, scrollable, slider, stack, text};
use iced::{Element, Length};

impl App {
    pub(crate) fn player_bar(&self) -> Element<'_, Message> {
        let current = self.queue.current();
        let id = current.map(|item| item.id.as_str());
        let liked = id.is_some_and(|id| self.liked.contains(id));
        let disliked = id.is_some_and(|id| self.disliked.contains(id));
        let play_t = self.play_anim.interpolate(0.0, 1.0, self.now);
        let play_face = stack![
            container(icons::glyph(Icon::Play, theme::CONTENT, 1.0 - play_t))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
            container(icons::glyph(Icon::Pause, theme::CONTENT, play_t))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ];
        let mut play = iced::widget::button(play_face)
            .width(PLAY_BUTTON)
            .height(PLAY_BUTTON)
            .padding(0)
            .style(play_style);
        if current.is_some() {
            play = play.on_press(Message::Toggle);
        }
        let repeat_icon = match self.queue.repeat {
            Repeat::One => Icon::RepeatOne,
            Repeat::Off | Repeat::All => Icon::Repeat,
        };
        let repeat_color = if self.queue.repeat == Repeat::Off {
            theme::TERTIARY
        } else {
            theme::ACTIVE
        };
        let cover: Element<_> = current
            .and_then(|item| item.thumbnail.as_ref())
            .and_then(|url| self.covers.get(url))
            .map(|handle| {
                container(iced::widget::image(handle.clone()).width(48).height(48))
                    .clip(true)
                    .style(theme::art_clip(2.0))
                    .into()
            })
            .unwrap_or_else(|| {
                container(icons::glyph(Icon::Library, theme::SECONDARY, 1.0))
                    .width(48)
                    .height(48)
                    .center_x(48)
                    .center_y(48)
                    .into()
            });
        let like_icon = if liked {
            Icon::ThumbUpFilled
        } else {
            Icon::ThumbUp
        };
        let dislike_icon = if disliked {
            Icon::ThumbDownFilled
        } else {
            Icon::ThumbDown
        };
        let (save_icon, save_color) = id
            .map(|id| download_icon(&self.playback.downloads(), id))
            .unwrap_or((Icon::Download, TEXT));
        let transport = row![
            icons::icon_button(
                Icon::Shuffle,
                TRANSPORT_BUTTON,
                TEXT,
                Some(Message::Shuffle)
            ),
            icons::icon_button(
                Icon::SkipPrevious,
                TRANSPORT_BUTTON,
                TEXT,
                Some(Message::Previous)
            ),
            play,
            icons::icon_button(
                Icon::SkipNext,
                TRANSPORT_BUTTON,
                TEXT,
                Some(Message::Next(false))
            ),
            icons::icon_button(
                repeat_icon,
                TRANSPORT_BUTTON,
                repeat_color,
                Some(Message::Repeat)
            ),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center);
        let actions = row![
            icons::icon_button(
                like_icon,
                theme::ROW_BUTTON,
                if liked { theme::ACTIVE } else { TEXT },
                id.map(|id| Message::Mutate(crate::Mutation::Like {
                    video_id: id.to_owned(),
                    liked: !liked,
                })),
            ),
            icons::icon_button(
                dislike_icon,
                theme::ROW_BUTTON,
                if disliked { theme::ACTIVE } else { TEXT },
                id.map(|id| {
                    if disliked {
                        Message::Mutate(crate::Mutation::Like {
                            video_id: id.to_owned(),
                            liked: false,
                        })
                    } else {
                        Message::Mutate(crate::Mutation::Dislike {
                            video_id: id.to_owned(),
                        })
                    }
                }),
            ),
            icons::icon_button(
                save_icon,
                theme::ROW_BUTTON,
                save_color,
                current.map(|_| Message::DownloadCurrent),
            ),
            icons::icon_button(
                Icon::Queue,
                theme::ROW_BUTTON,
                if self.queue_open { theme::ACTIVE } else { TEXT },
                Some(Message::ToggleQueue),
            ),
            icons::icon_button(
                Icon::Expand,
                theme::ROW_BUTTON,
                TEXT,
                Some(Message::ToggleNowPlaying)
            ),
        ]
        .spacing(2)
        .align_y(iced::Alignment::Center);
        let meta = column![
            text(
                current
                    .map(|item| item.title.as_str())
                    .unwrap_or("Выберите музыку")
            )
            .size(14)
            .font(theme::bold())
            .color(TEXT),
            text(
                current
                    .map(|item| item.subtitle.as_str())
                    .unwrap_or("Нативный звук")
            )
            .size(12)
            .color(theme::SECONDARY),
        ]
        .spacing(2);
        let thumb = if self.seek_drag { 20.0 } else { 12.0 };
        let seek = row![
            text(crate::time(self.position))
                .size(12)
                .color(theme::SECONDARY),
            slider(
                0.0..=self.duration.max(1.0),
                self.position.min(self.duration.max(1.0)),
                Message::Seek,
            )
            .width(Length::Fill)
            .height(16)
            .step(0.1)
            .on_release(Message::SeekReleased)
            .style(theme::seek_style(thumb))
            .width(Length::Fill),
            text(crate::time(self.duration))
                .size(12)
                .color(theme::SECONDARY),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        let volume = row![
            icons::glyph(Icon::Volume, TEXT, 1.0),
            slider(0.0..=100.0, self.volume, Message::Volume)
                .width(100)
                .style(theme::volume_style)
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        container(
            column![
                seek,
                row![
                    row![cover, meta]
                        .spacing(12)
                        .align_y(iced::Alignment::Center)
                        .width(Length::Fill),
                    transport,
                    row![volume, actions]
                        .spacing(8)
                        .align_y(iced::Alignment::Center)
                        .width(Length::Fill),
                ]
                .spacing(16)
                .padding([0, 16])
                .align_y(iced::Alignment::Center)
                .height(PLAYER_HEIGHT),
                status_line(self),
            ]
            .spacing(0),
        )
        .width(Length::Fill)
        .style(theme::player_bg)
        .into()
    }

    pub(crate) fn queue_panel(&self) -> Element<'_, Message> {
        let rows = self.queue.items.iter().enumerate().map(|(index, item)| {
            let hot = self.hovered_queue == Some(index) || self.queue.index == Some(index);
            let marker = self.drag.is_some_and(|drag| {
                drag.list == crate::DragList::Queue && drag.hover == index && drag.from != index
            });
            let title = if marker {
                format!("▸ {}", item.title)
            } else {
                item.title.clone()
            };
            let row = iced::widget::row![
                icons::icon_button(
                    Icon::Drag,
                    32.0,
                    theme::TERTIARY,
                    Some(Message::DragStart(crate::DragList::Queue, index)),
                ),
                text(title).size(14).color(TEXT),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center);
            iced::widget::mouse_area(
                container(
                    iced::widget::button(row)
                        .width(Length::Fill)
                        .padding([8, 8])
                        .on_press(Message::PlayQueue(index))
                        .style(theme::text_button_style),
                )
                .style(theme::row_bg(hot)),
            )
            .on_enter(Message::HoverQueue(Some(index)))
            .on_exit(Message::HoverQueue(None))
            .into()
        });
        container(
            column![
                text("Очередь").font(theme::bold()).size(16).color(TEXT),
                scrollable(column(rows).spacing(2)).height(Length::Fill),
            ]
            .spacing(12)
            .padding(16),
        )
        .width(320)
        .height(Length::Fill)
        .style(theme::surface)
        .into()
    }
}

fn status_line(app: &App) -> Element<'_, Message> {
    if app.player_status.is_empty() {
        return iced::widget::Space::new().height(0).into();
    }
    let color = if app.open_failed {
        theme::BRAND
    } else {
        theme::SECONDARY
    };
    container(text(&app.player_status).size(12).color(color))
        .padding(iced::Padding {
            top: 0.0,
            right: 16.0,
            bottom: 8.0,
            left: 16.0,
        })
        .into()
}

fn download_icon(store: &rustmusic_core::DownloadStore, id: &str) -> (Icon, iced::Color) {
    match store.phase(id) {
        DownloadPhase::Ready => (Icon::DownloadDone, theme::ACTIVE),
        DownloadPhase::Active(_) => (Icon::Download, theme::LINK),
        DownloadPhase::Failed => (Icon::Download, theme::BRAND),
        DownloadPhase::Idle => (Icon::Download, TEXT),
    }
}

fn play_style(
    _theme: &iced::Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    let hot = matches!(
        status,
        iced::widget::button::Status::Hovered | iced::widget::button::Status::Pressed
    );
    iced::widget::button::Style {
        background: Some(iced::Background::from(if hot {
            theme::SECONDARY
        } else {
            TEXT
        })),
        text_color: theme::CONTENT,
        border: iced::Border::default().rounded(999.0),
        shadow: iced::Shadow::default(),
        snap: true,
    }
}
