//! `ProfilePicker` screen — list existing users from the local replica,
//! or let the user create a new one.

use iced::widget::{button, column, container, row, text, text_input};
use iced::Element;

use crate::screens::{Message, UserRow};

/// `last_error` is the only field this screen reads from `AppState`;
/// the caller passes it as a short-lived borrow.
#[must_use]
pub fn view<'a>(
    last_error: Option<&'a str>,
    existing_users: &'a [UserRow],
    new_user_draft: &'a str,
) -> Element<'a, Message> {
    let header = text("Who are you?").size(24);

    let user_list: Element<Message> = if existing_users.is_empty() {
        text("(no users yet — create one below)").into()
    } else {
        let mut list_column = column![text("Pick an existing profile:").size(16)].spacing(6);
        for existing_user in existing_users {
            let pick_button = button(text(existing_user.display_name.as_str()))
                .on_press(Message::UserPicked(existing_user.id.clone()));
            list_column = list_column.push(pick_button);
        }
        list_column.into()
    };

    let new_user_form = column![
        text("Or create a new profile:").size(16),
        text_input("display name", new_user_draft).on_input(|updated_name| {
            Message::CreateUserSubmitted {
                display_name: updated_name,
            }
        }),
        button(text("Create")).on_press(Message::CreateUserSubmitted {
            display_name: new_user_draft.to_string(),
        }),
    ]
    .spacing(6);

    let error_row: Element<Message> = match last_error {
        Some(message) => row![text("Error: "), text(message)].into(),
        None => row![].into(),
    };

    container(
        column![header, user_list, new_user_form, error_row]
            .spacing(16)
            .padding(20),
    )
    .into()
}

/// Helper for `main.rs::update()` — runs `SELECT id, display_name FROM users`
/// on a libsql `Connection`.
///
/// # Errors
/// Returns an error on query failure.
pub async fn load_users(conn: &libsql::Connection) -> Result<Vec<UserRow>, anyhow::Error> {
    let mut rows = conn.query("SELECT id, display_name FROM users", ()).await?;
    let mut users = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        let display_name: String = row.get(1)?;
        users.push(UserRow { id, display_name });
    }
    Ok(users)
}

/// Helper for `main.rs::update()` — inserts a new `users` row with a
/// freshly-generated UUID-like id and returns the inserted row.
///
/// # Errors
/// Returns an error on insert failure.
pub async fn create_user(
    conn: &libsql::Connection,
    display_name: &str,
) -> Result<UserRow, anyhow::Error> {
    use rand::RngCore;
    let mut id_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut id_bytes);
    let new_id = hex::encode(id_bytes);
    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis()),
    )
    .unwrap_or(0);
    conn.execute(
        "INSERT INTO users (id, display_name, created_at, last_seen_at) VALUES (?1, ?2, ?3, ?3)",
        (new_id.clone(), display_name.to_string(), now_ms),
    )
    .await?;
    Ok(UserRow {
        id: new_id,
        display_name: display_name.to_string(),
    })
}
