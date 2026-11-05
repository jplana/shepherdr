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
    /// Flag that carries the model, e.g. `--model`.
    pub model_flag: String,
    /// Flag that carries the effort level; `None` when the CLI has none.
    pub effort_flag: Option<String>,
    /// Headless resume template (`{session}`, `{model}`, `{effort}`,
    /// `{prompt}`); `None` when the CLI cannot resume headlessly.
    pub resume_command: Option<String>,
    /// The CLI's real interface, run in the pane in place of `command` (`{session}`,
    /// `{settings}`, `{first}`, `{model}`, `{effort}`). `None` means it always runs headless.
    pub tui_command: Option<String>,
    /// The same, resuming the session in `{session}`.
    pub tui_resume_command: Option<String>,
}

const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Name of the CLI a template invokes: the first word after the `cat {prompt} |` pipe.
fn cli_name(spec: &VendorSpec) -> &str {
    spec.command
        .split_once("| ")
        .map_or(spec.command.as_str(), |(_, rest)| rest)
        .split_whitespace()
        .next()
        .unwrap_or("vendor")
}

fn check_effort(spec: &VendorSpec, effort: Option<&str>) -> anyhow::Result<()> {
    if let Some(level) = effort {
        if !EFFORT_LEVELS.contains(&level) {
            anyhow::bail!(
                "unknown effort {level:?}; valid levels: {}",
                EFFORT_LEVELS.join(", ")
            );
        }
        if spec.effort_flag.is_none() {
            anyhow::bail!("{} does not support --effort", cli_name(spec));
        }
    }
    Ok(())
}

fn model_and_effort(spec: &VendorSpec, template: &str, model: Option<&str>, effort: Option<&str>) -> String {
    use crate::protocol::shell_quote;
    let flag = |flag: &str, value: Option<&str>| {
        value.map_or(String::new(), |v| format!(" {flag} {}", shell_quote(v)))
    };
    template
        .replace("{model}", &flag(&spec.model_flag, model))
        .replace("{effort}", &flag(spec.effort_flag.as_deref().unwrap_or(""), effort))
}

/// Build the command that runs the vendor's real interface, shell-quoting every value.
/// `resuming` picks whether `session` is continued or created with that ID.
pub struct TuiRun<'a> {
    pub first_message: &'a str,
    pub settings_file: &'a str,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub session: &'a str,
    pub resuming: bool,
}

pub fn tui_command(spec: &VendorSpec, run: &TuiRun) -> anyhow::Result<String> {
    use crate::protocol::shell_quote;
    check_effort(spec, run.effort)?;
    let template = if run.resuming { &spec.tui_resume_command } else { &spec.tui_command };
    let template = template
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("{} has no real interface mode", cli_name(spec)))?;
    Ok(model_and_effort(spec, template, run.model, run.effort)
        .replace("{session}", &shell_quote(run.session))
        .replace("{settings}", &shell_quote(run.settings_file))
        .replace("{first}", &shell_quote(run.first_message)))
}

/// Build the headless command line, shell-quoting every value.
pub fn headless_command(
    spec: &VendorSpec,
    prompt_file: &str,
    model: Option<&str>,
    effort: Option<&str>,
    resume_session: Option<&str>,
) -> anyhow::Result<String> {
    use crate::protocol::shell_quote;
    let cli = cli_name(spec);
    check_effort(spec, effort)?;
    let template = match resume_session {
        Some(_) => spec
            .resume_command
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("{cli} cannot resume a session headlessly"))?,
        None => spec.command.as_str(),
    };
    Ok(model_and_effort(spec, template, model, effort)
        .replace("{session}", &shell_quote(resume_session.unwrap_or("")))
        .replace("{prompt}", &shell_quote(prompt_file)))
}

pub struct Config {
    pub roles: BTreeMap<String, Vec<String>>,
    pub vendors: BTreeMap<String, VendorSpec>,
}

fn vendor(
    command: &str,
    interactive_command: &str,
    ready_cue: &str,
    model_flag: &str,
    effort_flag: Option<&str>,
    resume_command: Option<&str>,
) -> VendorSpec {
    VendorSpec {
        model_flag: model_flag.into(),
        effort_flag: effort_flag.map(Into::into),
        resume_command: resume_command.map(Into::into),
        tui_command: None,
        tui_resume_command: None,
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
    let mut claude = vendor(
            "cat {prompt} | claude -p{model}{effort} --output-format stream-json --verbose --dangerously-skip-permissions",
            "claude --dangerously-skip-permissions",
            "",
            "--model",
            Some("--effort"),
            Some("cat {prompt} | claude -p --resume {session}{model}{effort} --output-format stream-json --verbose --dangerously-skip-permissions"),
    );
    claude.tui_command = Some(
        "claude{model}{effort} --session-id {session} --settings {settings} --dangerously-skip-permissions {first}".into(),
    );
    claude.tui_resume_command = Some(
        "claude --resume {session}{model}{effort} --settings {settings} --dangerously-skip-permissions {first}".into(),
    );
    vendors.insert("claude".into(), claude);
    vendors.insert(
        "codex".into(),
        vendor(
            "cat {prompt} | codex exec{model} --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox --dangerously-bypass-hook-trust",
            "codex --dangerously-bypass-approvals-and-sandbox --dangerously-bypass-hook-trust",
            "",
            "-m",
            None,
            Some("cat {prompt} | codex exec resume --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox --dangerously-bypass-hook-trust{model} {session} -"),
        ),
    );
    vendors.insert(
        "cursor".into(),
        vendor(
            "cat {prompt} | cursor-agent -p{model} --output-format json --force",
            "cursor-agent --force",
            "",
            "--model",
            None,
            None,
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

    fn cfg(v: &str) -> VendorSpec {
        let mut c = default_config();
        c.vendors.remove(v).unwrap()
    }

    #[test]
    fn new_session_without_model_has_no_flag_or_placeholder() {
        let cmd = headless_command(&cfg("claude"), "/s/p.md", None, None, None).unwrap();
        assert!(!cmd.contains("--model"));
        assert!(!cmd.contains("{model}"));
        assert!(!cmd.contains("{prompt}"));
        assert!(cmd.contains("cat '/s/p.md' | claude -p"));
    }

    #[test]
    fn model_uses_each_vendors_flag() {
        let c = headless_command(&cfg("claude"), "/s/p.md", Some("opus"), None, None).unwrap();
        assert!(c.contains("--model 'opus'"));
        let c = headless_command(&cfg("codex"), "/s/p.md", Some("o3"), None, None).unwrap();
        assert!(c.contains("-m 'o3'"));
        let c = headless_command(&cfg("cursor"), "/s/p.md", Some("x"), None, None).unwrap();
        assert!(c.contains("--model 'x'"));
    }

    #[test]
    fn claude_resume_keeps_stream_json_and_flags() {
        let c = headless_command(&cfg("claude"), "/s/p.md", None, None, Some("sess-1")).unwrap();
        assert!(c.contains("claude -p --resume 'sess-1'"));
        assert!(c.contains("--output-format stream-json"));
        assert!(c.contains("--dangerously-skip-permissions"));
    }

    #[test]
    fn codex_resume_reads_prompt_from_stdin() {
        let c = headless_command(&cfg("codex"), "/s/p.md", None, None, Some("sess-1")).unwrap();
        assert!(c.ends_with("'sess-1' -"));
        assert!(c.contains("codex exec resume"));
        for f in ["--skip-git-repo-check", "--dangerously-bypass-approvals-and-sandbox", "--dangerously-bypass-hook-trust"] {
            assert!(c.contains(f));
            assert!(cfg("codex").command.contains(f));
        }
    }

    #[test]
    fn cursor_resume_is_a_clear_error() {
        let e = headless_command(&cfg("cursor"), "/s/p.md", None, None, Some("s")).unwrap_err().to_string();
        assert!(e.contains("cursor") && e.contains("resume"));
    }

    #[test]
    fn hostile_values_are_quoted() {
        let c = headless_command(&cfg("claude"), "/s/p.md", Some("a b'c"), None, Some("a b'c")).unwrap();
        assert!(c.contains("'a b'\\''c'"));
        assert!(c.contains("--resume 'a b'\\''c'"));
        assert!(c.contains("--model 'a b'\\''c'"));
    }

    #[test]
    fn claude_effort_is_passed_on_new_and_resumed_sessions() {
        let s = cfg("claude");
        assert!(headless_command(&s, "/p", None, Some("high"), None).unwrap().contains("--effort 'high'"));
        assert!(headless_command(&s, "/p", None, Some("high"), Some("s")).unwrap().contains("--effort 'high'"));
        for r in [None, Some("s")] {
            let c = headless_command(&s, "/p", None, None, r).unwrap();
            assert!(!c.contains("--effort") && !c.contains("{effort}"));
        }
    }

    #[test]
    fn every_valid_effort_level_is_accepted() {
        for l in ["low", "medium", "high", "xhigh", "max"] {
            assert!(headless_command(&cfg("claude"), "/p", None, Some(l), None).is_ok());
        }
    }

    #[test]
    fn unknown_effort_is_an_error_listing_levels() {
        let e = headless_command(&cfg("claude"), "/p", None, Some("bogus"), None).unwrap_err().to_string();
        assert!(e.contains("bogus") && e.contains("xhigh"));
        assert!(headless_command(&cfg("claude"), "/p", None, Some("HIGH"), None).is_err());
        assert!(headless_command(&cfg("claude"), "/p", None, Some(""), None).is_err());
    }

    #[test]
    fn effort_on_codex_or_cursor_is_a_clear_error() {
        for v in ["codex", "cursor"] {
            let s = cfg(v);
            let e = headless_command(&s, "/p", None, Some("high"), None).unwrap_err().to_string();
            assert!(e.contains(v) && e.contains("--effort"));
            assert!(!headless_command(&s, "/p", None, None, None).unwrap().contains("{effort}"));
        }
    }
}
