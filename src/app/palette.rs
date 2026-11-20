use super::*;

impl App {
    /// The palette rows: command names while the name is being typed, then
    /// the command's arguments, so neither has to be memorised.
    pub(crate) fn palette_matches(&self) -> Vec<Suggestion> {
        if self.palette_hidden {
            return Vec::new();
        }
        let commands = matching_commands(&self.input.value);
        if !commands.is_empty() {
            return commands
                .into_iter()
                .map(|command| Suggestion {
                    label: command.display_name(),
                    completion: command.completion(),
                    description: command.description,
                    submit: true,
                })
                .collect();
        }
        argument_suggestions(&self.input.value)
    }

    pub(crate) fn palette_selected(&self) -> usize {
        let count = self.palette_matches().len();
        if count == 0 {
            0
        } else {
            self.palette_index.min(count - 1)
        }
    }

    pub(crate) fn palette_open(&self) -> bool {
        !self.palette_matches().is_empty()
    }

    pub(crate) fn reset_palette(&mut self) {
        self.palette_index = 0;
        self.palette_hidden = false;
    }

    pub(crate) fn palette_move(&mut self, forward: bool) {
        let count = self.palette_matches().len();
        if count == 0 {
            return;
        }
        let current = self.palette_index.min(count - 1);
        self.palette_index = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
    }

    /// Complete the selected row into the composer, keeping the palette closed
    /// until the next keystroke reopens it.
    pub(crate) fn palette_complete(&mut self) -> Option<Suggestion> {
        let matches = self.palette_matches();
        let index = self.palette_index.min(matches.len().saturating_sub(1));
        let suggestion = matches.get(index).cloned()?;
        self.input.set(suggestion.completion.clone());
        self.palette_index = 0;
        self.palette_hidden = true;
        Some(suggestion)
    }

    /// Enter on the palette: a completed command runs, a completed argument
    /// stays in the composer for the rest of the line.
    pub(crate) fn palette_accept(&mut self) -> Action {
        match self.palette_complete() {
            Some(suggestion) if suggestion.submit => self.submit(),
            _ => Action::None,
        }
    }
}
