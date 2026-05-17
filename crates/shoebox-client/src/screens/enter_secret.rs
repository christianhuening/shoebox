//! `EnterSecret` screen — user types the shared catalog secret + display
//! name. On submit, `main.rs` runs `enrollment::fetch_ca_cert` then
//! `enrollment::enroll`.

use iced::widget::{button, column, container, row, text, text_input};
use iced::Element;

use crate::app_state::AppState;
use crate::discovery::DiscoveredServer;
use crate::screens::Message;

#[must_use]
pub fn view<'a>(
    state: &'a AppState,
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

    let form = column![
        text("Enter the shared catalog secret your admin gave you:"),
        text_input("shared secret", secret_draft).on_input(|updated_secret| {
            Message::SecretSubmitted {
                secret: updated_secret,
                display_name: display_name_draft.to_string(),
            }
        }),
        text("Your display name (shown to others on the same catalog):"),
        text_input("display name", display_name_draft).on_input(|updated_name| {
            Message::SecretSubmitted {
                secret: secret_draft.to_string(),
                display_name: updated_name,
            }
        }),
        button(text("Enroll")).on_press(Message::SecretSubmitted {
            secret: secret_draft.to_string(),
            display_name: display_name_draft.to_string(),
        }),
    ]
    .spacing(8);

    let error_row: Element<Message> = match state.last_error.as_deref() {
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
