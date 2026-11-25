use crate::AdapterKind;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const DEFAULT_PORT: u16 = 4820;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to resolve config directory")]
    NoConfigDir,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse error: {0}")]
    TomlDe(#[from] toml::de::Error),
    #[error("toml serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomAgentConfig {
    pub id: String,
    pub name: String,
    pub kind: AdapterKind,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub default_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub bind_addr: String,
    pub port: u16,
    pub token: String,
    pub db_path: PathBuf,
    #[serde(default)]
    pub custom_agents: Vec<CustomAgentConfig>,
}

impl DaemonConfig {
    pub fn config_dir() -> Result<PathBuf, ConfigError> {
        let base = dirs::config_dir().ok_or(ConfigError::NoConfigDir)?;
        Ok(base.join("hasilanx"))
    }

    pub fn data_dir() -> Result<PathBuf, ConfigError> {
        let base = dirs::data_local_dir()
            .or_else(dirs::data_dir)
            .ok_or(ConfigError::NoConfigDir)?;
        Ok(base.join("hasilanx"))
    }

    pub fn default_config_path() -> Result<PathBuf, ConfigError> {
        Ok(Self::config_dir()?.join("daemon.toml"))
    }

    pub fn default_db_path() -> Result<PathBuf, ConfigError> {
        Ok(Self::data_dir()?.join("hasilanx.db"))
    }

    pub fn generate_token() -> String {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        hex::encode(bytes)
    }

    pub fn load_or_create(path: Option<&Path>) -> Result<Self, ConfigError> {
        let config_path = match path {
            Some(p) => p.to_path_buf(),
            None => Self::default_config_path()?,
        };

        if config_path.exists() {
            let content = fs::read_to_string(&config_path)?;
            let cfg: DaemonConfig = toml::from_str(&content)?;
            return Ok(cfg);
        }

        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let db_path = Self::default_db_path()?;
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let cfg = DaemonConfig {
            bind_addr: "127.0.0.1".to_string(),
            port: DEFAULT_PORT,
            token: Self::generate_token(),
            db_path,
            custom_agents: Vec::new(),
        };

        let serialized = toml::to_string_pretty(&cfg)?;
        fs::write(&config_path, serialized)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600));
        }

        Ok(cfg)
    }

    pub fn base_http_url(&self) -> String {
        let host = if self.bind_addr == "0.0.0.0" || self.bind_addr == "::" {
            "127.0.0.1"
        } else {
            &self.bind_addr
        };
        format!("http://{}:{}", host, self.port)
    }

    pub fn base_ws_url(&self) -> String {
        let host = if self.bind_addr == "0.0.0.0" || self.bind_addr == "::" {
            "127.0.0.1"
        } else {
            &self.bind_addr
        };
        format!("ws://{}:{}/api/v1/ws", host, self.port)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteHostConfig {
    pub id: String,
    pub name: String,
    /// e.g. "http://100.64.0.12:4820"
    pub http_url: String,
    pub token: String,
}

impl RemoteHostConfig {
    pub fn ws_url(&self) -> String {
        let trimmed = self.http_url.trim_end_matches('/');
        if let Some(rest) = trimmed.strip_prefix("https://") {
            format!("wss://{rest}/api/v1/ws")
        } else if let Some(rest) = trimmed.strip_prefix("http://") {
            format!("ws://{rest}/api/v1/ws")
        } else {
            format!("ws://{trimmed}/api/v1/ws")
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostsConfig {
    #[serde(default)]
    pub remotes: Vec<RemoteHostConfig>,
}

impl HostsConfig {
    pub fn default_path() -> Result<PathBuf, ConfigError> {
        Ok(DaemonConfig::config_dir()?.join("hosts.toml"))
    }

    pub fn load_or_default() -> Result<Self, ConfigError> {
        let path = Self::default_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)?;
        Ok(toml::from_str(&raw)?)
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        let path = Self::default_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let serialized = toml::to_string_pretty(self)?;
        fs::write(&path, serialized)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}
