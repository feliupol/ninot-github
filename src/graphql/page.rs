//! Forward pagination over a complete collection.

use super::objects::{Connection, PageInfo};
use super::{cursor_index, cursor_of};

/// One page of `items` using Relay `first` / `after` (and `last` as a page size).
pub(super) fn page<T>(
    items: Vec<T>,
    first: Option<i64>,
    last: Option<i64>,
    after: Option<String>,
) -> Connection<T> {
    let total = items.len();
    let start = after
        .as_deref()
        .and_then(cursor_index)
        .map_or(0, |index| index + 1);
    let count = first.or(last).unwrap_or(30).clamp(0, 100) as usize;
    let end = (start + count).min(total);
    let nodes = items
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect();
    Connection {
        nodes,
        total_count: total,
        page_info: PageInfo {
            has_next_page: end < total,
            has_previous_page: start > 0,
            end_cursor: (end > start).then(|| cursor_of(end - 1)),
            start_cursor: (end > start).then(|| cursor_of(start)),
        },
    }
}

/// One page of `items`. `last` without `first` takes the last N items.
pub(super) fn page_window<T>(
    items: Vec<T>,
    first: Option<i64>,
    last: Option<i64>,
    after: Option<String>,
) -> Connection<T> {
    if first.is_none()
        && let Some(last) = last
    {
        let total = items.len();
        let count = last.clamp(0, 100) as usize;
        let end = total;
        let start = end.saturating_sub(count);
        let nodes = items.into_iter().skip(start).take(count).collect();
        return Connection {
            nodes,
            total_count: total,
            page_info: PageInfo {
                has_next_page: false,
                has_previous_page: start > 0,
                end_cursor: (end > start).then(|| cursor_of(end - 1)),
                start_cursor: (end > start).then(|| cursor_of(start)),
            },
        };
    }
    page(items, first, last, after)
}
