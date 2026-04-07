use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SwitchAction {
    #[serde(rename = "session")]
    Session { target: String },
    #[serde(rename = "window")]
    Window { session: String, window: i64 },
}

fn switch_path() -> PathBuf {
    std::env::temp_dir().join("torchard-switch.json")
}

pub fn write_switch(action: &SwitchAction) {
    let json = serde_json::to_string(action).expect("serialize switch action");
    fs::write(switch_path(), json).expect("write switch file");
}

pub fn read_switch() -> Option<SwitchAction> {
    let path = switch_path();
    let data = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn cleanup() {
    let _ = fs::remove_file(switch_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_switch_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("torchard-test-switch-{}.json", name))
    }

    #[test]
    fn roundtrip_session() {
        let path = test_switch_path("session");
        let action = SwitchAction::Session {
            target: "my-session".to_string(),
        };
        let json = serde_json::to_string(&action).unwrap();
        fs::write(&path, &json).unwrap();
        let data = fs::read_to_string(&path).unwrap();
        let read: SwitchAction = serde_json::from_str(&data).unwrap();
        match read {
            SwitchAction::Session { target } => assert_eq!(target, "my-session"),
            _ => panic!("expected Session"),
        }
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn roundtrip_window() {
        let path = test_switch_path("window");
        let action = SwitchAction::Window {
            session: "sess".to_string(),
            window: 3,
        };
        let json = serde_json::to_string(&action).unwrap();
        fs::write(&path, &json).unwrap();
        let data = fs::read_to_string(&path).unwrap();
        let read: SwitchAction = serde_json::from_str(&data).unwrap();
        match read {
            SwitchAction::Window { session, window } => {
                assert_eq!(session, "sess");
                assert_eq!(window, 3);
            }
            _ => panic!("expected Window"),
        }
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn json_format_matches_python() {
        let session = serde_json::to_string(&SwitchAction::Session {
            target: "test".into(),
        })
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&session).unwrap();
        assert_eq!(parsed["type"], "session");
        assert_eq!(parsed["target"], "test");

        let window = serde_json::to_string(&SwitchAction::Window {
            session: "s".into(),
            window: 2,
        })
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&window).unwrap();
        assert_eq!(parsed["type"], "window");
        assert_eq!(parsed["session"], "s");
        assert_eq!(parsed["window"], 2);
    }
}
