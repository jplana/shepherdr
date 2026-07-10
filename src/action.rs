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
    fn second_opinion_mentions_worktree_and_diff() {
        let c = PluginContext {
            workspace_id: None, pane_id: None, worktree_path: Some("/repo/wt".into()),
        };
        let text = second_opinion_context(&c);
        assert!(text.contains("/repo/wt"));
        assert!(text.to_lowercase().contains("diff"));
    }
}
