//! `EnterSecret` screen — populated in Task 14.

use iced::widget::text;
use iced::Element;

use crate::app_state::AppState;
use crate::screens::Message;

#[must_use]
pub fn view(_state: &AppState) -> Element<'_, Message> {
    text("EnterSecret (Task 14)").into()
}
