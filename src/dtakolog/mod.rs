//! Dtakolog スクレイパーモジュール
//!
//! Vehicleデータを取得してgRPC経由でrust-logiに送信する

mod scraper;
mod types;

pub use scraper::DtakologScraper;
pub use types::{
    DtakologConfig, DtakologData, DtakologResult, GrpcResponse, VehicleData,
    VideoNotificationResult,
};
