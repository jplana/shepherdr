use crate::cli::DispatchArgs;
use serde_json::Value;

pub struct PluginContext {
    pub workspace_id: Option<String>,
    pub pane_id: Option<String>,
    pub worktree_path: Option<String>,
}

pub fn parse_context(json: &str) -> PluginContext {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    let text = |key: &str| v.get(key).and_then(Value::as_str).map(String::from);
    PluginContext {
        workspace_id: text("workspace_id"),
        pane_id: text("focused_pane_id"),
        // herdr reports the directory the focused pane's shell is in, which isn't
        // always the worktree its agent is working in.
        worktree_path: text("focused_pane_cwd"),
    }
}

pub fn second_opinion_context(ctx: &PluginContext) -> String {
    let path = ctx.worktree_path.as_deref().unwrap_or(".");
    let quoted = crate::protocol::shell_quote(path);
    format!(
        "You are a second reviewer. Review the current git diff in the repository \
at `{path}` (run `git -C {quoted} diff`). Report correctness bugs, risks, and \
unstated assumptions. Be concise and reference file:line.",
    )
}

/// The dispatch the `second-opinion` action runs.
pub fn second_opinion_args(ctx: &PluginContext, scratch: &str, context_file: &str) -> DispatchArgs {
    DispatchArgs {
        role: "reviewer".into(), context_file: context_file.into(),
        from_pane: ctx.pane_id.clone().unwrap_or_else(|| "1-1".into()),
        task_id: "second-opinion".into(), scratch: scratch.into(),
        vendor: None, unavailable: String::new(), timeout_ms: 600_000,
        split_direction: "right".into(), placement: "split".into(), keep_open: true, headless: false,
        state_dir: Some(scratch.into()), reuse_pane: None, interactive: false,
        model: None, effort: None, resume_session: None,
        // This sets no working directory: the pane starts in the launch directory.
        // Passing the worktree path would add nothing safe. The action is not isolated
        // from the reviewed repo, because `git -C` honors that repo's config. Isolation
        // belongs to the sandboxing work.
        cwd: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What herdr 0.9.1 really sets in HERDR_PLUGIN_CONTEXT_JSON, captured from a plugin.
    const REAL_CONTEXT: &str = r#"{"workspace_id":"wB","workspace_label":"sheperd-enhance",
        "workspace_cwd":"/Users/jplana","tab_id":"wB:t1","tab_label":"1","focused_pane_id":"wB:p1",
        "focused_pane_cwd":"/repo/wt","focused_pane_status":"unknown","invocation_source":"cli",
        "correlation_id":"cli:plugin"}"#;

    #[test]
    fn parses_the_context_herdr_really_sends() {
        let c = parse_context(REAL_CONTEXT);
        assert_eq!(c.workspace_id.as_deref(), Some("wB"));
        assert_eq!(c.pane_id.as_deref(), Some("wB:p1"));
        assert_eq!(c.worktree_path.as_deref(), Some("/repo/wt"));
    }

    #[test]
    fn tolerates_missing_fields() {
        let c = parse_context("{}");
        assert!(c.workspace_id.is_none());
        assert!(c.worktree_path.is_none());
    }

    #[test]
    fn parse_context_tolerates_garbage_json() {
        let c = parse_context("not json");
        assert!(c.workspace_id.is_none());
        assert!(c.pane_id.is_none());
        assert!(c.worktree_path.is_none());
    }

    #[test]
    fn second_opinion_stays_out_of_the_context_worktree() {
        let ctx = parse_context(REAL_CONTEXT);
        let args = second_opinion_args(&ctx, "/state", "/state/ctx.md");
        assert!(args.cwd.is_none(), "was: {:?}", args.cwd);
        assert_eq!(args.from_pane, "wB:p1");
        assert_eq!(args.role, "reviewer");
        assert_eq!(args.context_file, "/state/ctx.md");
        assert_eq!(args.scratch, "/state");
        assert_eq!(args.state_dir.as_deref(), Some("/state"));
    }

    #[test]
    fn second_opinion_without_a_worktree_leaves_cwd_unset() {
        let args = second_opinion_args(&parse_context("{}"), "/state", "/state/ctx.md");
        assert!(args.cwd.is_none());
        assert_eq!(args.from_pane, "1-1");
    }

    #[test]
    fn second_opinion_quotes_a_path_with_spaces() {
        let c = PluginContext {
            workspace_id: None, pane_id: None, worktree_path: Some("/my repo/wt".into()),
        };
        assert!(second_opinion_context(&c).contains("git -C '/my repo/wt' diff"));
    }

    #[test]
    fn second_opinion_mentions_worktree_and_diff() {
        let c = PluginContext {
            workspace_id: None, pane_id: None, worktree_path: Some("/repo/wt".into()),
        };
        let text = second_opinion_context(&c);
        assert!(text.contains("/repo/wt"));
        assert!(text.to_lowercase().contains("diff"));
    }
}
