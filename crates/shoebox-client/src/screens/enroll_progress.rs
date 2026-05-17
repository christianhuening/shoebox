//! `EnrollProgress` screen — spinner during /enroll + `cert_store::store`,
//! plus the keychain-failure consent dialog.

use iced::widget::{button, column, container, row, text};
use iced::Element;

use crate::screens::{Message, Screen};

/// `EnrollProgress` doesn't currently read anything from `AppState`; we
/// drop the parameter to free the caller from holding a read guard
/// across the Element's lifetime.
#[must_use]
pub fn view(current_screen: &Screen) -> Element<'_, Message> {
    match current_screen {
        Screen::EnrollProgress { chosen_server, .. } => container(
            column![
                text(format!("Enrolling with {}…", chosen_server.display_name)).size(20),
                text("This usually takes about a second."),
            ]
            .spacing(12)
            .padding(20),
        )
        .into(),
        Screen::KeychainFailure {
            last_keychain_error,
            ..
        } => container(
            column![
                text("Could not store your cert in the OS keychain").size(20),
                text(format!("Reason: {last_keychain_error}")).size(14),
                text(
                    "You can retry (e.g., unlock the keychain if it was locked), \
                     or use file storage instead. File storage writes the cert + \
                     key to a mode-0600 file in your app-data directory. It works \
                     but isn't as secure as the keychain — anyone with read access \
                     to your home directory could recover the key.",
                ),
                row![
                    button(text("Retry keychain")).on_press(Message::RetryKeychainStore),
                    button(text("Use file storage instead"))
                        .on_press(Message::UseFileStorageInstead),
                ]
                .spacing(12),
            ]
            .spacing(12)
            .padding(20),
        )
        .into(),
        _ => container(text("(invalid screen for enroll_progress::view)"))
            .padding(20)
            .into(),
    }
}

/// Helper for `main.rs::update()` — given an enroll result, attempt to
/// store via keychain. Returns the right next-step Message either way.
///
/// # Errors
/// Returns the keychain error as a String; the caller transitions to
/// `Screen::KeychainFailure` on Err.
pub fn store_via_keychain_or_signal_failure(
    server_url: &str,
    cert_pem: &str,
    key_pem: &str,
) -> Result<(), String> {
    crate::cert_store::store_in_keyring(server_url, cert_pem, key_pem)
        .map_err(|store_err| store_err.to_string())
}

/// Same, for file storage (called when the user picks "Use file storage instead").
///
/// # Errors
/// Returns the filesystem error as a String.
pub fn store_via_file(server_url: &str, cert_pem: &str, key_pem: &str) -> Result<(), String> {
    crate::cert_store::store_in_file(server_url, cert_pem, key_pem)
        .map_err(|store_err| store_err.to_string())
}
