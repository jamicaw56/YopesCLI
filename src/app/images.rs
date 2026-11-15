use super::*;

/// The longest edge sent to a vision model, in pixels: larger images are
/// downscaled before encoding, so one file cannot dominate the context.
const MAX_IMAGE_DIMENSION: u32 = 1568;

/// Image file extensions the bundled decoders actually support: anything
/// else falls through to the composer as plain text.
const IMAGE_EXTENSIONS: [&str; 6] = [".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp"];

/// The file path of a pasted image, if it points at a readable image file:
/// trims whitespace and quotes, understands `file://` URIs with percent
/// escapes and Windows drive letters, and tolerates shell-escaped spaces.
pub(crate) fn pasted_image_path(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_matches('"').trim_matches('\'');
    let candidate = match trimmed.strip_prefix("file://") {
        Some(rest) => {
            let rest = rest.strip_prefix("localhost").unwrap_or(rest);
            // `file:///C:/path` keeps the drive letter without the leading slash.
            let rest = if rest.starts_with('/')
                && rest.len() > 2
                && rest.as_bytes()[1].is_ascii_alphabetic()
                && rest.as_bytes()[2] == b':'
            {
                &rest[1..]
            } else {
                rest
            };
            percent_decode(rest)
        }
        None => trimmed.to_owned(),
    };
    for path in [candidate.clone(), candidate.replace("\\ ", " ")] {
        let lower = path.to_ascii_lowercase();
        if IMAGE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
            && std::path::Path::new(&path).is_file()
        {
            return Some(path);
        }
    }
    None
}

/// Decode `%XX` escapes in a `file://` URI path.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16)
        {
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read the pixels of a `read_image` result. The tool's text result cannot
/// carry binary data, so the app attaches the PNG to the tool message itself.
pub(super) fn tool_image_attachment(call: &ToolCall) -> Option<ImageAttachment> {
    let value: serde_json::Value = serde_json::from_str(&call.function.arguments).ok()?;
    let path = value.get("path")?.as_str()?.trim();
    if path.is_empty() {
        return None;
    }
    let path = std::path::Path::new(path);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let mut image = image::open(&resolved).ok()?;
    if image.width() > MAX_IMAGE_DIMENSION || image.height() > MAX_IMAGE_DIMENSION {
        image = image.resize(
            MAX_IMAGE_DIMENSION,
            MAX_IMAGE_DIMENSION,
            image::imageops::FilterType::Triangle,
        );
    }
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .ok()?;
    let name = resolved.file_name().map_or_else(
        || "image.png".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    Some(ImageAttachment {
        name,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// Half-block pixel art of a pasted image: each TUI row shows two pixel
/// lines at once, through `▄` glyphs with per-span foreground and background.
pub(super) fn build_image_rows(encoded: &str, cols: u16) -> Vec<Line<'static>> {
    let unreadable = || {
        Line::styled(
            " (image could not be shown)",
            Style::default().fg(Color::DarkGray),
        )
    };
    let decoded = match base64::engine::general_purpose::STANDARD.decode(encoded.trim()) {
        Ok(bytes) => bytes,
        Err(_) => return vec![unreadable()],
    };
    let rendered = match image::load_from_memory(&decoded) {
        Ok(image) => image.into_rgba8(),
        Err(_) => return vec![unreadable()],
    };
    let cols = usize::from(cols).max(1);
    let (width, height) = (rendered.width().max(1), rendered.height().max(1));
    // Cells are twice as wide as tall, and one `▄` carries two pixel rows,
    // so two pixels per cell in both directions keep the aspect square.
    let row_pairs = ((u64::from(height) * cols as u64) / u64::from(width) / 2).clamp(1, 18) as u32;
    let scaled = image::imageops::thumbnail(&rendered, (cols * 2) as u32, row_pairs * 2);
    let row_pairs = scaled.height().div_ceil(2);
    let mut rows = Vec::with_capacity(row_pairs as usize);
    for pair in 0..row_pairs {
        let top_y = (pair * 2).min(scaled.height() - 1);
        let bottom_y = (top_y + 1).min(scaled.height() - 1);
        let mut spans = Vec::with_capacity(cols);
        for x in 0..scaled.width() {
            spans.push(Span::styled(
                "▄",
                Style::default()
                    .fg(pixel_color(scaled.get_pixel(x, bottom_y)))
                    .bg(pixel_color(scaled.get_pixel(x, top_y))),
            ));
        }
        rows.push(Line::from(spans));
    }
    rows
}

pub(super) fn pixel_color(pixel: &image::Rgba<u8>) -> Color {
    let [red, green, blue, _] = pixel.0;
    Color::Rgb(red, green, blue)
}

impl App {
    /// A clipboard read arrived: queue it as an image or insert its text.
    pub(crate) fn on_paste_image(&mut self, result: Result<(String, String), String>) -> Action {
        match result {
            Ok((kind, payload)) => {
                if kind == "text" {
                    // The same route as a bracketed paste keeps the caret,
                    // connect fields and help overlay behaving the same.
                    self.handle_paste(&payload);
                } else {
                    self.attach_pasted_image_data(None, payload);
                }
            }
            Err(error) => self.error(error),
        }
        Action::None
    }

    /// Queue a pasted image: the composer shows a `[Image N]` placeholder,
    /// the real path stays hidden in the map for the swap-back at submit.
    /// The number counts per prompt: `attachments` drain on send, so the
    /// next paste is `[Image 1]` again.
    pub(crate) fn attach_pasted_image_data(&mut self, path: Option<String>, data: String) {
        let attachment = format!("paste-{}.png", self.next_image);
        self.next_image += 1;
        let placeholder = format!("[Image {}]", self.attachments.len() + 1);
        self.attachments.push(ImageAttachment {
            name: attachment.clone(),
            data,
        });
        self.image_paths
            .insert(placeholder.clone(), PastedImage { path, attachment });
        self.input.insert_str(&placeholder);
        self.info(format!(
            "{placeholder} attached — send it with the next message."
        ));
    }

    /// Swap `[Image N]` placeholders back to their real paths on submit: the
    /// model and the transcript see the path, the composer never did.
    pub(crate) fn resolve_image_placeholders(&mut self, mut value: String) -> String {
        for (placeholder, pasted) in self.image_paths.drain() {
            let replacement = pasted.path.as_deref().unwrap_or_default();
            value = value.replace(placeholder.as_str(), replacement);
        }
        value
    }

    /// Backspace over a whole `[Image N]` token: drops it, its map entry, and
    /// the queued attachment. `false` when no token sits before the cursor.
    pub(crate) fn remove_pasted_image_token(&mut self) -> bool {
        if self.input.selection().is_some() {
            return false;
        }
        let Some(placeholder) = self
            .image_paths
            .keys()
            .find(|placeholder| self.input.before_cursor().ends_with(placeholder.as_str()))
            .cloned()
        else {
            return false;
        };
        let end = self.input.cursor;
        let start = end - placeholder.len();
        self.input.value.replace_range(start..end, "");
        self.input.cursor = start;
        if let Some(pasted) = self.image_paths.remove(&placeholder) {
            self.attachments
                .retain(|image| image.name != pasted.attachment);
        }
        true
    }

    /// Drop every pending pasted image: the composer was cleared wholesale.
    pub(crate) fn discard_pasted_images(&mut self) {
        self.image_paths.clear();
        self.attachments.clear();
    }

    /// Render any not-yet-cached pasted image for a terminal width.
    ///
    /// The transcript loop only reads the session, so rows are prepared here
    /// beforehand and the loop then extends its lines from this cache.
    pub(crate) fn warm_image_cache(&mut self, width: u16) {
        if self.image_width != width {
            self.image_rows.clear();
            self.image_width = width;
        }
        if self.image_rows.len() > 40 {
            self.image_rows.clear();
        }
        let mut rows = std::mem::take(&mut self.image_rows);
        for message in &self.session.messages {
            for image in &message.images {
                let key = format!("{}:{width}", image.name);
                rows.entry(key)
                    .or_insert_with(|| build_image_rows(&image.data, width));
            }
        }
        self.image_rows = rows;
    }

    /// The cached half-block rows of one pasted image, if rendered for the
    /// current terminal width.
    pub(crate) fn cached_image(&self, name: &str) -> Option<&[Line<'static>]> {
        self.image_rows
            .get(&format!("{name}:{}", self.image_width))
            .map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::build_image_rows;
    use base64::Engine as _;

    fn tiny_png() -> Vec<u8> {
        let png = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(2, 2, |_, _| {
            image::Rgb([255u8, 0, 0])
        }));
        let mut bytes = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut bytes);
        png.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
        bytes
    }

    fn paste_fixture(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, tiny_png()).unwrap();
        path
    }

    #[test]
    fn pasted_paths_show_placeholders_that_number_per_prompt() {
        let mut app = crate::app::turn::tests::test_app();
        let first = paste_fixture("yopes-paste-1.png");
        let second = paste_fixture("yopes-paste-2.png");
        app.handle_paste(&first.display().to_string());
        app.handle_paste("just text");
        app.handle_paste(&second.display().to_string());
        assert_eq!(
            app.input.before_cursor(),
            "[Image 1]just text[Image 2]".to_string()
        );
        assert_eq!(app.attachments.len(), 2);
        assert_eq!(app.image_paths.len(), 2);
        std::fs::remove_file(&first).ok();
        std::fs::remove_file(&second).ok();
    }

    #[test]
    fn submit_swaps_placeholders_back_and_resets_the_numbering() {
        let mut app = crate::app::turn::tests::test_app();
        let path = paste_fixture("yopes-paste-3.png");
        app.handle_paste(&path.display().to_string());
        let value = app.input.take();
        let resolved = app.resolve_image_placeholders(value);
        assert_eq!(resolved, path.display().to_string());
        assert!(app.image_paths.is_empty());
        // The real submit drains the attachments into the message as well.
        app.attachments.clear();
        // The numbering restarts for the next prompt.
        app.handle_paste(&path.display().to_string());
        assert!(app.input.before_cursor().ends_with("[Image 1]"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn file_uris_with_escaped_spaces_paste_as_placeholders() {
        let mut app = crate::app::turn::tests::test_app();
        let path = paste_fixture("yopes-paste uri.png");
        let uri = format!("file://{}", path.display()).replace(' ', "%20");
        app.handle_paste(&uri);
        assert!(app.input.before_cursor().ends_with("[Image 1]"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn backspace_deletes_a_whole_placeholder_and_its_map_entry() {
        let mut app = crate::app::turn::tests::test_app();
        let path = paste_fixture("yopes-paste-4.png");
        app.handle_paste(&path.display().to_string());
        assert!(app.input.before_cursor().ends_with("[Image 1]"));
        assert!(app.remove_pasted_image_token());
        assert_eq!(app.input.before_cursor(), "");
        assert!(app.image_paths.is_empty());
        assert!(app.attachments.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn non_image_pastes_stay_plain_text() {
        let mut app = crate::app::turn::tests::test_app();
        app.handle_paste("src/lib.rs is a plain path");
        assert_eq!(app.input.before_cursor(), "src/lib.rs is a plain path");
        assert!(app.attachments.is_empty());
        assert!(app.image_paths.is_empty());
    }

    #[test]
    fn an_image_renders_as_half_block_pixel_rows() {
        use ratatui::style::Color;

        let png = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(2, 2, |_, _| {
            image::Rgb([255u8, 0, 0])
        }));
        let mut bytes = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut bytes);
        png.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);

        let rows = build_image_rows(&data, 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].spans.len(), 4);
        assert_eq!(rows[0].spans[0].content, "▄");
        assert_eq!(rows[0].spans[0].style.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(rows[0].spans[3].style.fg, Some(Color::Rgb(255, 0, 0)));
    }
}
