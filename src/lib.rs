//! ETC利用照会サービスからCSVをダウンロードするスクレイパー
//!
//! # 使用例
//!
//! ```rust,ignore
//! use scraper_service::{ScraperService, ScrapeRequest};
//! use tower::Service;
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut service = ScraperService::new();
//!
//!     let request = ScrapeRequest::new("user_id", "password")
//!         .with_download_path("./downloads")
//!         .with_headless(false);
//!
//!     let result = service.call(request).await.unwrap();
//!     println!("CSV downloaded: {:?}", result.csv_path);
//! }
//! ```

pub mod config;
pub mod error;
pub mod etc;
pub mod service;
pub mod traits;

// 主要な型をリエクスポート
pub use config::ScraperConfig;
pub use error::ScraperError;
pub use etc::EtcScraper;
pub use service::{ScrapeRequest, ScrapeResult, ScraperService};
pub use traits::Scraper;
