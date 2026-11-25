//! `/stats`: the global token ledger as a full-screen view, closed with Esc.
//!
//! The view is separate from the session on purpose: sessions come and go and
//! tabs hide each other, but "how much have I spent in the last three months"
//! is a property of the machine, not of the conversation on screen.

use super::*;

impl App {
    /// Open the view on a fresh read of `~/.yopes/usage.json`.
    pub(crate) fn open_stats(&mut self) {
        if self.approval.is_some() {
            self.info("Approve or deny the pending tool first.");
            return;
        }
        self.stats = Some(StatsView::open());
        self.palette_hidden = true;
    }

    pub(crate) fn close_stats(&mut self) {
        self.stats = None;
    }

    /// Keys while the view is open: it owns the keyboard until Esc.
    pub(crate) fn handle_stats_key(&mut self, key: KeyEvent) -> Action {
        let Some(view) = self.stats.as_mut() else {
            return Action::None;
        };
        // The close happens after the borrow ends: clearing the view inside the
        // match would free what `view` points at.
        let mut close = false;
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | 'Q') => close = true,
            KeyCode::Up => view.scroll = view.scroll.saturating_sub(1),
            KeyCode::Down => view.scroll = view.scroll.saturating_add(1),
            KeyCode::PageUp => view.scroll = view.scroll.saturating_sub(10),
            KeyCode::PageDown => view.scroll = view.scroll.saturating_add(10),
            KeyCode::Home => view.scroll = 0,
            KeyCode::End => view.scroll = usize::MAX,
            _ => {}
        }
        if close {
            self.close_stats();
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The command opens the view, the arrows scroll it and Esc closes it.
    #[test]
    fn slash_stats_opens_the_view_and_esc_closes_it() {
        let mut app = crate::app::test_app();
        app.command("/stats");
        assert!(app.stats.is_some(), "/stats did not open the view");
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.stats.as_ref().expect("view").scroll, 1);
        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(app.stats.as_ref().expect("view").scroll, 11);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.stats.is_none(), "/stats did not close on esc");
    }
}
