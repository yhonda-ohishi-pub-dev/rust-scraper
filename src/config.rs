use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ScraperConfig {
    pub user_id: String,
    pub password: String,
    pub download_path: PathBuf,
    pub headless: bool,
    pub timeout: Duration,
}

impl Default for ScraperConfig {
    fn default() -> Self {
        Self {
            user_id: String::new(),
            password: String::new(),
            download_path: PathBuf::from("./downloads"),
            headless: true,
            timeout: Duration::from_secs(60),
        }
    }
}

impl ScraperConfig {
    pub fn new(user_id: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            password: password.into(),
            ..Default::default()
        }
    }

    pub fn with_download_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.download_path = path.into();
        self
    }

    pub fn with_headless(mut self, headless: bool) -> Self {
        self.headless = headless;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}
