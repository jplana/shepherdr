use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const PLUGIN_ID: &str = "jplana.shepherdr";

#[derive(Serialize, Deserialize, Clone)]
pub struct DispatchRecord {
    pub task_id: String,
    pub role: String,
    pub vendor: String,
    pub pane_id: String,
    pub status: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub created_at: String,
}

fn default_mode() -> String {
    "headless".to_string()
}

/// Epoch-milliseconds as a string, stamped once per dispatch. Sorts
/// correctly when parsed back to an integer (see `latest_resumable`).
pub fn now_created_at() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

pub fn write_record(state_dir: &Path, rec: &DispatchRecord) -> Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let path = state_dir.join(format!("{}.json", rec.task_id));
    std::fs::write(path, serde_json::to_string_pretty(rec)?)?;
    Ok(())
}

pub fn read_records(state_dir: &Path) -> Result<Vec<DispatchRecord>> {
    let mut out = Vec::new();
    if !state_dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(state_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path)?;
        if let Ok(rec) = serde_json::from_str::<DispatchRecord>(&text) {
            out.push(rec);
        }
    }
    out.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    Ok(out)
}

pub fn read_record(state_dir: &Path, task_id: &str) -> Result<Option<DispatchRecord>> {
    let path = state_dir.join(format!("{task_id}.json"));
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str::<DispatchRecord>(&text).ok())
}

/// Where dispatch records live, which is the folder herdr gives this plugin. Plugin
/// commands get it from herdr. Other callers, such as the skill's dispatches, work it out.
pub fn state_dir(
    plugin_env: Option<String>,
    xdg_state_home: Option<String>,
    home: Option<String>,
) -> Option<PathBuf> {
    let given = |v: Option<String>| v.filter(|s| !s.is_empty());
    if let Some(dir) = given(plugin_env) {
        return Some(PathBuf::from(dir));
    }
    let base = given(xdg_state_home)
        .map(PathBuf::from)
        .or_else(|| given(home).map(|home| Path::new(&home).join(".local/state")))?;
    Some(base.join("herdr/plugins").join(PLUGIN_ID))
}

/// The newest `n` records, newest first.
pub fn newest(mut records: Vec<DispatchRecord>, n: usize) -> Vec<DispatchRecord> {
    records.sort_by_key(|r| std::cmp::Reverse(r.created_at.parse::<u128>().unwrap_or(0)));
    records.truncate(n);
    records
}

/// The newest resumable dispatch: latest `created_at` among records that have
/// both a captured `session_id` and `status == "done"`. `created_at` is an
/// epoch-millis string, compared numerically.
pub fn latest_resumable(records: &[DispatchRecord]) -> Option<DispatchRecord> {
    records
        .iter()
        .filter(|r| r.session_id.is_some() && r.status == "done")
        .max_by_key(|r| r.created_at.parse::<u128>().unwrap_or(0))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, status: &str) -> DispatchRecord {
        DispatchRecord {
            task_id: id.into(), role: "reviewer".into(), vendor: "codex".into(),
            pane_id: "1-3".into(), status: status.into(),
            session_id: None, mode: "headless".into(),
            cwd: String::new(), created_at: String::new(),
        }
    }

    fn full_rec(id: &str, status: &str, session_id: Option<&str>, created_at: &str) -> DispatchRecord {
        DispatchRecord {
            task_id: id.into(), role: "reviewer".into(), vendor: "codex".into(),
            pane_id: "1-3".into(), status: status.into(),
            session_id: session_id.map(String::from), mode: "headless".into(),
            cwd: "/wt".into(), created_at: created_at.into(),
        }
    }

    #[test]
    fn latest_resumable_picks_newest_done_with_session_id() {
        let records = vec![
            full_rec("old", "done", Some("s-old"), "100"),
            full_rec("newest", "done", Some("s-new"), "300"),
            full_rec("no-id", "done", None, "400"),          // no session id
            full_rec("not-done", "working", Some("s-x"), "500"), // not done
        ];
        let picked = latest_resumable(&records).unwrap();
        assert_eq!(picked.task_id, "newest");
    }

    #[test]
    fn latest_resumable_none_when_nothing_qualifies() {
        let records = vec![full_rec("a", "working", None, "1")];
        assert!(latest_resumable(&records).is_none());
    }

    #[test]
    fn legacy_record_without_new_fields_deserializes_with_defaults() {
        let json = r#"{"task_id":"t1","role":"reviewer","vendor":"codex","pane_id":"1-3","status":"done"}"#;
        let rec: DispatchRecord = serde_json::from_str(json).unwrap();
        assert_eq!(rec.session_id, None);
        assert_eq!(rec.mode, "headless");
        assert_eq!(rec.cwd, "");
        assert_eq!(rec.created_at, "");
    }

    #[test]
    fn now_created_at_is_a_parseable_millis_string() {
        let stamp = now_created_at();
        assert!(stamp.parse::<u128>().is_ok(), "expected millis integer, got {stamp:?}");
    }

    #[test]
    fn write_then_read_roundtrips() {
        let dir = std::env::temp_dir().join("hd-state-1");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        write_record(&dir, &rec("t1", "working")).unwrap();
        write_record(&dir, &rec("t2", "done")).unwrap();
        let records = read_records(&dir).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].task_id, "t1");
        assert_eq!(records[1].status, "done");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_record_finds_one_by_task_id() {
        let dir = std::env::temp_dir().join("hd-state-read1");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        write_record(&dir, &rec("t1", "done")).unwrap();
        let found = read_record(&dir, "t1").unwrap();
        assert_eq!(found.unwrap().task_id, "t1");
        assert!(read_record(&dir, "missing").unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_overwrites_same_task() {
        let dir = std::env::temp_dir().join("hd-state-2");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        write_record(&dir, &rec("t1", "working")).unwrap();
        write_record(&dir, &rec("t1", "done")).unwrap();
        let records = read_records(&dir).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "done");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn state_dir_prefers_what_herdr_gave_the_plugin() {
        let got = state_dir(Some("/from/herdr".into()), Some("/xdg".into()), Some("/home/me".into()));
        assert_eq!(got.unwrap(), Path::new("/from/herdr"));
    }

    #[test]
    fn state_dir_is_worked_out_the_way_herdr_lays_it_out() {
        let got = state_dir(None, None, Some("/home/me".into()));
        assert_eq!(got.unwrap(), Path::new("/home/me/.local/state/herdr/plugins/jplana.shepherdr"));
        let got = state_dir(None, Some("/xdg".into()), Some("/home/me".into()));
        assert_eq!(got.unwrap(), Path::new("/xdg/herdr/plugins/jplana.shepherdr"));
    }

    #[test]
    fn state_dir_ignores_empty_values_and_can_be_unknown() {
        let got = state_dir(Some(String::new()), Some(String::new()), Some("/home/me".into()));
        assert_eq!(got.unwrap(), Path::new("/home/me/.local/state/herdr/plugins/jplana.shepherdr"));
        assert!(state_dir(None, None, None).is_none());
    }

    #[test]
    fn newest_keeps_the_latest_records_newest_first() {
        let at = |id: &str, created: &str| DispatchRecord { created_at: created.into(), ..rec(id, "done") };
        let kept = newest(vec![at("a", "9"), at("b", "100"), at("c", "20")], 2);
        let ids: Vec<&str> = kept.iter().map(|r| r.task_id.as_str()).collect();
        assert_eq!(ids, ["b", "c"], "100 is newer than 20 and 9, compared as numbers");
    }
}
