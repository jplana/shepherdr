use crate::state::{newest, read_records, DispatchRecord};
use std::path::Path;

/// Every dispatch leaves a record, so the board shows only the latest ones.
const BOARD_ROWS: usize = 20;

pub fn render(records: &[DispatchRecord]) -> String {
    if records.is_empty() {
        return "workers board\n\n(no active dispatches)\n".to_string();
    }
    let mut out = String::from("workers board\n\n");
    out.push_str(&format!("{:<20} {:<12} {:<8} {:<6} {:<10} {:<12}\n",
        "TASK", "ROLE", "VENDOR", "PANE", "STATUS", "SESSION"));
    for r in records {
        let session: String = match &r.session_id {
            Some(id) => id.chars().take(8).collect(),
            None => "-".to_string(),
        };
        out.push_str(&format!("{:<20} {:<12} {:<8} {:<6} {:<10} {:<12}\n",
            r.task_id, r.role, r.vendor, r.pane_id, r.status, session));
    }
    out
}

pub fn run_board_loop(state_dir: &Path) -> ! {
    loop {
        let records = newest(read_records(state_dir).unwrap_or_default(), BOARD_ROWS);
        print!("\x1b[2J\x1b[H{}", render(&records));
        use std::io::Write;
        std::io::stdout().flush().ok();
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::DispatchRecord;

    fn rec(id: &str, status: &str) -> DispatchRecord {
        DispatchRecord {
            task_id: id.into(), role: "reviewer".into(), vendor: "codex".into(),
            pane_id: "1-3".into(), status: status.into(),
            session_id: None, mode: "headless".into(),
            cwd: String::new(), created_at: String::new(),
        }
    }

    #[test]
    fn empty_shows_placeholder() {
        assert!(render(&[]).contains("no active dispatches"));
    }

    fn rec_with_session(id: &str, session_id: Option<&str>) -> DispatchRecord {
        DispatchRecord {
            task_id: id.into(), role: "reviewer".into(), vendor: "codex".into(),
            pane_id: "1-3".into(), status: "done".into(),
            session_id: session_id.map(String::from), mode: "headless".into(),
            cwd: String::new(), created_at: String::new(),
        }
    }

    #[test]
    fn renders_session_column() {
        let out = render(&[
            rec_with_session("t1", Some("sess-abcdef123")),
            rec_with_session("t2", None),
        ]);
        assert!(out.contains("SESSION"));
        assert!(out.contains("sess-abc"), "resumable id fragment missing: {out}");
        assert!(out.contains(" - "), "non-resumable dash missing: {out}");
    }

    #[test]
    fn renders_headers_and_rows() {
        let out = render(&[rec("t1", "working"), rec("t2", "done")]);
        assert!(out.contains("TASK"));
        assert!(out.contains("STATUS"));
        assert!(out.contains("t1"));
        assert!(out.contains("working"));
        assert!(out.contains("t2"));
        assert!(out.contains("done"));
    }
}
