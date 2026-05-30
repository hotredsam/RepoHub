//! Runtime configuration. Defaults to ~/RepoHub as the storage root;
//! overridable via env vars and (later) the Settings tab / config.json.

use std::net::Ipv4Addr;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    /// Address the API binds to. Local-only by default (127.0.0.1).
    pub bind_host: Ipv4Addr,
    pub port: u16,
    /// Umbrella storage folder (clones, db, config).
    pub root: PathBuf,
    /// How often the scheduler fetches tracked repos.
    pub fetch_interval_secs: u64,
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let root = match std::env::var("REPOHUB_ROOT") {
            Ok(p) => PathBuf::from(p),
            Err(_) => dirs_home()?.join("RepoHub"),
        };
        std::fs::create_dir_all(root.join("repos"))?;
        std::fs::create_dir_all(root.join("data"))?;

        let port = std::env::var("REPOHUB_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8787);

        let fetch_interval_secs = std::env::var("REPOHUB_FETCH_INTERVAL_SECS")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(300);

        Ok(Self {
            bind_host: Ipv4Addr::LOCALHOST,
            port,
            root,
            fetch_interval_secs,
        })
    }

    pub fn repos_dir(&self) -> PathBuf {
        self.root.join("repos")
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join("data").join("repohub.db")
    }
}

fn dirs_home() -> anyhow::Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| anyhow::anyhow!("HOME not set"))
}
