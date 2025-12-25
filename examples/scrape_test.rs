use scraper_service::{EtcScraper, ScraperConfig, Scraper};
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    // ログ設定
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    // 環境変数から認証情報を取得
    let username = std::env::var("ETC_USERNAME")
        .expect("ETC_USERNAME environment variable not set");
    let password = std::env::var("ETC_PASSWORD")
        .expect("ETC_PASSWORD environment variable not set");

    let config = ScraperConfig::new(&username, &password)
        .with_download_path(PathBuf::from("./downloads"))
        .with_headless(false);  // デバッグ用に表示モード

    let mut scraper = EtcScraper::new(config);

    println!("=== ETC Scraper Test ===");

    match scraper.execute().await {
        Ok(path) => {
            println!("成功! CSV保存先: {:?}", path);
        }
        Err(e) => {
            eprintln!("エラー: {}", e);
        }
    }
}
