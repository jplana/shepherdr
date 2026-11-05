use crate::protocol::tab_id_from_create;
use crate::runner::HerdrRunner;
use anyhow::Result;
use serde_json::Value;

pub fn workspace_from_event(event_json: &str) -> Option<String> {
    let value: Value = serde_json::from_str(event_json).ok()?;
    value.get("workspace")?.get("workspace_id")?.as_str().map(String::from)
}

pub fn handle_worktree_created<R: HerdrRunner>(
    runner: &R,
    event_json: &str,
) -> Result<Option<String>> {
    match workspace_from_event(event_json) {
        Some(workspace) => {
            let created = runner.tab_create(&workspace, "workers")?;
            Ok(Some(tab_id_from_create(&created)?))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeRunner { calls: RefCell<Vec<(String, String)>> }
    impl HerdrRunner for FakeRunner {
        fn split(&self, _p: &str, _d: &str, _f: bool, _c: Option<&str>) -> anyhow::Result<String> { Ok("{}".into()) }
        fn run(&self, _p: &str, _c: &str) -> anyhow::Result<()> { Ok(()) }
        fn wait_output(&self, _p: &str, _m: &str, _t: u64) -> anyhow::Result<bool> { Ok(true) }
        fn read(&self, _p: &str, _s: &str, _l: u32) -> anyhow::Result<String> { Ok(String::new()) }
        fn close(&self, _p: &str) -> anyhow::Result<()> { Ok(()) }
        fn tab_create(&self, w: &str, l: &str) -> anyhow::Result<String> {
            self.calls.borrow_mut().push((w.into(), l.into()));
            Ok(r#"{"result":{"tab":{"tab_id":"1:2"}}}"#.into())
        }
        fn pane_exists(&self, _pane: &str) -> anyhow::Result<bool> { Ok(false) }
    }

    #[test]
    fn reads_workspace_from_event() {
        assert_eq!(
            workspace_from_event(r#"{"workspace":{"workspace_id":"3"}}"#).as_deref(),
            Some("3")
        );
        assert!(workspace_from_event("{}").is_none());
    }

    #[test]
    fn creates_workers_tab_in_event_workspace() {
        let r = FakeRunner { calls: RefCell::new(vec![]) };
        let tab = handle_worktree_created(&r, r#"{"workspace":{"workspace_id":"3"}}"#).unwrap();
        assert_eq!(tab.as_deref(), Some("1:2"));
        assert_eq!(r.calls.borrow().as_slice(), [("3".to_string(), "workers".to_string())]);
    }

    #[test]
    fn no_workspace_creates_nothing() {
        let r = FakeRunner { calls: RefCell::new(vec![]) };
        assert!(handle_worktree_created(&r, "{}").unwrap().is_none());
        assert!(r.calls.borrow().is_empty());
    }
}
