// SPDX-License-Identifier: MPL-2.0

//! Which lines of the conversation are in view. Drawing lays the
//! conversation out into lines and asks [`Viewport::laid_out`] which of
//! them to show; keys and the wheel scroll through [`Viewport::scroll_by`].
//! Following new messages, holding still while scrolled up, and bringing
//! the selected message into view are decided here, on plain numbers.

use std::ops::Range;

#[derive(Clone, Debug, Default)]
pub struct Viewport {
    /// Lines between the bottom of the view and the newest line; 0
    /// follows new messages.
    scroll: usize,
    /// Lines of the conversation, as of the last layout.
    total: usize,
    /// Lines that fit into the view, as of the last layout.
    height: usize,
    /// The newest message of the last layout, to tell new ones arriving.
    newest: String,
    /// Bring the selected message into view at the next layout.
    reveal: bool,
}

impl Viewport {
    /// Lines between the bottom of the view and the newest line.
    #[cfg(test)]
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Whether the next layout brings the selected message into view.
    #[cfg(test)]
    pub fn wants_reveal(&self) -> bool {
        self.reveal
    }

    /// Whether the newest line is in view.
    pub fn at_bottom(&self) -> bool {
        self.scroll == 0
    }

    /// Follow new messages again.
    pub fn follow_newest(&mut self) {
        self.scroll = 0;
    }

    /// Scroll up (positive) or down. True when that wanted to go past the
    /// oldest line, so older messages should be loaded.
    pub fn scroll_by(&mut self, delta: isize) -> bool {
        let max = self.total.saturating_sub(self.height);
        let wanted = self.scroll as isize + delta;
        self.scroll = wanted.clamp(0, max as isize) as usize;
        wanted > max as isize
    }

    /// One screen, less a line to keep for orientation.
    pub fn page(&self) -> isize {
        self.height.max(2) as isize - 1
    }

    /// Bring the selected message into view at the next layout.
    pub fn reveal_selection(&mut self) {
        self.reveal = true;
    }

    /// The conversation was laid out into `total` lines for a view
    /// `height` lines high; `newest` is its newest message and `selected`
    /// the first and last line of the selected message. Returns the lines
    /// to show.
    pub fn laid_out(
        &mut self,
        total: usize,
        height: usize,
        newest: &str,
        selected: Option<(usize, usize)>,
    ) -> Range<usize> {
        // While scrolled up, keep the view still when new messages arrive
        // below.
        if self.scroll > 0 && newest != self.newest && total > self.total {
            self.scroll += total - self.total;
        }
        newest.clone_into(&mut self.newest);
        self.total = total;
        self.height = height;
        self.scroll = self.scroll.min(total.saturating_sub(height));

        if std::mem::take(&mut self.reveal)
            && let Some((first, last)) = selected
        {
            let end = total - self.scroll;
            let start = end.saturating_sub(height);
            if first < start {
                self.scroll = total.saturating_sub(first + height);
            } else if last >= end {
                self.scroll = total - (last + 1);
            }
        }
        let end = total - self.scroll;
        end.saturating_sub(height)..end
    }

    /// Nothing is laid out (no room open).
    pub fn empty(&mut self, height: usize) {
        self.total = 0;
        self.height = height;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_follows_new_messages_at_the_bottom() {
        let mut view = Viewport::default();
        assert_eq!(view.laid_out(30, 10, "$1", None), 20..30);
        assert_eq!(view.laid_out(32, 10, "$2", None), 22..32);
        assert!(view.at_bottom());
    }

    #[test]
    fn scrolled_up_it_holds_still_when_messages_arrive() {
        let mut view = Viewport::default();
        view.laid_out(30, 10, "$1", None);
        assert!(!view.scroll_by(5));
        assert_eq!(view.laid_out(30, 10, "$1", None), 15..25);
        // Two more lines below: the same lines stay in view.
        assert_eq!(view.laid_out(32, 10, "$2", None), 15..25);
        // A message that grew (not a new one) moves nothing.
        assert_eq!(view.laid_out(33, 10, "$2", None), 16..26);
    }

    #[test]
    fn scrolling_past_the_oldest_line_asks_for_more() {
        let mut view = Viewport::default();
        view.laid_out(30, 10, "$1", None);
        assert!(view.scroll_by(25));
        assert_eq!(view.scroll(), 20);
        assert!(!view.scroll_by(-100));
        assert!(view.at_bottom());
        assert_eq!(view.page(), 9);
    }

    #[test]
    fn the_selected_message_is_brought_into_view_once() {
        let mut view = Viewport::default();
        view.laid_out(30, 10, "$1", None);
        view.reveal_selection();
        // Lines 3 to 5 are above the view: scroll up to show them on top.
        assert_eq!(view.laid_out(30, 10, "$1", Some((3, 5))), 3..13);
        // Not again until asked.
        view.scroll_by(-100);
        assert_eq!(view.laid_out(30, 10, "$1", Some((3, 5))), 20..30);
    }
}
