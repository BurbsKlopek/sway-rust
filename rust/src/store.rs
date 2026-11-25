use crate::bridge;
use crate::models::PersistedState;
use std::fs;
use std::path::PathBuf;

fn state_path() -> PathBuf {
    let dir = PathBuf::from(bridge::get_files_dir());
    let _ = fs::create_dir_all(&dir);
    dir.join("sway_rust_state.json")
}

pub fn load_state() -> PersistedState {
    let path = state_path();
    match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => PersistedState::default(),
    }
}

pub fn save_state(state: &PersistedState) {
    let path = state_path();
    let tmp = path.with_extension("json.tmp");
    if let Ok(json) = serde_json::to_string(state) {
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}
