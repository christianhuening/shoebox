//! Right-pane: EXIF + rating + keyword editor + virtual-copy button +
//! lock-status banner.

use iced::widget::{
    button, column as col, container, row, scrollable, text, text_input, Row,
};
use iced::{Element, Length};

use crate::library_state::{DetailLoaded, LockStatus};
use crate::screens::Message;

#[must_use]
pub fn view<'a>(
    detail: Option<&'a DetailLoaded>,
    lock_status: &'a LockStatus,
    keyword_input: &'a str,
) -> Element<'a, Message> {
    let body: Element<Message> = match detail {
        None => text("Select a photo to see details.").into(),
        Some(detail) => detail_body(detail, lock_status, keyword_input),
    };
    container(scrollable(body).height(Length::Fill))
        .width(Length::Fixed(320.0))
        .height(Length::Fill)
        .padding(12)
        .into()
}

fn detail_body<'a>(
    detail: &'a DetailLoaded,
    lock_status: &'a LockStatus,
    keyword_input: &'a str,
) -> Element<'a, Message> {
    let exif = &detail.exif;

    let camera_line = format!(
        "{} {}",
        exif.camera_make.clone().unwrap_or_default(),
        exif.camera_model.clone().unwrap_or_default()
    );
    let lens_line = exif.lens.clone().unwrap_or_else(|| "—".into());
    let dimensions = match (exif.width_px, exif.height_px) {
        (Some(width), Some(height)) => format!("{width}×{height}"),
        _ => "—".into(),
    };
    #[allow(clippy::cast_precision_loss)]
    let shutter = exif
        .shutter_us
        .map_or_else(|| "—".into(), |us| format!("1/{:.0}s", 1_000_000.0 / us as f64));
    let aperture = exif
        .aperture
        .map_or_else(|| "—".into(), |f| format!("f/{f:.1}"));
    let iso = exif
        .iso
        .map_or_else(|| "—".into(), |n| n.to_string());
    let focal = exif
        .focal_length_mm
        .map_or_else(|| "—".into(), |f| format!("{f:.0}mm"));

    let exif_block = col![
        text("EXIF").size(16),
        text(camera_line),
        text(format!("Lens: {lens_line}")),
        text(format!("Pixels: {dimensions}")),
        text(format!("{aperture} · {shutter} · ISO {iso} · {focal}")),
    ]
    .spacing(2);

    let stars = stars_for(detail.rating);
    let mut keyword_row: Row<Message> = row![].spacing(4);
    for keyword in &detail.keywords {
        let kid = keyword.id.clone();
        keyword_row = keyword_row.push(
            button(text(format!("{} ×", keyword.name)))
                .on_press(Message::LibraryKeywordRemoveClicked { keyword_id: kid }),
        );
    }
    let keyword_input_row = row![
        text_input("add keyword…", keyword_input)
            .on_input(Message::LibraryKeywordInputChanged)
            .on_submit(Message::LibraryKeywordSubmitted),
        button(text("Add")).on_press(Message::LibraryKeywordSubmitted),
    ]
    .spacing(6);

    let lock_block = lock_banner(lock_status);
    let virtual_copy_button = button(text("New virtual copy"))
        .on_press(Message::LibraryNewVirtualCopyClicked);

    col![
        exif_block,
        text("Rating").size(16),
        stars,
        text("Keywords").size(16),
        keyword_row,
        keyword_input_row,
        text("Variants").size(16),
        virtual_copy_button,
        text("Lock").size(16),
        lock_block,
    ]
    .spacing(10)
    .into()
}

fn stars_for(rating: u8) -> Element<'static, Message> {
    let mut star_row: Row<Message> = row![].spacing(2);
    for star_index in 0u8..=5 {
        let glyph = if star_index == 0 {
            "—".to_string()
        } else if star_index <= rating {
            "★".to_string()
        } else {
            "☆".to_string()
        };
        star_row = star_row.push(
            button(text(glyph))
                .on_press(Message::LibraryKeyboardRating(star_index))
                .style(button::text),
        );
    }
    star_row.into()
}

fn lock_banner(status: &LockStatus) -> Element<'_, Message> {
    match status {
        LockStatus::Free => row![
            text("No lock — anyone can edit"),
            button(text("Acquire")).on_press(Message::LibraryAcquireLockClicked),
        ]
        .spacing(8)
        .into(),
        LockStatus::HeldByYou => row![
            text("You hold the lock."),
            button(text("Release")).on_press(Message::LibraryReleaseLockClicked),
        ]
        .spacing(8)
        .into(),
        LockStatus::HeldByYouTakeoverPending {
            requested_by_display_name,
        } => col![
            text(format!(
                "{requested_by_display_name} requested takeover of your lock"
            )),
            row![
                button(text("Release")).on_press(Message::LibraryReleaseLockClicked),
            ]
            .spacing(8),
        ]
        .spacing(4)
        .into(),
        LockStatus::HeldByOther { holder_display_name } => row![
            text(format!("Held by {holder_display_name}")),
            button(text("Request takeover"))
                .on_press(Message::LibraryRequestTakeoverClicked),
        ]
        .spacing(8)
        .into(),
        LockStatus::HeldByOtherTakeoverPending { holder_display_name } => {
            text(format!(
                "Waiting on {holder_display_name} to release the lock…"
            ))
            .into()
        }
    }
}
