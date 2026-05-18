//! Discovery screen: mDNS list + manual entry + retry.

use iced::widget::{button, column, container, row, text, text_input};
use iced::{Element, Length};

use crate::discovery::DiscoveredServer;
use crate::screens::Message;

/// View state lives in `AppState` (the discovered-servers list is
/// accumulated by `update()` from `Message::ServerDiscovered`). This
/// module is pure `view`.
///
/// `last_error` is the only field this screen needs from `AppState`;
/// the caller passes it as a short-lived borrow so the screen module
/// doesn't have to take a borrow of the whole `AppState` (which would
/// force the caller to hold a read guard across the Element's lifetime).
#[must_use]
pub fn view<'a>(
    last_error: Option<&'a str>,
    discovered_servers: &'a [DiscoveredServer],
    manual_url_draft: &'a str,
    manual_name_draft: &'a str,
) -> Element<'a, Message> {
    let header = text("Pick a shoebox server").size(28);

    let server_list: Element<Message> = if discovered_servers.is_empty() {
        text("(no servers found yet \u{2014} try Retry or add one manually)").into()
    } else {
        let mut list_column = column![].spacing(8);
        for server in discovered_servers {
            let pick_button = button(text(format!(
                "{}  \u{2014}  {}",
                server.display_name, server.url
            )))
            .width(Length::Fill)
            .on_press(Message::ServerPicked(server.clone()));
            list_column = list_column.push(pick_button);
        }
        list_column.into()
    };

    let submit_message = Message::ManualUrlSubmitted {
        display_name: manual_name_draft.to_string(),
        url: manual_url_draft.to_string(),
    };
    let manual_form = column![
        text("Or add manually:").size(18),
        text_input("Display name", manual_name_draft)
            .on_input(Message::ManualNameDraftChanged)
            .on_submit(submit_message.clone()),
        text_input("https://host:9000", manual_url_draft)
            .on_input(Message::ManualUrlDraftChanged)
            .on_submit(submit_message.clone()),
        button(text("Add this server")).on_press(submit_message),
    ]
    .spacing(6);

    let retry_button = button(text("Retry discovery")).on_press(Message::DiscoveryRetry);

    let error_row: Element<Message> = match last_error {
        Some(message) => row![text("Error: "), text(message)].into(),
        None => row![].into(),
    };

    container(
        column![header, server_list, retry_button, manual_form, error_row]
            .spacing(16)
            .padding(20),
    )
    .into()
}

/// Helper consumed by `main.rs::update()` to drop a new entry into the
/// running list (deduped by URL).
pub fn merge_discovered(existing: &mut Vec<DiscoveredServer>, new_entry: DiscoveredServer) {
    if existing.iter().any(|server| server.url == new_entry.url) {
        return;
    }
    existing.push(new_entry);
}
