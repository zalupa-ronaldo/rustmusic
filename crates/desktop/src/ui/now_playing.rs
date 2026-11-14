use crate::icons::{self, Icon};
use crate::theme::{self, TEXT};
use crate::{App, Message, NowTab};
use iced::widget::{column, container, mouse_area, row, scrollable, text};
use iced::{Element, Length};

impl App {
    pub(crate) fn now_playing(&self, fade: f32) -> Element<'_, Message> {
        let current = self.queue.current();
        let color = |base: iced::Color| base.scale_alpha(fade);
        let cover: Element<_> = current
            .and_then(|item| item.thumbnail.as_ref())
            .and_then(|url| self.covers.get(url))
            .map(|handle| {
                container(
                    iced::widget::image(handle.clone())
                        .width(320)
                        .height(320)
                        .opacity(fade),
                )
                .clip(true)
                .style(theme::art_clip(12.0))
                .into()
            })
            .unwrap_or_else(|| {
                container(icons::glyph(Icon::Library, color(theme::SECONDARY), fade))
                    .width(320)
                    .height(320)
                    .center_x(320)
                    .center_y(320)
                    .style(theme::art_clip(12.0))
                    .into()
            });
        let stage = column![
            text(
                current
                    .map(|item| item.title.as_str())
                    .unwrap_or("Сейчас играет")
            )
            .font(theme::bold())
            .size(24)
            .color(color(TEXT)),
            text(current.map(|item| item.subtitle.as_str()).unwrap_or(""))
                .size(16)
                .color(color(theme::SECONDARY)),
        ]
        .spacing(8);
        let tabs = row![
            tab(self, "Далее", NowTab::UpNext, fade),
            tab(self, "Тексты", NowTab::Lyrics, fade),
            tab(self, "Похожие", NowTab::Related, fade),
        ]
        .spacing(8);
        let body: Element<_> = match self.now_tab {
            NowTab::UpNext => {
                let start = self.queue.index.map(|index| index + 1).unwrap_or(0);
                let rows = self
                    .queue
                    .items
                    .iter()
                    .enumerate()
                    .skip(start)
                    .map(|(index, item)| {
                        button_row(item.title.as_str(), fade, Message::PlayQueue(index))
                    });
                if self.queue.items.len() <= start {
                    placeholder("Дальше в очереди пока пусто", fade)
                } else {
                    scrollable(column(rows).spacing(4))
                        .height(Length::Fill)
                        .into()
                }
            }
            NowTab::Lyrics => placeholder("Текст песни недоступен в этой версии", fade),
            NowTab::Related => column![
                placeholder("Похожие треки откроются как радио текущего трека", fade),
                icons::text_button(text("Открыть радио").size(14).color(color(theme::LINK)))
                    .on_press(Message::RadioCurrent),
            ]
            .spacing(12)
            .into(),
        };
        let right = column![stage, tabs, body].spacing(16).width(Length::Fill);
        row![cover, right]
            .spacing(32)
            .padding(32)
            .align_y(iced::Alignment::Start)
            .into()
    }

    pub(crate) fn menu_layer(&self, size: iced::Size) -> Element<'_, Message> {
        let Some(menu) = &self.menu else {
            return iced::widget::Space::new().into();
        };
        let menu_w = 280.0;
        let menu_h = 460.0;
        let x = menu.at.x.min((size.width - menu_w - 8.0).max(8.0)).max(8.0);
        let y = menu
            .at
            .y
            .min((size.height - menu_h - 8.0).max(8.0))
            .max(8.0);
        mouse_area(
            container(
                column![
                    iced::widget::Space::new().height(y),
                    row![
                        iced::widget::Space::new().width(x),
                        container(self.context_menu(menu.index)).width(menu_w),
                    ]
                    .spacing(0),
                ]
                .spacing(0),
            )
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .on_press(Message::CloseMenu)
        .into()
    }
}

fn tab<'a>(app: &App, label: &'a str, tab: NowTab, fade: f32) -> Element<'a, Message> {
    let active = app.now_tab == tab;
    let color = if active { TEXT } else { theme::SECONDARY };
    icons::text_button(text(label).size(14).color(color.scale_alpha(fade)))
        .on_press(Message::NowTab(tab))
        .into()
}

fn placeholder<'a>(label: &'a str, fade: f32) -> Element<'a, Message> {
    text(label)
        .size(14)
        .color(theme::SECONDARY.scale_alpha(fade))
        .into()
}

fn button_row<'a>(label: &'a str, fade: f32, message: Message) -> Element<'a, Message> {
    icons::text_button(text(label).size(14).color(TEXT.scale_alpha(fade)))
        .on_press(message)
        .width(Length::Fill)
        .into()
}
