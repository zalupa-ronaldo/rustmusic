use crate::icons::{self, Icon};
use crate::theme::{self, COVER_ROW, TEXT, TRACK_ROW};
use crate::{App, Item, ItemKind, MenuCommand, Message, Route};
use iced::widget::{button, column, container, mouse_area, row, scrollable, text, text_input};
use iced::{Element, Length};

impl App {
    pub(crate) fn library(&self) -> Element<'_, Message> {
        let mut list = column![
            self.search_bar(),
            text(self.route.title())
                .font(theme::bold())
                .size(28)
                .color(TEXT),
        ]
        .spacing(16)
        .padding(24);
        if self.account.is_some() {
            list = list.push(
                row![
                    text_input("Название плейлиста", &self.title)
                        .on_input(Message::Title)
                        .padding(8)
                        .size(14),
                    icons::text_button(text("Создать").size(14).color(TEXT)).on_press(
                        Message::Mutate(crate::Mutation::Create {
                            title: self.title.clone(),
                        }),
                    ),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
            if let Some(playlist_id) = crate::owned_playlist(&self.route) {
                list = list.push(
                    row![
                        icons::text_button(text("Переименовать").size(14).color(TEXT)).on_press(
                            Message::Mutate(crate::Mutation::Rename {
                                playlist_id: playlist_id.clone(),
                                title: self.title.clone(),
                            }),
                        ),
                        icons::text_button(text("Удалить плейлист").size(14).color(TEXT))
                            .on_press(Message::DeleteConfirm(true)),
                    ]
                    .spacing(8),
                );
                if self.delete_confirm {
                    list = list.push(
                        row![
                            text("Удалить этот плейлист из аккаунта?")
                                .size(14)
                                .color(TEXT),
                            icons::text_button(text("Да, удалить").size(14).color(theme::BRAND))
                                .on_press(Message::Mutate(crate::Mutation::Delete { playlist_id })),
                            icons::text_button(text("Отмена").size(14).color(TEXT))
                                .on_press(Message::DeleteConfirm(false)),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                    );
                }
            }
        }
        for (index, item) in self.items.iter().enumerate() {
            list = list.push(self.track_row(index, item));
        }
        if self.continuation.is_some() {
            let mut more = icons::text_button(text("Загрузить ещё").size(14).color(TEXT));
            if !self.loading {
                more = more.on_press(Message::More);
            }
            list = list.push(more);
        }
        list = list.push(text(&self.status).size(12).color(theme::SECONDARY));
        scrollable(list).height(Length::Fill).into()
    }

    pub(crate) fn track_row<'a>(&'a self, index: usize, item: &'a Item) -> Element<'a, Message> {
        let hot = self.hovered_track == Some(index);
        let track = item.kind == ItemKind::Track;
        let caption = crate::caption::split_subtitle(&item.subtitle);
        let cover: Element<_> = item
            .thumbnail
            .as_ref()
            .and_then(|url| self.covers.get(url))
            .map(|handle| {
                iced::widget::image(handle.clone())
                    .width(COVER_ROW)
                    .height(COVER_ROW)
                    .into()
            })
            .unwrap_or_else(|| {
                container(
                    text(if track { "♫" } else { "▣" })
                        .size(16)
                        .color(theme::BRAND),
                )
                .center_x(COVER_ROW)
                .center_y(COVER_ROW)
                .into()
            });
        let cover = container(cover)
            .width(COVER_ROW)
            .height(COVER_ROW)
            .clip(true)
            .style(theme::art_clip(2.0));
        let play_overlay = if hot && track {
            container(icons::glyph(Icon::Play, TEXT, 1.0))
                .width(COVER_ROW)
                .height(COVER_ROW)
                .center_x(COVER_ROW)
                .center_y(COVER_ROW)
        } else {
            container(iced::widget::Space::new()).width(0).height(0)
        };
        let cover = iced::widget::Stack::with_children([cover.into(), play_overlay.into()]);
        let info = column![
            text(&item.title).size(14).color(TEXT),
            text(if caption.artist.is_empty() {
                caption.album.unwrap_or_default()
            } else {
                caption.artist
            })
            .size(12)
            .color(theme::SECONDARY),
        ]
        .spacing(2)
        .width(Length::Fill);
        let mut line = row![
            button(cover)
                .padding(0)
                .style(theme::text_button_style)
                .on_press(Message::Open(index)),
            info,
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center)
        .height(TRACK_ROW);
        if hot && track {
            let liked = self.liked.contains(&item.id);
            let (save_icon, save_color) = download_icon(&self.playback.downloads(), &item.id);
            line = line.push(icons::icon_button(
                if liked {
                    Icon::ThumbUpFilled
                } else {
                    Icon::ThumbUp
                },
                theme::ROW_BUTTON,
                if liked { theme::ACTIVE } else { TEXT },
                Some(Message::Menu(MenuCommand::Like(index))),
            ));
            line = line.push(icons::icon_button(
                save_icon,
                theme::ROW_BUTTON,
                save_color,
                Some(Message::Menu(MenuCommand::Download(index))),
            ));
            if let rustmusic_core::DownloadPhase::Active(progress) =
                self.playback.downloads().phase(&item.id)
            {
                line = line.push(text(format!("{progress}%")).size(12).color(theme::LINK));
            }
            line = line.push(icons::icon_button(
                Icon::More,
                theme::ROW_BUTTON,
                TEXT,
                Some(Message::OpenMenu(index)),
            ));
        } else {
            line = line.push(
                text(item.duration.as_deref().unwrap_or(""))
                    .size(12)
                    .color(theme::TERTIARY),
            );
        }
        let playlist_drag =
            track && item.set_video_id.is_some() && crate::owned_playlist(&self.route).is_some();
        let marker = self.drag.is_some_and(|drag| {
            drag.list == crate::DragList::Playlist && drag.hover == index && drag.from != index
        });
        let body: Element<_> = if playlist_drag {
            row![
                icons::icon_button(
                    Icon::Drag,
                    32.0,
                    if marker {
                        theme::ACTIVE
                    } else {
                        theme::TERTIARY
                    },
                    Some(Message::DragStart(crate::DragList::Playlist, index)),
                ),
                line,
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center)
            .into()
        } else {
            line.into()
        };
        mouse_area(container(body).padding([0, 8]).style(theme::row_bg(hot)))
            .on_enter(Message::HoverTrack(Some(index)))
            .on_exit(Message::HoverTrack(None))
            .on_move(Message::Pointer)
            .on_right_press(Message::OpenMenu(index))
            .into()
    }

    pub(crate) fn context_menu(&self, index: usize) -> Element<'_, Message> {
        let Some(item) = self.items.get(index) else {
            return iced::widget::Space::new().into();
        };
        let mut menu = column![
            menu_item(
                Icon::Queue,
                "В очередь",
                Message::Menu(MenuCommand::Enqueue(index))
            ),
            menu_item(
                Icon::SkipNext,
                "Следующим",
                Message::Menu(MenuCommand::Next(index))
            ),
            menu_item(
                Icon::Explore,
                "Радио",
                Message::Menu(MenuCommand::Radio(index))
            ),
            menu_item(
                Icon::ThumbUp,
                "Лайк",
                Message::Menu(MenuCommand::Like(index))
            ),
            menu_item(
                Icon::ThumbDown,
                "Дизлайк",
                Message::Menu(MenuCommand::Dislike(index))
            ),
            menu_item(
                Icon::ThumbUp,
                "Снять оценку",
                Message::Menu(MenuCommand::Unlike(index))
            ),
            menu_item(
                Icon::Download,
                "Скачать",
                Message::Menu(MenuCommand::Download(index))
            ),
            menu_item(Icon::Share, "Поделиться", Message::Menu(MenuCommand::Share)),
        ]
        .spacing(0)
        .padding(8)
        .width(280);
        menu = menu.push(
            row![
                text_input("ID плейлиста", &self.target)
                    .on_input(Message::Target)
                    .padding(8)
                    .size(14),
                icons::icon_button(
                    Icon::PlaylistAdd,
                    theme::ROW_BUTTON,
                    TEXT,
                    Some(Message::Menu(MenuCommand::Add(index))),
                ),
            ]
            .spacing(4)
            .padding([4, 8])
            .align_y(iced::Alignment::Center),
        );
        if let Some(set_id) = &item.set_video_id
            && let Route::Browse(_) = &self.route
        {
            let _ = set_id;
            menu = menu.push(menu_item(
                Icon::PlaylistAdd,
                "Убрать из плейлиста",
                Message::Menu(MenuCommand::Remove(index)),
            ));
            if index > 0 && self.items[index - 1].set_video_id.is_some() {
                menu = menu.push(menu_item(
                    Icon::Expand,
                    "Выше",
                    Message::Menu(MenuCommand::Up(index)),
                ));
            }
        }
        container(menu).style(theme::menu_bg).into()
    }

    fn search_bar(&self) -> Element<'_, Message> {
        row![
            icons::glyph(Icon::Search, theme::SECONDARY, 1.0),
            text_input("Найти музыку, альбом или исполнителя", &self.query)
                .on_input(Message::Query)
                .on_submit(Message::Search)
                .padding(12)
                .size(14)
                .width(Length::Fill),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into()
    }
}

fn menu_item<'a>(icon: Icon, label: &'a str, message: Message) -> Element<'a, Message> {
    button(
        row![
            icons::glyph(icon, TEXT, 1.0),
            text(label).size(14).color(TEXT)
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center),
    )
    .width(Length::Fill)
    .height(40)
    .padding([0, 12])
    .on_press(message)
    .style(theme::menu_item_style)
    .into()
}

fn download_icon(store: &rustmusic_core::DownloadStore, id: &str) -> (Icon, iced::Color) {
    match store.phase(id) {
        rustmusic_core::DownloadPhase::Ready => (Icon::Check, theme::ACTIVE),
        rustmusic_core::DownloadPhase::Active(_) => (Icon::Download, theme::LINK),
        rustmusic_core::DownloadPhase::Failed => (Icon::Download, theme::BRAND),
        rustmusic_core::DownloadPhase::Idle => (Icon::Download, TEXT),
    }
}
