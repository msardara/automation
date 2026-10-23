use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct State {
    next_episode: u32,
}

pub fn load_next_episode(path: &Path) -> Option<u32> {
    let data = std::fs::read(path).ok()?;
    serde_json::from_slice::<State>(&data).ok().map(|s| s.next_episode)
}

pub fn save_next_episode(path: &Path, episode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec(&State { next_episode: episode })?;
    std::fs::write(path, data).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let path = std::env::temp_dir().join("opf-watch-state-test/state.json");
        assert_eq!(load_next_episode(&path.with_file_name("missing.json")), None);
        save_next_episode(&path, 1182).unwrap();
        assert_eq!(load_next_episode(&path), Some(1182));
    }
}
