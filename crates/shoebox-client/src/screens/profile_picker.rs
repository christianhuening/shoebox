//! `ProfilePicker` screen — populated in Task 16.

use iced::widget::text;
use iced::Element;

use crate::app_state::AppState;
use crate::screens::Message;

#[must_use]
pub fn view(_state: &AppState) -> Element<'_, Message> {
    text("ProfilePicker (Task 16)").into()
}
