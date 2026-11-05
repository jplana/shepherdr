use crate::cli::DispatchArgs;
use serde_json::Value;

pub struct PluginContext {
    pub workspace_id: Option<String>,
    pub pane_id: Option<String>,
    pub worktree_path: Option<String>,
}

fn get_str(v: &Value, path: &[&str]) -> Option<String> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_str().map(String::from)
}

pub fn parse_context(json: &str) -> PluginContext {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    PluginContext {
        workspace_id: get_str(&v, &["workspace", "workspace_id"]),
        pane_id: get_str(&v, &["focused_pane", "pane_id"]),
        worktree_path: get_str(&v, &["worktree", "path"]),
    }
}

pub fn second_opinion_context(ctx: &PluginContext) -> String {
    let path = ctx.worktree_path.as_deref().unwrap_or(".");
    format!(
        "You are a second reviewer. Review the current git diff in the repository \
at `{path}` (run `git -C {path} diff`). Report correctness bugs, risks, and \
unstated assumptions. Be concise and reference file:line.",
    )
}

/// The dispatch the `second-opinion` action runs, in the context's worktree when
/// it has one.
pub fn second_opinion_args(ctx: &PluginContext, scratch: &str, context_file: &str) -> DispatchArgs {
    DispatchArgs {
        role: "reviewer".into(), context_file: context_file.into(),
        from_pane: ctx.pane_id.clone().unwrap_or_else(|| "1-1".into()),
        task_id: "second-opinion".into(), scratch: scratch.into(),
        vendor: None, unavailable: String::new(), timeout_ms: 600_000,
        split_direction: "right".into(), close: false,
        state_dir: Some(scratch.into()), reuse_pane: None, interactive: false,
        model: None, effort: None, resume_session: None,
        cwd: ctx.worktree_path.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_present_fields() {
        let j = r#"{"workspace":{"workspace_id":"1"},
                    "focused_pane":{"pane_id":"1-2"},
                    "worktree":{"path":"/repo/wt"}}"#;
        let c = parse_context(j);
        assert_eq!(c.workspace_id.as_deref(), Some("1"));
        assert_eq!(c.pane_id.as_deref(), Some("1-2"));
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
    fn second_opinion_runs_in_the_context_worktree() {
        let ctx = parse_context(r#"{"focused_pane":{"pane_id":"1-2"},"worktree":{"path":"/repo/wt"}}"#);
        let args = second_opinion_args(&ctx, "/state", "/state/ctx.md");
        assert_eq!(args.cwd.as_deref(), Some("/repo/wt"));
        assert_eq!(args.from_pane, "1-2");
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
    fn second_opinion_mentions_worktree_and_diff() {
        let c = PluginContext {
            workspace_id: None, pane_id: None, worktree_path: Some("/repo/wt".into()),
        };
        let text = second_opinion_context(&c);
        assert!(text.contains("/repo/wt"));
        assert!(text.to_lowercase().contains("diff"));
    }
}
