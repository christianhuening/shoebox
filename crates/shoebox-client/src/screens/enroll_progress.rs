//! `EnrollProgress` screen — populated in Task 15.

use iced::widget::text;
use iced::Element;

use crate::app_state::AppState;
use crate::screens::Message;

#[must_use]
pub fn view(_state: &AppState) -> Element<'_, Message> {
    text("EnrollProgress (Task 15)").into()
}
