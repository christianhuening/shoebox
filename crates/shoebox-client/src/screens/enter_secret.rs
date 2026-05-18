//! `EnterSecret` screen — user types the shared catalog secret + display
//! name. On submit, `main.rs` runs `enrollment::fetch_ca_cert` then
//! `enrollment::enroll`.

use iced::widget::{button, column, container, row, text, text_input};
use iced::Element;

use crate::discovery::DiscoveredServer;
use crate::screens::Message;

/// `last_error` is the only piece of `AppState` this screen reads; the
/// caller passes it as a short-lived borrow so we don't need to hold a
/// read guard across the Element's lifetime.
#[must_use]
pub fn view<'a>(
    last_error: Option<&'a str>,
    chosen_server: &'a DiscoveredServer,
    secret_draft: &'a str,
    display_name_draft: &'a str,
    ca_cert_loaded: bool,
) -> Element<'a, Message> {
    let header = text(format!("Connect to {}", chosen_server.display_name)).size(24);
    let url_line = text(chosen_server.url.as_str()).size(14);

    let ca_status: Element<Message> = if ca_cert_loaded {
        text("✓ Server CA loaded — your data will be TLS-validated.").into()
    } else {
        text("Fetching server CA…").into()
    };

    let submit_message = Message::SecretSubmitted {
        secret: secret_draft.to_string(),
        display_name: display_name_draft.to_string(),
    };
    let form = column![
        text("Enter the shared catalog secret your admin gave you:"),
        text_input("shared secret", secret_draft)
            .on_input(Message::SecretDraftChanged)
            .on_submit(submit_message.clone()),
        text("Your display name (shown to others on the same catalog):"),
        text_input("display name", display_name_draft)
            .on_input(Message::DisplayNameDraftChanged)
            .on_submit(submit_message.clone()),
        button(text("Enroll")).on_press(submit_message),
    ]
    .spacing(8);

    let error_row: Element<Message> = match last_error {
        Some(message) => row![text("Error: "), text(message)].into(),
        None => row![].into(),
    };

    container(
        column![header, url_line, ca_status, form, error_row]
            .spacing(16)
            .padding(20),
    )
    .into()
}
