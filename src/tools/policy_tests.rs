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

    /// A chat turn is plain conversation: no tool text, and not even the
    /// project's AGENTS.md, which only means something to a tool-using agent.
    #[test]
    fn a_chat_prompt_carries_no_tools_and_no_project_files() {
        let root = std::env::temp_dir().join(format!("yopes-chat-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("AGENTS.md"), "cargo build is allowed").unwrap();
        let chat = system_prompt(Prompt::Chat, &root);
        let agent = system_prompt(Prompt::Tools, &root);
        assert!(chat.contains("chat companion"), "{chat}");
        assert!(!chat.contains("Project Markdown context"), "{chat}");
        assert!(!chat.contains("cargo build is allowed"), "{chat}");
        assert!(!chat.contains("run_command"), "{chat}");
        assert!(!chat.contains("Working directory"), "{chat}");
        assert!(agent.contains("Project Markdown context"), "{agent}");
        assert!(agent.contains("cargo build is allowed"), "{agent}");
        // The writing style rides on both: it is about prose, not tools.
        assert!(chat.contains("Writing style"), "{chat}");
        let _ = fs::remove_dir_all(&root);
    }
}
