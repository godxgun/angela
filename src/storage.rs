use crate::domain::State;
use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};

pub fn directory() -> Result<PathBuf> {
    let p = if let Some(p) = std::env::var_os("ANGELA_DATA_DIR") {
        PathBuf::from(p)
    } else {
        directories::ProjectDirs::from("net", "godgun", "angela")
            .context("No user data directory")?
            .data_dir()
            .to_path_buf()
    };
    fs::create_dir_all(&p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700))?;
    }
    Ok(p)
}
pub fn read<T: DeserializeOwned>(name: &str) -> Result<T> {
    Ok(serde_json::from_slice(&fs::read(directory()?.join(name))?)?)
}
pub fn write<T: Serialize>(name: &str, value: &T) -> Result<()> {
    let p = directory()?.join(name);
    let tmp = p.with_extension("tmp");
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options.open(&tmp)?;
    f.write_all(&serde_json::to_vec_pretty(value)?)?;
    f.sync_all()?;
    fs::rename(tmp, p)?;
    Ok(())
}
pub fn load() -> Result<State> {
    let p = directory()?.join("state.json");
    if p.exists() {
        read("state.json").context("Cannot read state.json; existing data was left untouched")
    } else {
        Ok(State::default())
    }
}
pub fn save(s: &State) -> Result<()> {
    write("state.json", s)
}
