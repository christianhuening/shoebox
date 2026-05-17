//! Left-pane: scrollable folder tree.

use iced::widget::{button, column, container, scrollable, text};
use iced::{Element, Length};

use crate::library_state::FolderRow;
use crate::screens::Message;

#[must_use]
pub fn view<'a>(
    rows: &'a [FolderRow],
    selected: Option<&'a str>,
) -> Element<'a, Message> {
    let mut column_widget = column![text("Folders").size(18)].spacing(2).padding(8);
    if rows.is_empty() {
        column_widget = column_widget.push(text("(empty)"));
    }
    for row in rows {
        let indent = "  ".repeat(row.depth);
        let label = format!("{indent}{}", row.name);
        let is_selected = selected == Some(row.id.as_str());
        let style = if is_selected {
            button::primary
        } else {
            button::text
        };
        column_widget = column_widget.push(
            button(text(label))
                .on_press(Message::LibraryFolderSelected(row.id.clone()))
                .style(style)
                .width(Length::Fill),
        );
    }
    container(scrollable(column_widget).height(Length::Fill))
        .width(Length::Fixed(220.0))
        .height(Length::Fill)
        .into()
}
