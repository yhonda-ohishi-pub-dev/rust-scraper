//! Dtakolog スクレイパーテスト
//!
//! 実行方法:
//! ```
//! cargo run -p scraper-service --example dtakolog_test
//! ```

use scraper_service::dtakolog::{DtakologConfig, DtakologScraper};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ログ設定
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // .envがあれば読み込む
    if let Ok(env_path) = std::fs::canonicalize(".env") {
        println!("Loading .env from: {:?}", env_path);
        for line in std::fs::read_to_string(".env")?.lines() {
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim().trim_matches('\'').trim_matches('"');
                if !key.starts_with('#') && !key.is_empty() {
                    std::env::set_var(key, value);
                }
            }
        }
    }

    // 環境変数から認証情報を取得
    let comp_id = std::env::var("COMP_ID").expect("COMP_ID not set");
    let user_name = std::env::var("USER_NAME").expect("USER_NAME not set");
    let user_pass = std::env::var("USER_PASS").expect("USER_PASS not set");

    println!("=== Dtakolog Scraper Test ===");
    println!("Company ID: {}", comp_id);
    println!("User Name: {}", user_name);
    println!("Headless: false (visible browser)");
    println!();

    // 設定を作成
    let config = DtakologConfig {
        comp_id,
        user_name,
        user_pass,
        headless: false, // ブラウザを表示
        debug: true,
        ..Default::default()
    };

    // スクレイパーを初期化
    let mut scraper = DtakologScraper::new(config);

    println!("Initializing browser...");
    scraper.initialize().await?;

    println!("Starting scrape...");
    let result = scraper.scrape(None, true).await?;

    println!();
    println!("=== Results ===");
    println!("Session ID: {}", result.session_id);
    println!("Vehicles found: {}", result.vehicles.len());
    println!();

    // 最初の5件を表示
    for (i, vehicle) in result.vehicles.iter().take(5).enumerate() {
        println!(
            "{}. {} ({}) - Status: {}",
            i + 1,
            vehicle.vehicle_name,
            vehicle.vehicle_cd,
            vehicle.status
        );
    }

    if result.vehicles.len() > 5 {
        println!("... and {} more", result.vehicles.len() - 5);
    }

    // ブラウザを閉じる
    scraper.close().await?;

    println!();
    println!("Test completed successfully!");

    Ok(())
}
