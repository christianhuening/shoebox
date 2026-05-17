//! Composes the three demo-library panes into one screen and exposes
//! the keyboard subscription that turns arrow keys + 0-5 into Messages.

pub mod detail_panel;
pub mod folder_tree;
pub mod photo_grid;

use iced::keyboard::{key::Named, Key};
use iced::widget::{column as col, container, row, text};
use iced::{Element, Length, Subscription};

use crate::app_state::AppState;
use crate::library_state::NavigationDirection;
use crate::screens::Message;

#[must_use]
pub fn view(state: &AppState) -> Element<'_, Message> {
    let panes = row![
        folder_tree::view(
            &state.library_view.folder_tree,
            state.library_view.selected_folder_id.as_deref(),
        ),
        photo_grid::view(
            &state.library_view.grid,
            state.library_view.selected_grid_index,
        ),
        detail_panel::view(
            state.library_view.detail.as_ref(),
            &state.library_view.lock_status,
            &state.library_view.keyword_input,
        ),
    ]
    .height(Length::Fill);

    let error_banner: Element<Message> = match &state.library_view.error {
        Some(message) => text(format!("⚠ {message}")).into(),
        None => text("").into(),
    };

    container(col![error_banner, panes].spacing(4))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

pub fn keyboard_subscription() -> Subscription<Message> {
    iced::keyboard::on_key_press(|key, _modifiers| match key {
        Key::Named(Named::ArrowLeft) => Some(Message::LibraryKeyboardNavigation(
            NavigationDirection::Left,
        )),
        Key::Named(Named::ArrowRight) => Some(Message::LibraryKeyboardNavigation(
            NavigationDirection::Right,
        )),
        Key::Named(Named::ArrowUp) => {
            Some(Message::LibraryKeyboardNavigation(NavigationDirection::Up))
        }
        Key::Named(Named::ArrowDown) => Some(Message::LibraryKeyboardNavigation(
            NavigationDirection::Down,
        )),
        Key::Character(c) => match c.as_str() {
            "0" => Some(Message::LibraryKeyboardRating(0)),
            "1" => Some(Message::LibraryKeyboardRating(1)),
            "2" => Some(Message::LibraryKeyboardRating(2)),
            "3" => Some(Message::LibraryKeyboardRating(3)),
            "4" => Some(Message::LibraryKeyboardRating(4)),
            "5" => Some(Message::LibraryKeyboardRating(5)),
            _ => None,
        },
        _ => None,
    })
}
