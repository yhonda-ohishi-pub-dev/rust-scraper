use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tower::Service;
use tracing::info;

use crate::config::ScraperConfig;
use crate::error::ScraperError;
use crate::etc::EtcScraper;
use crate::traits::Scraper;

/// スクレイピングリクエスト
#[derive(Debug, Clone)]
pub struct ScrapeRequest {
    pub user_id: String,
    pub password: String,
    pub download_path: PathBuf,
    pub headless: bool,
}

impl ScrapeRequest {
    pub fn new(user_id: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            password: password.into(),
            download_path: PathBuf::from("./downloads"),
            headless: true,
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
}

impl From<ScrapeRequest> for ScraperConfig {
    fn from(req: ScrapeRequest) -> Self {
        ScraperConfig {
            user_id: req.user_id,
            password: req.password,
            download_path: req.download_path,
            headless: req.headless,
            timeout: Duration::from_secs(60),
        }
    }
}

/// スクレイピング結果
#[derive(Debug)]
pub struct ScrapeResult {
    pub csv_path: PathBuf,
    pub csv_content: Vec<u8>,
}

impl ScrapeResult {
    pub fn new(csv_path: PathBuf) -> std::io::Result<Self> {
        let csv_content = std::fs::read(&csv_path)?;
        Ok(Self {
            csv_path,
            csv_content,
        })
    }
}

/// tower::Serviceを実装したスクレイパーサービス
#[derive(Debug, Clone, Default)]
pub struct ScraperService {
    // 将来的な拡張用（レートリミット、キャッシュなど）
}

impl ScraperService {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Service<ScrapeRequest> for ScraperService {
    type Response = ScrapeResult;
    type Error = ScraperError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: ScrapeRequest) -> Self::Future {
        info!("スクレイピングリクエスト受信: user_id={}", req.user_id);

        Box::pin(async move {
            let config: ScraperConfig = req.into();
            let mut scraper = EtcScraper::new(config);

            // スクレイピング実行
            let csv_path = scraper.execute().await?;

            // 結果を作成
            let result = ScrapeResult::new(csv_path)?;

            info!(
                "スクレイピング完了: path={:?}, size={}bytes",
                result.csv_path,
                result.csv_content.len()
            );

            Ok(result)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scrape_request_builder() {
        let req = ScrapeRequest::new("user", "pass")
            .with_download_path("/tmp/dl")
            .with_headless(false);

        assert_eq!(req.user_id, "user");
        assert_eq!(req.password, "pass");
        assert_eq!(req.download_path, PathBuf::from("/tmp/dl"));
        assert!(!req.headless);
    }

    #[test]
    fn test_scrape_request_to_config() {
        let req = ScrapeRequest::new("user", "pass");
        let config: ScraperConfig = req.into();

        assert_eq!(config.user_id, "user");
        assert_eq!(config.password, "pass");
    }
}
