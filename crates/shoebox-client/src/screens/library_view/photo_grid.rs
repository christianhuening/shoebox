//! Center-pane: photo grid as a wrapping row of fixed 256 px tiles.

use iced::widget::image::{Handle, Image};
use iced::widget::{button, column as col, container, row, scrollable, text, Column, Row};
use iced::{Color, Element, Length, Padding};

use crate::library_state::GridCell;
use crate::screens::Message;

const TILE_PX: f32 = 256.0;
const TILE_PAD: f32 = 8.0;

#[must_use]
pub fn view(cells: &[GridCell], selected: Option<usize>) -> Element<'_, Message> {
    if cells.is_empty() {
        return container(text("(no photos)")).padding(20).into();
    }
    let mut grid: Column<Message> = col![].spacing(8);
    let cells_per_row = 4;
    let mut current_row: Row<Message> = row![].spacing(8);
    let mut in_row = 0usize;
    for (index, cell) in cells.iter().enumerate() {
        current_row = current_row.push(tile(cell, Some(index) == selected, index));
        in_row += 1;
        if in_row == cells_per_row {
            grid = grid.push(current_row);
            current_row = row![].spacing(8);
            in_row = 0;
        }
    }
    if in_row > 0 {
        grid = grid.push(current_row);
    }
    scrollable(container(grid).padding(Padding::from(12)))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn tile(cell: &GridCell, selected: bool, index: usize) -> Element<'_, Message> {
    let image: Element<Message> = match &cell.thumbnail {
        Some(image) => {
            let rgba = image.to_rgba8();
            let handle = Handle::from_rgba(rgba.width(), rgba.height(), rgba.into_raw());
            Image::new(handle)
                .width(Length::Fixed(TILE_PX))
                .height(Length::Fixed(TILE_PX))
                .into()
        }
        None => container(text("…"))
            .width(Length::Fixed(TILE_PX))
            .height(Length::Fixed(TILE_PX))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into(),
    };
    let stars = star_row(&cell.variant_id, cell.rating);
    let label = text(&cell.display_name).size(12);
    let inner = col![image, label, stars].spacing(4).padding(TILE_PAD);
    let bg = if selected {
        Color::from_rgb(0.2, 0.5, 0.9)
    } else {
        Color::from_rgb(0.15, 0.15, 0.15)
    };
    button(container(inner).style(move |_| container::Style {
        background: Some(iced::Background::Color(bg)),
        ..container::Style::default()
    }))
    .on_press(Message::LibraryGridCellSelected(index))
    .padding(0)
    .into()
}

fn star_row(variant_id: &str, rating: u8) -> Element<'static, Message> {
    let mut star_row: Row<Message> = row![].spacing(2);
    for star_index in 1u8..=5 {
        let glyph = if star_index <= rating { "★" } else { "☆" };
        let vid = variant_id.to_string();
        star_row = star_row.push(
            button(text(glyph))
                .on_press(Message::LibraryRatingChanged {
                    variant_id: vid,
                    rating: star_index,
                })
                .style(button::text),
        );
    }
    star_row.into()
}
