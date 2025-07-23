use serde::{Deserialize, Serialize};
use std::path::Path;
use crate::performance::TvcPerformanceLevel;
use crate::error::{Result, VoteMonitorError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceFilterConfig {
    pub enabled: bool,
    pub min_latency_threshold: Option<u64>,
    pub max_latency_threshold: Option<u64>,
    pub min_tvc_threshold: Option<u64>,
    pub max_tvc_threshold: Option<u64>,
    pub performance_levels: Vec<String>,
}

impl Default for PerformanceFilterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_latency_threshold: Some(1),
            max_latency_threshold: None,
            min_tvc_threshold: None,
            max_tvc_threshold: Some(15),
            performance_levels: vec!["poor".to_string(), "critical".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub grpc_url: String,
    pub vote_account: String,
    pub performance_logging: PerformanceFilterConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            grpc_url: String::new(),
            vote_account: String::new(),
            performance_logging: PerformanceFilterConfig::default(),
        }
    }
}

impl Config {
    pub async fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = tokio::fs::read_to_string(path).await?;
        let config: Config = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    pub async fn load_or_default<P: AsRef<Path>>(path: P) -> Self {
        match Self::load_from_file(path).await {
            Ok(config) => {
                log::info!("configuration loaded from config.toml");
                config
            }
            Err(e) => {
                log::warn!("failed to load config.toml ({}), using defaults", e);
                Self::default()
            }
        }
    }

    pub async fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        self.validate()?;
        let content = toml::to_string_pretty(self)?;
        tokio::fs::write(path, content).await?;
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        // validate grpc_url
        if self.grpc_url.is_empty() {
            return Err(VoteMonitorError::Config("grpc_url cannot be empty".to_string()));
        }
        
        // validate vote_account
        if self.vote_account.is_empty() {
            return Err(VoteMonitorError::Config("vote_account cannot be empty".to_string()));
        }
        
        if self.vote_account.len() < 32 || self.vote_account.len() > 44 {
            return Err(VoteMonitorError::Config(
                "vote_account appears to be invalid (should be 32-44 characters)".to_string()
            ));
        }
        
        // validate performance logging settings
        let perf = &self.performance_logging;
        
        if let (Some(min), Some(max)) = (perf.min_latency_threshold, perf.max_latency_threshold) {
            if min > max {
                return Err(VoteMonitorError::Config(
                    format!("min_latency_threshold ({}) > max_latency_threshold ({})", min, max)
                ));
            }
        }
        
        if let (Some(min), Some(max)) = (perf.min_tvc_threshold, perf.max_tvc_threshold) {
            if min > max {
                return Err(VoteMonitorError::Config(
                    format!("min_tvc_threshold ({}) > max_tvc_threshold ({})", min, max)
                ));
            }
        }
        
        if let Some(tvc) = perf.max_tvc_threshold {
            if tvc > 16 {
                return Err(VoteMonitorError::Config(
                    format!("max_tvc_threshold ({}) cannot exceed 16", tvc)
                ));
            }
        }
        
        if let Some(tvc) = perf.min_tvc_threshold {
            if tvc == 0 {
                return Err(VoteMonitorError::Config(
                    "min_tvc_threshold cannot be 0".to_string()
                ));
            }
        }
        
        for level in &perf.performance_levels {
            match level.to_lowercase().as_str() {
                "optimal" | "good" | "fair" | "poor" | "critical" => {},
                _ => return Err(VoteMonitorError::Config(
                    format!("invalid performance level: '{}'. valid levels: optimal, good, fair, poor, critical", level)
                )),
            }
        }
        
        Ok(())
    }
}

impl PerformanceFilterConfig {
    // criteria for logging
    pub fn should_save_vote(&self, latency: u64, tvc_credits: u64, performance_level: TvcPerformanceLevel) -> bool {
        if !self.enabled {
            return false;
        }

        if let Some(min_latency) = self.min_latency_threshold {
            if latency < min_latency {
                return false;
            }
        }

        if let Some(max_latency) = self.max_latency_threshold {
            if latency > max_latency {
                return false;
            }
        }

        if let Some(min_tvc) = self.min_tvc_threshold {
            if tvc_credits < min_tvc {
                return false;
            }
        }

        if let Some(max_tvc) = self.max_tvc_threshold {
            if tvc_credits > max_tvc {
                return false;
            }
        }

        if !self.performance_levels.is_empty() {
            let level_str = performance_level.as_str();
            if !self.performance_levels.iter().any(|level| level.to_lowercase() == level_str.to_lowercase()) {
                return false;
            }
        }

        true
    }

    pub fn describe_filters(&self) -> String {
        if !self.enabled {
            return "disabled".to_string();
        }
        
        let mut filters = Vec::new();
        
        if let Some(min) = self.min_latency_threshold {
            filters.push(format!("latency >= {}", min));
        }
        if let Some(max) = self.max_latency_threshold {
            filters.push(format!("latency <= {}", max));
        }
        if let Some(min) = self.min_tvc_threshold {
            filters.push(format!("tvc >= {}", min));
        }
        if let Some(max) = self.max_tvc_threshold {
            filters.push(format!("tvc <= {}", max));
        }
        if !self.performance_levels.is_empty() {
            filters.push(format!("levels: [{}]", self.performance_levels.join(", ")));
        }
        
        if filters.is_empty() {
            "all votes".to_string()
        } else {
            filters.join(", ")
        }
    }
}