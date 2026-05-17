//! Discovery screen: mDNS list + manual entry + retry.

use iced::widget::{button, column, container, row, text, text_input};
use iced::{Element, Length};

use crate::app_state::AppState;
use crate::discovery::DiscoveredServer;
use crate::screens::Message;

/// View state lives in `AppState` (the discovered-servers list is
/// accumulated by `update()` from `Message::ServerDiscovered`). This
/// module is pure `view`.
#[must_use]
pub fn view<'a>(
    state: &'a AppState,
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

    let manual_form = column![
        text("Or add manually:").size(18),
        text_input("Display name", manual_name_draft).on_input(|new_name| {
            Message::ManualUrlSubmitted {
                display_name: new_name,
                url: manual_url_draft.to_string(),
            }
        }),
        text_input("https://host:9000", manual_url_draft).on_input(|new_url| {
            Message::ManualUrlSubmitted {
                display_name: manual_name_draft.to_string(),
                url: new_url,
            }
        }),
        button(text("Add this server")).on_press(Message::ManualUrlSubmitted {
            display_name: manual_name_draft.to_string(),
            url: manual_url_draft.to_string(),
        }),
    ]
    .spacing(6);

    let retry_button = button(text("Retry discovery")).on_press(Message::DiscoveryRetry);

    let error_row: Element<Message> = match state.last_error.as_deref() {
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
