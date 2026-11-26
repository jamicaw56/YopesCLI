#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn agents_scan_is_bounded_and_keeps_root_instructions() {
        let root = std::env::temp_dir().join(format!("yopes-agents-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("AGENTS.md"), "root rules").unwrap();
        for index in 0..300 {
            let directory = root.join(format!("d{index:03}"));
            fs::create_dir(&directory).unwrap();
            if index == 1 || index == 299 {
                fs::write(directory.join("AGENTS.md"), format!("rules {index}")).unwrap();
            }
        }
        let text = load_agents_instructions(&root);
        assert!(text.contains("root rules"), "{text}");
        assert!(text.contains("rules 1"), "{text}");
        assert!(!text.contains("rules 299"), "{text}");
        let _ = fs::remove_dir_all(&root);
    }
}
