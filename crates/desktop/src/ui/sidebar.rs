use crate::icons::{self, Icon};
use crate::theme::{self, SECONDARY, SIDEBAR_WIDTH, TEXT};
use crate::{App, ItemKind, Message, Route};
use iced::widget::{column, container, scrollable, text};
use iced::{Element, Length};

impl App {
    pub(crate) fn sidebar(&self) -> Element<'_, Message> {
        let library = matches!(self.route, Route::Library | Route::Liked | Route::Browse(_));
        let mut nav = column![
            text("RustMusic")
                .font(theme::bold())
                .size(22)
                .color(theme::BRAND),
            nav_button(
                Icon::Home,
                "Главная",
                matches!(self.route, Route::Home),
                Message::Go(Route::Home)
            ),
            nav_button(
                Icon::Explore,
                "Навигатор",
                matches!(self.route, Route::Search(_)),
                Message::Navigator,
            ),
            nav_button(
                Icon::Library,
                "Библиотека",
                library,
                Message::Go(Route::Library)
            ),
        ]
        .spacing(4);

        nav = nav.push(text("Плейлисты").size(12).color(theme::TERTIARY));
        nav = nav.push(sub_link("Любимые треки", Message::Go(Route::Liked)));
        nav = nav.push(sub_link(
            "Альбомы",
            Message::Go(Route::Browse("FEmusic_liked_albums".into())),
        ));
        nav = nav.push(sub_link(
            "Исполнители",
            Message::Go(Route::Browse("FEmusic_library_corpus_track_artists".into())),
        ));
        if matches!(self.route, Route::Library) {
            for item in self
                .items
                .iter()
                .filter(|item| item.kind == ItemKind::Playlist)
            {
                nav = nav.push(sub_link(
                    item.title.as_str(),
                    Message::Go(Route::Browse(item.id.clone())),
                ));
            }
        }

        let account = self
            .account
            .as_ref()
            .map(|account| account.name.as_str())
            .unwrap_or("Аккаунт не подключён");
        container(
            column![
                scrollable(nav).height(Length::Fill),
                text(account).size(14).color(SECONDARY),
                icons::text_button(text("Аккаунт / настройки").size(14).color(TEXT))
                    .on_press(Message::Settings)
                    .width(Length::Fill),
                text(format!(
                    "{} · {}",
                    crate::app_version(),
                    crate::app_commit()
                ))
                .size(12)
                .color(theme::TERTIARY),
            ]
            .spacing(12)
            .padding(16),
        )
        .width(SIDEBAR_WIDTH)
        .height(Length::Fill)
        .style(theme::surface)
        .into()
    }
}

fn nav_button<'a>(
    icon: Icon,
    label: &'a str,
    active: bool,
    message: Message,
) -> Element<'a, Message> {
    let color = if active { TEXT } else { SECONDARY };
    iced::widget::button(
        iced::widget::row![
            icons::glyph(icon, color, 1.0),
            text(label).size(14).color(color)
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center),
    )
    .padding([10, 12])
    .width(Length::Fill)
    .on_press(message)
    .style(theme::nav_style(active))
    .into()
}

fn sub_link<'a>(label: &'a str, message: Message) -> Element<'a, Message> {
    icons::text_button(text(label).size(14).color(SECONDARY))
        .on_press(message)
        .width(Length::Fill)
        .into()
}
