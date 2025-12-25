use async_trait::async_trait;
use std::path::PathBuf;

use crate::error::ScraperError;

#[async_trait]
pub trait Scraper: Send + Sync {
    /// ブラウザ初期化
    async fn initialize(&mut self) -> Result<(), ScraperError>;

    /// ログイン実行
    async fn login(&mut self) -> Result<(), ScraperError>;

    /// CSVダウンロード
    async fn download(&mut self) -> Result<PathBuf, ScraperError>;

    /// リソース解放
    async fn close(&mut self) -> Result<(), ScraperError>;

    /// 一括実行（initialize → login → download → close）
    async fn execute(&mut self) -> Result<PathBuf, ScraperError> {
        self.initialize().await?;
        self.login().await?;
        let path = self.download().await?;
        self.close().await?;
        Ok(path)
    }
}
