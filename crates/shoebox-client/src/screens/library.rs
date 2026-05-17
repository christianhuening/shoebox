//! Library screen — populated in Task 17.

use iced::widget::text;
use iced::Element;

use crate::app_state::AppState;
use crate::screens::Message;

#[must_use]
pub fn view(_state: &AppState) -> Element<'_, Message> {
    text("Library (Task 17)").into()
}
