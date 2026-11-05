use std::collections::BTreeMap;

pub struct VendorSpec {
    /// Headless invocation: `cat {prompt} | <cli> ...`.
    pub command: String,
    /// Bare interactive TUI launch, for you to drive (no prompt piped).
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
    /// Headless run that can't change anything (`{result}`), where the CLI itself saves
    /// the final message to the result file. `None` means no such mode.
    pub read_only_command: Option<String>,
    /// The same, resuming the session in `{session}`.
    pub read_only_resume_command: Option<String>,
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

/// Build the read-only headless command, if the vendor has that mode.
pub fn read_only_command(
    spec: &VendorSpec,
    prompt_file: &str,
    result_file: &str,
    model: Option<&str>,
    effort: Option<&str>,
    resume_session: Option<&str>,
) -> Option<anyhow::Result<String>> {
    use crate::protocol::shell_quote;
    let template = match resume_session {
        Some(_) => spec.read_only_resume_command.as_deref()?,
        None => spec.read_only_command.as_deref()?,
    };
    Some(check_effort(spec, effort).map(|()| {
        model_and_effort(spec, template, model, effort)
            .replace("{session}", &shell_quote(resume_session.unwrap_or("")))
            .replace("{result}", &shell_quote(result_file))
            .replace("{prompt}", &shell_quote(prompt_file))
    }))
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
    // The agent writes its result file next to the prompt, so its sandbox must allow that.
    let scratch = std::path::Path::new(prompt_file)
        .parent()
        .map_or(String::new(), |dir| dir.to_string_lossy().into_owned());
    let roots = format!(
        "sandbox_workspace_write.writable_roots=[{}]",
        serde_json::Value::from(scratch.as_str()),
    );
    Ok(model_and_effort(spec, template, model, effort)
        .replace("{session}", &shell_quote(resume_session.unwrap_or("")))
        .replace("{scratch}", &shell_quote(&scratch))
        .replace("{writable_roots}", &shell_quote(&roots))
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
        read_only_command: None,
        read_only_resume_command: None,
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
            "cat {prompt} | claude -p{model}{effort} --output-format stream-json --verbose --permission-mode auto",
            "claude",
            "",
            "--model",
            Some("--effort"),
            Some("cat {prompt} | claude -p --resume {session}{model}{effort} --output-format stream-json --verbose --permission-mode auto"),
    );
    claude.tui_command = Some(
        "claude{model}{effort} --session-id {session} --settings {settings} --permission-mode auto {first}".into(),
    );
    claude.tui_resume_command = Some(
        "claude --resume {session}{model}{effort} --settings {settings} --permission-mode auto {first}".into(),
    );
    vendors.insert("claude".into(), claude);
    // The sandbox flags differ on resume because `codex exec resume` only takes `-c`.
    let mut codex = vendor(
        "cat {prompt} | codex exec{model} --skip-git-repo-check --sandbox workspace-write --add-dir {scratch}",
        "codex",
        "",
        "-m",
        None,
        Some("cat {prompt} | codex exec resume --skip-git-repo-check -c 'sandbox_mode=\"workspace-write\"' -c {writable_roots}{model} {session} -"),
    );
    codex.read_only_command = Some(
        "cat {prompt} | codex exec{model} --skip-git-repo-check --sandbox read-only --output-last-message {result}".into(),
    );
    codex.read_only_resume_command = Some(
        "cat {prompt} | codex exec resume --skip-git-repo-check -c 'sandbox_mode=\"read-only\"' --output-last-message {result}{model} {session} -".into(),
    );
    vendors.insert("codex".into(), codex);
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
    fn interactive_commands_are_the_bare_clis_for_you_to_drive() {
        let config = default_config();
        assert_eq!(config.vendors["claude"].interactive_command, "claude");
        assert_eq!(config.vendors["codex"].interactive_command, "codex");
    }

    #[test]
    fn vendor_commands_run_autonomously() {
        let config = default_config();
        assert!(config.vendors["claude"].command.contains("--permission-mode auto"));
        assert!(config.vendors["codex"].command.contains("--sandbox workspace-write"));
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
        assert!(c.contains("--permission-mode auto"));
    }

    #[test]
    fn codex_resume_reads_prompt_from_stdin() {
        let c = headless_command(&cfg("codex"), "/s/p.md", None, None, Some("sess-1")).unwrap();
        assert!(c.ends_with("'sess-1' -"));
        assert!(c.contains("codex exec resume"));
        assert!(c.contains("--skip-git-repo-check"));
        assert!(cfg("codex").command.contains("--skip-git-repo-check"));
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

    #[test]
    fn no_claude_or_codex_command_bypasses_permissions_or_trust() {
        for name in ["claude", "codex"] {
            let spec = cfg(name);
            for template in [
                Some(&spec.command), Some(&spec.interactive_command), spec.resume_command.as_ref(),
                spec.tui_command.as_ref(), spec.tui_resume_command.as_ref(),
                spec.read_only_command.as_ref(), spec.read_only_resume_command.as_ref(),
            ].into_iter().flatten() {
                assert!(!template.contains("dangerously"), "{name}: {template}");
                assert!(!template.contains("full-auto"), "{name}: {template}");
            }
        }
    }

    #[test]
    fn claudes_real_interface_runs_in_auto_mode_new_and_resumed() {
        for resuming in [false, true] {
            let c = tui_command(&cfg("claude"), &TuiRun {
                first_message: "go", settings_file: "/s/x.json", model: None, effort: None,
                session: "sess", resuming,
            }).unwrap();
            assert!(c.contains("--permission-mode auto"), "was: {c}");
        }
    }

    #[test]
    fn a_codex_implementer_can_write_its_workspace_and_the_scratch_dir_only() {
        let c = headless_command(&cfg("codex"), "/s c/p.md", None, None, None).unwrap();
        assert!(c.contains("--sandbox workspace-write"), "was: {c}");
        assert!(c.contains("--add-dir '/s c'"), "was: {c}");
    }

    #[test]
    fn a_resumed_codex_implementer_keeps_the_same_sandbox() {
        let c = headless_command(&cfg("codex"), "/s/p.md", None, None, Some("sess")).unwrap();
        assert!(c.contains(r#"-c 'sandbox_mode="workspace-write"'"#), "was: {c}");
        assert!(c.contains(r#"-c 'sandbox_workspace_write.writable_roots=["/s"]'"#), "was: {c}");
    }

    #[test]
    fn a_hostile_scratch_path_stays_one_shell_word_in_the_resumed_sandbox() {
        let c = headless_command(&cfg("codex"), "/s'\"x/p.md", None, None, Some("sess")).unwrap();
        assert!(c.contains(r#"writable_roots=["/s'\''\"x"]'"#), "was: {c}");
    }

    #[test]
    fn a_codex_reviewer_is_read_only_and_codex_itself_saves_the_answer() {
        let spec = cfg("codex");
        let new = read_only_command(&spec, "/s/p.md", "/s/r.md", Some("o3"), None, None).unwrap().unwrap();
        assert!(new.contains("--sandbox read-only"), "was: {new}");
        assert!(new.contains("--output-last-message '/s/r.md'"), "was: {new}");
        assert!(new.contains("-m 'o3'"), "was: {new}");
        let resumed = read_only_command(&spec, "/s/p.md", "/s/r.md", None, None, Some("sess")).unwrap().unwrap();
        assert!(resumed.contains(r#"-c 'sandbox_mode="read-only"'"#), "was: {resumed}");
        assert!(resumed.contains("--output-last-message '/s/r.md'"), "was: {resumed}");
        assert!(resumed.ends_with("'sess' -"), "was: {resumed}");
    }

    #[test]
    fn a_vendor_with_no_read_only_mode_says_so() {
        assert!(read_only_command(&cfg("cursor"), "/p", "/r", None, None, None).is_none());
    }
}
