//! スクレイパーライブラリ
//!
//! - ETC利用照会サービスからCSVをダウンロード
//! - Dtakolog (Vehicle) データを取得してgRPC送信
//!
//! # ETC スクレイパー使用例
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
//!
//! # Dtakolog スクレイパー使用例
//!
//! ```rust,ignore
//! use scraper_service::dtakolog::{DtakologScraper, DtakologConfig};
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = DtakologConfig {
//!         comp_id: "company".to_string(),
//!         user_name: "user".to_string(),
//!         user_pass: "pass".to_string(),
//!         ..Default::default()
//!     };
//!
//!     let mut scraper = DtakologScraper::new(config);
//!     scraper.initialize().await.unwrap();
//!     let result = scraper.scrape(None, false).await.unwrap();
//!     println!("Vehicles: {:?}", result.vehicles.len());
//! }
//! ```

pub mod config;
pub mod dtakolog;
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

// Dtakolog 関連の型もリエクスポート
pub use dtakolog::{
    DtakologConfig, DtakologData, DtakologResult, DtakologScraper, GrpcResponse, VehicleData,
    VideoNotificationResult,
};
