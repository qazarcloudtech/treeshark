use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub min_size: String,
    pub scan_paths: Vec<String>,
    #[serde(default = "default_top_n")]
    pub top_n: usize,
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Number of threads (0 = all available cores)
    #[serde(default)]
    pub threads: usize,
}

fn default_top_n() -> usize {
    50
}

impl Default for Config {
    fn default() -> Self {
        Self {
            min_size: "1GB".to_string(),
            scan_paths: vec![".".to_string()],
            top_n: 50,
            exclude: vec![
                ".git".to_string(),
                "node_modules".to_string(),
                ".cache".to_string(),
            ],
            threads: 0,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config: {}", path.display()))?;
        let config: Config =
            serde_yaml::from_str(&content).with_context(|| "Failed to parse YAML config")?;
        Ok(config)
    }

    pub fn min_size_bytes(&self) -> Result<u64> {
        parse_size(&self.min_size)
    }

    pub fn effective_threads(&self) -> usize {
        if self.threads == 0 {
            num_cpus::get()
        } else {
            self.threads
        }
    }

    pub fn resolve_scan_paths(&self, config_dir: &Path) -> Vec<PathBuf> {
        self.scan_paths
            .iter()
            .map(|p| {
                let path = PathBuf::from(p);
                if path.is_absolute() {
                    path
                } else {
                    config_dir.join(path)
                }
            })
            .collect()
    }
}

pub fn parse_size(s: &str) -> Result<u64> {
    let s = s.trim().to_uppercase();
    let (num_str, multiplier) = if s.ends_with("TB") {
        (&s[..s.len() - 2], 1_u64 << 40)
    } else if s.ends_with("GB") {
        (&s[..s.len() - 2], 1_u64 << 30)
    } else if s.ends_with("MB") {
        (&s[..s.len() - 2], 1_u64 << 20)
    } else if s.ends_with("KB") {
        (&s[..s.len() - 2], 1_u64 << 10)
    } else if s.ends_with('B') {
        (&s[..s.len() - 1], 1_u64)
    } else {
        (s.as_str(), 1_u64)
    };

    let num: f64 = num_str
        .trim()
        .parse()
        .with_context(|| format!("Invalid size value: {}", num_str))?;

    Ok((num * multiplier as f64) as u64)
}
