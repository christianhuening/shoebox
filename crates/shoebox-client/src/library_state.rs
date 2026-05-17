//! Domain types + async DB helpers + mutation helpers + pure
//! keyboard-navigation helpers for the demo library view (Plan 1.4b).
//!
//! All reads/writes use a fresh `libsql::Connection` per call (cheap;
//! the underlying `Database` is shared).

#[allow(unused_imports)]
use anyhow::{Context, Result};
use std::sync::Arc;

use crate::thumb_cache::ThumbCache;

/// One row in the folder-tree pane. `depth` is the indentation level
/// (0 = root). Computed during tree-flattening, not stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRow {
    pub id: String,
    pub name: String,
    pub depth: usize,
}

/// One cell in the photo grid. `display_name` is the filename for masters
/// and `"<filename> (<variant_index + 1>)"` for virtual copies.
#[derive(Debug, Clone, PartialEq)]
pub struct GridCell {
    pub variant_id: String,
    pub photo_id: String,
    pub variant_index: i64,
    pub display_name: String,
    pub rating: u8,
    pub thumbnail: Option<Arc<image::DynamicImage>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExifSummary {
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<i64>,
    pub aperture: Option<f64>,
    pub shutter_us: Option<i64>,
    pub focal_length_mm: Option<f64>,
    pub width_px: Option<i64>,
    pub height_px: Option<i64>,
    pub captured_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct KeywordRow {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct DetailLoaded {
    pub variant_id: String,
    pub photo_id: String,
    pub exif: ExifSummary,
    pub rating: u8,
    pub keywords: Vec<KeywordRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LockStatus {
    #[default]
    Free,
    HeldByYou,
    HeldByYouTakeoverPending { requested_by_display_name: String },
    HeldByOther { holder_display_name: String },
    HeldByOtherTakeoverPending { holder_display_name: String },
}

#[derive(Debug, Clone, Default)]
pub struct LibraryViewState {
    pub folder_tree: Vec<FolderRow>,
    pub selected_folder_id: Option<String>,
    pub grid: Vec<GridCell>,
    pub selected_grid_index: Option<usize>,
    pub detail: Option<DetailLoaded>,
    pub lock_status: LockStatus,
    pub error: Option<String>,
    pub cells_per_row: usize,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct LibraryThumbBundle {
    pub cache: Option<ThumbCache>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationDirection {
    Left,
    Right,
    Up,
    Down,
}

#[must_use]
pub fn advance_selection(
    current: Option<usize>,
    total: usize,
    cells_per_row: usize,
    direction: NavigationDirection,
) -> Option<usize> {
    if total == 0 {
        return None;
    }
    let cells_per_row = cells_per_row.max(1);
    let current = current.unwrap_or(0).min(total - 1);
    let next = match direction {
        NavigationDirection::Left => current.saturating_sub(1),
        NavigationDirection::Right => (current + 1).min(total - 1),
        NavigationDirection::Up => current.saturating_sub(cells_per_row),
        NavigationDirection::Down => (current + cells_per_row).min(total - 1),
    };
    Some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advance_selection_right_increments() {
        assert_eq!(
            advance_selection(Some(5), 10, 4, NavigationDirection::Right),
            Some(6)
        );
    }

    #[test]
    fn advance_selection_left_saturates_at_zero() {
        assert_eq!(
            advance_selection(Some(0), 10, 4, NavigationDirection::Left),
            Some(0)
        );
    }

    #[test]
    fn advance_selection_down_jumps_one_row() {
        assert_eq!(
            advance_selection(Some(2), 12, 4, NavigationDirection::Down),
            Some(6)
        );
    }

    #[test]
    fn advance_selection_up_saturates_at_first_row() {
        assert_eq!(
            advance_selection(Some(1), 12, 4, NavigationDirection::Up),
            Some(0)
        );
    }

    #[test]
    fn advance_selection_right_at_end_stays() {
        assert_eq!(
            advance_selection(Some(9), 10, 4, NavigationDirection::Right),
            Some(9)
        );
    }

    #[test]
    fn advance_selection_with_empty_grid_returns_none() {
        assert_eq!(
            advance_selection(Some(0), 0, 4, NavigationDirection::Right),
            None
        );
    }
}
