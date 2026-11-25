//! `/generate`: an image model call whose result is written into the
//! workspace as a PNG, so it can be opened, attached or committed like any
//! other file.

use super::*;

impl App {
    /// `/generate [--model <id>] <prompt>`: ask the image endpoint for one
    /// picture.
    ///
    /// The model is explicit because an image endpoint has no default worth
    /// guessing; without `--model` the built-in name is used.
    pub(crate) fn generate_image(&mut self, request: &str) -> Action {
        let request = request.trim();
        if request.is_empty() {
            self.info(format!(
                "Usage: /generate <prompt> · model {} · saved as image-<stamp>.png in the workspace",
                crate::image_api::DEFAULT_MODEL
            ));
            return Action::None;
        }
        let (model, prompt) = match request.strip_prefix("--model ") {
            Some(rest) => match rest.trim().split_once(char::is_whitespace) {
                Some((model, prompt)) => (model.to_owned(), prompt.trim().to_owned()),
                None => {
                    self.error("Usage: /generate --model <id> <prompt>");
                    return Action::None;
                }
            },
            None => (
                crate::image_api::DEFAULT_MODEL.to_owned(),
                request.to_owned(),
            ),
        };
        if prompt.is_empty() {
            self.error("Usage: /generate --model <id> <prompt>");
            return Action::None;
        }
        let base_url = self.backend.base_url().trim().to_owned();
        if base_url.is_empty() {
            self.error("Connect a provider first: /generate needs an endpoint.");
            return Action::None;
        }
        if self.generating_image {
            self.info("An image request is already running.");
            return Action::None;
        }
        self.generating_image = true;
        self.info(format!("Generating an image with {model}…"));
        Action::GenerateImage {
            base_url,
            auth: self.backend.auth(),
            key: self.backend.api_key().map(str::to_owned),
            model,
            prompt,
        }
    }

    /// The image came back: write it beside the code and say where.
    pub(crate) fn on_generated_image(&mut self, result: Result<Vec<u8>, String>) {
        self.generating_image = false;
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(error) => {
                self.error(format!("Image generation failed: {error}"));
                return;
            }
        };
        let dir = self.working_dir().to_path_buf();
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let name = crate::image_api::file_name(&dir, &stamp);
        let path = dir.join(&name);
        match std::fs::write(&path, &bytes) {
            Ok(()) => self.info(format!(
                "Image saved to {} · {} KB · @{name} attaches it",
                short_path(&path, &dir),
                bytes.len().div_ceil(1024)
            )),
            Err(error) => self.error(format!("Could not save the image: {error}")),
        }
    }
}

/// The image path relative to the workspace, which is how the user thinks of
/// it; a path outside it is shown whole.
fn short_path(path: &std::path::Path, dir: &std::path::Path) -> String {
    path.strip_prefix(dir)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_asks_for_the_default_image_model() {
        let mut app = crate::app::test_app();
        match app.command("/generate a red bicycle") {
            Action::GenerateImage {
                base_url,
                model,
                prompt,
                ..
            } => {
                assert_eq!(model, crate::image_api::DEFAULT_MODEL);
                assert_eq!(prompt, "a red bicycle");
                assert!(base_url.contains("://"), "{base_url}");
            }
            _ => panic!("no image request"),
        }
        assert!(app.generating_image);
        // A second request while one is running is refused, not queued.
        app.command("/generate another");
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|notice| notice.message.contains("already running"))
        );
    }

    #[test]
    fn an_explicit_model_and_an_empty_prompt() {
        let mut app = crate::app::test_app();
        match app.command("/generate --model flux-1 a cat") {
            Action::GenerateImage { model, prompt, .. } => {
                assert_eq!(model, "flux-1");
                assert_eq!(prompt, "a cat");
            }
            _ => panic!("no image request"),
        }
        let mut app = crate::app::test_app();
        assert!(matches!(app.command("/generate"), Action::None));
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|notice| notice.message.contains("Usage: /generate"))
        );
    }

    #[test]
    fn the_written_image_lands_in_the_workspace() {
        let dir = crate::temp::unique_dir("yopes-generate-write");
        let mut app = crate::app::test_app();
        app.working_dir = dir.clone();
        app.generating_image = true;
        app.on_generated_image(Ok(b"\x89PNG\r\n".to_vec()));
        assert!(!app.generating_image);
        let saved: Vec<_> = std::fs::read_dir(&dir)
            .expect("workspace")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(saved.len(), 1, "{saved:?}");
        assert!(saved[0].starts_with("image-"), "{saved:?}");
        assert!(saved[0].ends_with(".png"), "{saved:?}");
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|notice| notice.message.contains("Image saved to"))
        );

        // A failure is reported, not written.
        app.on_generated_image(Err("boom".to_owned()));
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|notice| notice.message.contains("failed"))
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
