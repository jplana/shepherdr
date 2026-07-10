use std::collections::BTreeMap;

pub struct VendorSpec {
    /// Headless invocation: `cat {prompt} | <cli> ...` (autonomy flags).
    pub command: String,
    /// Bare interactive TUI launch with gate-bypass flags (no prompt piped).
    pub interactive_command: String,
    /// Substring signalling the TUI is ready for input. Empty means "treat
    /// any non-empty visible output as ready" (with a timeout fallback);
    /// this is the fragile per-vendor knob, tuned by the live `--interactive`
    /// smoke test.
    pub ready_cue: String,
}

pub struct Config {
    pub roles: BTreeMap<String, Vec<String>>,
    pub vendors: BTreeMap<String, VendorSpec>,
}

fn vendor(command: &str, interactive_command: &str, ready_cue: &str) -> VendorSpec {
    VendorSpec {
        command: command.into(),
        interactive_command: interactive_command.into(),
        ready_cue: ready_cue.into(),
    }
}

pub fn default_config() -> Config {
    let mut roles = BTreeMap::new();
    roles.insert("implementer".into(), vec!["claude".into(), "cursor".into()]);
    roles.insert("reviewer".into(), vec!["codex".into(), "cursor".into()]);

    let mut vendors = BTreeMap::new();
    vendors.insert(
        "claude".into(),
        vendor(
            "cat {prompt} | claude -p --output-format stream-json --verbose --dangerously-skip-permissions",
            "claude --dangerously-skip-permissions",
            "",
        ),
    );
    vendors.insert(
        "codex".into(),
        vendor(
            "cat {prompt} | codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox --dangerously-bypass-hook-trust",
            "codex --dangerously-bypass-approvals-and-sandbox --dangerously-bypass-hook-trust",
            "",
        ),
    );
    vendors.insert(
        "cursor".into(),
        vendor(
            "cat {prompt} | cursor-agent -p --output-format json --force",
            "cursor-agent --force",
            "",
        ),
    );

    Config { roles, vendors }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implementer_prefers_claude_reviewer_prefers_codex() {
        let config = default_config();
        assert_eq!(config.roles["implementer"][0], "claude");
        assert_eq!(config.roles["reviewer"][0], "codex");
    }

    #[test]
    fn cursor_is_backup_at_end_of_every_chain() {
        let config = default_config();
        for chain in config.roles.values() {
            assert_eq!(chain.last().unwrap(), "cursor");
        }
    }

    #[test]
    fn vendor_commands_are_headless_invocations() {
        let config = default_config();
        assert!(config.vendors["claude"].command.contains("claude -p"));
        assert!(config.vendors["codex"].command.contains("codex exec"));
        assert!(config.vendors["cursor"].command.contains("cursor-agent -p"));
    }

    #[test]
    fn claude_streams_json_with_session_id() {
        let config = default_config();
        assert!(config.vendors["claude"].command.contains("--output-format stream-json"));
        assert!(config.vendors["claude"].command.contains("--verbose"));
    }

    #[test]
    fn cursor_emits_json_output() {
        let config = default_config();
        assert!(config.vendors["cursor"].command.contains("--output-format json"));
    }

    #[test]
    fn interactive_commands_launch_bare_tui_with_bypass() {
        let config = default_config();
        assert!(config.vendors["claude"].interactive_command.contains("claude"));
        assert!(config.vendors["claude"].interactive_command.contains("--dangerously-skip-permissions"));
        // no headless `-p` print flag (as a space-delimited token; the bare
        // string "-p" also matches inside "--dangerously-skip-permissions").
        assert!(!config.vendors["claude"].interactive_command.contains(" -p"));
        assert!(config.vendors["codex"].interactive_command.contains("--dangerously-bypass-approvals-and-sandbox"));
        assert!(config.vendors["cursor"].interactive_command.contains("--force"));
    }

    #[test]
    fn vendor_commands_run_autonomously() {
        let config = default_config();
        assert!(config.vendors["claude"].command.contains("--dangerously-skip-permissions"));
        assert!(config.vendors["codex"].command.contains("--dangerously-bypass-approvals-and-sandbox"));
        assert!(config.vendors["codex"].command.contains("--skip-git-repo-check"));
        assert!(config.vendors["cursor"].command.contains("--force"));
    }
}
