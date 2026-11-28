#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::{
        capture_script, chord_keys, parse_capture, parse_resolve, parse_status, resolve_script,
        virtual_key, window_script,
    };
    use crate::tools::computer::{Button, Region, Step};

    #[test]
    fn the_resolve_line_names_the_handle_client_area_and_title() {
        let target = parse_resolve(
            "noise\nwindow 66052 100 200 800 600\ntitle Untitled - Notepad\n",
        )
        .unwrap();
        assert_eq!(target.handle, 66052);
        assert_eq!(target.client, (100, 200, 800, 600));
        assert_eq!(target.title, "Untitled - Notepad");
        // A missing geometry line is refused rather than guessed at.
        assert!(parse_resolve("title only\n").is_none());
    }

    #[test]
    fn the_capture_lines_say_how_the_pixels_were_taken() {
        let (how, client) = parse_capture("grab print\nclient 40 60 1200 900\n").unwrap();
        assert_eq!(how, "print");
        assert_eq!(client, (40, 60, 1200, 900));
        assert_eq!(parse_capture("grab screen\nclient 0 0 10 10\n").unwrap().0, "screen");
        assert!(parse_capture("client 0 0 10 10\n").is_none());
    }

    #[test]
    fn the_status_file_follows_the_last_window() {
        let target = parse_status("66052\n10 20 640 480\nUntitled - Notepad\n").unwrap();
        assert_eq!(target.handle, 66052);
        assert_eq!(target.client, (10, 20, 640, 480));
        assert_eq!(target.title, "Untitled - Notepad");
        assert!(parse_status("66052\n10 20\n").is_none());
    }

    #[test]
    fn named_keys_and_chords_become_virtual_codes() {
        assert_eq!(virtual_key("ENTER").unwrap(), 0x0D);
        assert_eq!(virtual_key("PageDown").unwrap(), 0x22);
        assert_eq!(virtual_key("s").unwrap(), 0x53);
        assert!(virtual_key("hyperdrive").is_err());
        assert_eq!(chord_keys("ctrl+s").unwrap(), (vec![0x11], 0x53));
        assert_eq!(chord_keys("ctrl+shift+t").unwrap(), (vec![0x11, 0x10], 0x54));
        assert!(chord_keys("hyper+s").is_err());
    }

    #[test]
    fn the_background_script_posts_instead_of_injecting() {
        let steps = vec![
            Step::Move { x: 5, y: 6 },
            Step::Click {
                x: 7,
                y: 8,
                button: Button::Right,
                count: 2,
            },
            Step::Type {
                text: "it's".to_owned(),
            },
            Step::Key {
                keys: Vec::new(),
                chord: "ctrl+s".to_owned(),
            },
        ];
        let script = window_script(&steps, Some((0, 0)), 66052, Path::new("C:\\Temp\\w")).unwrap();
        // Posted messages, and no real input at all: nothing here may move the
        // system pointer or raise a window.
        assert!(script.contains("[YopesWindowMode]::Click($h, 7, 8, 1, 2)"), "{script}");
        assert!(script.contains("[YopesWindowMode]::Text($h, 'it''s')"), "{script}");
        assert!(script.contains("[YopesWindowMode]::Chord($h, @(17), 83)"), "{script}");
        assert!(!script.contains("SendInput"), "{script}");
        assert!(!script.contains("SetCursorPos"), "{script}");
        assert!(!script.contains("SetForegroundWindow"), "{script}");
        assert!(!script.contains("SendKeys"), "{script}");
        // The glide posts points between the start and the aim.
        let moves = script.matches("[YopesWindowMode]::Move(").count();
        assert!(moves >= 6, "{script}");
        // The last target rides back to the caller, for the after capture.
        assert!(script.contains("WriteAllLines"), "{script}");
        assert!(script.ends_with("exit 0;\n"), "{script}");
    }

    #[test]
    fn a_background_focus_step_retargets_without_raising() {
        let steps = vec![Step::FocusWindow {
            title: "Notepad".to_owned(),
        }];
        let script = window_script(&steps, None, 66052, Path::new("C:\\Temp\\w")).unwrap();
        assert!(script.contains("$h = [YopesWindowMode]::ByTitle('Notepad')"), "{script}");
        assert!(!script.contains("SetForegroundWindow"), "{script}");
        assert!(!script.contains("ShowWindow"), "{script}");
    }

    #[test]
    fn a_drag_is_a_press_a_glide_and_a_release() {
        let steps = vec![Step::Drag {
            from_x: 10,
            from_y: 10,
            to_x: 210,
            to_y: 10,
            button: Button::Left,
        }];
        let script = window_script(&steps, None, 1, Path::new("C:\\Temp\\w")).unwrap();
        assert!(script.contains("[YopesWindowMode]::DragBegin($h, 10, 10, 0)"), "{script}");
        assert!(script.contains("[YopesWindowMode]::DragEnd($h)"), "{script}");
        let steps_posted = script.matches("[YopesWindowMode]::DragStep(").count();
        assert!(steps_posted >= 6, "{script}");
    }

    #[test]
    fn a_region_crops_the_window_capture() {
        let script = capture_script(
            7,
            Some(Region {
                x: 10,
                y: 20,
                width: 300,
                height: 200,
            }),
            Path::new("C:\\Temp\\a.png"),
        );
        assert!(script.contains(r"Grab($h, 'C:\Temp\a.png', 10, 20, 300, 200)"), "{script}");
        let full = capture_script(7, None, Path::new("C:\\Temp\\a.png"));
        assert!(full.contains(r"Grab($h, 'C:\Temp\a.png', 0, 0, 0, 0)"), "{full}");
    }

    #[test]
    fn the_resolve_script_quotes_the_title_it_looks_for() {
        let script = resolve_script("it's");
        assert!(script.contains("ByTitle('it''s')"), "{script}");
        assert!(script.contains("no window title contains ' + 'it''s'"), "{script}");
    }

/// End to end against a real window: spawn Notepad, capture it, post a click
/// and a keystroke into it, capture again. Not part of the normal run because
/// it opens an app and needs a desktop; run it with
/// `cargo test -- --ignored a_live_window`.
#[cfg(windows)]
#[test]
#[ignore]
fn a_live_window_can_be_driven_in_the_background() {
    let mut child = std::process::Command::new("notepad.exe")
        .spawn()
        .expect("notepad should start");
    std::thread::sleep(std::time::Duration::from_secs(2));
    let result = (|| -> anyhow::Result<()> {
        let target = super::super::resolve("Notepad")?;
        assert!(target.client.2 > 0 && target.client.3 > 0, "{target:?}");
        let (shot, how) = super::super::capture(target.handle, None)?;
        assert!(shot.path.is_file(), "{}", shot.path.display());
        assert!(shot.size.0 > 0 && shot.size.1 > 0, "{:?}", shot.size);
        assert!(how == "print" || how == "screen", "{how}");
        let steps = vec![
            Step::Click {
                x: 200,
                y: 200,
                button: Button::Left,
                count: 1,
            },
            Step::Type {
                text: "hello from the background".to_owned(),
            },
        ];
        let status = super::super::status_path();
        let script = super::super::window_script(&steps, None, target.handle, &status)?;
        crate::tools::computer::run_script(&script)?;
        let ended = std::fs::read_to_string(&status).unwrap_or_default();
        let after = super::super::parse_status(&ended).expect("status");
        assert_eq!(after.handle, target.handle);
        let (again, _) = super::super::capture(after.handle, None)?;
        assert!(again.path.is_file());
        Ok(())
    })();
    let _ = child.kill();
    let _ = child.wait();
    result.expect("background drive");
}
}
