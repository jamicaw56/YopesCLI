#[cfg(test)]
mod tests {
    #[test]
    fn the_overlay_animates_toward_its_aim_instead_of_teleporting() {
        let script = super::super::SCRIPT;
        // A 60fps tick that eases a share of the remaining gap each frame.
        assert!(script.contains("FromMilliseconds(16)"), "{script}");
        assert!(script.contains("$ease = 0.35"), "{script}");
        assert!(script.contains("$script:shownX += $gapX * $ease"));
        assert!(script.contains("$script:shownY += $gapY * $ease"));
    }

    #[test]
    fn ready_handshake_waits_for_complete_signal_and_reports_explicit_error() {
        use super::super::{StartupSignal, startup_signal};

        assert_eq!(startup_signal(""), StartupSignal::Pending);
        assert_eq!(startup_signal("rea"), StartupSignal::Pending);
        assert_eq!(startup_signal(" ready\n"), StartupSignal::Ready);
        assert_eq!(
            startup_signal("error: WPF initialization failed"),
            StartupSignal::Error("WPF initialization failed".to_owned())
        );
    }

    #[test]
    fn startup_timeout_is_longer_than_the_previous_five_second_window() {
        let source = include_str!("cursor.rs");
        assert!(source.contains("Duration::from_secs(15)"));
        assert!(source.contains("PowerShell log: {detail}"));
    }

    #[test]
    fn the_overlay_is_visible_click_through_and_owner_scoped() {
        let script = super::super::SCRIPT;
        assert!(script.contains("$window.AllowsTransparency = $true"));
        assert!(script.contains("$window.Topmost = $true"));
        assert!(script.contains("$window.ShowActivated = $false"));
        assert!(script.contains("$window.IsHitTestVisible = $false"));
        assert!(script.contains("0x00000020 | 0x00000080 | 0x08000000"));
        assert!(script.contains("Get-Process -Id $OwnerId"));
        assert!(script.contains("Step-AgentCursor"));
        assert!(script.contains("Read-Aim"));
        assert!(!script.contains("Move-AgentCursor"));
        assert!(script.contains("SetWindowDisplayAffinity"));
        assert!(script.contains("SetThreadDpiAwarenessContext"));
        assert!(script.contains("GetDpiForWindow"));
        assert!(script.contains("SetWindowPos"));
        assert!(!script.contains("SetCursorPos"));
    }
}
