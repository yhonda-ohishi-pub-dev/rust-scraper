use scraper_service::{EtcScraper, ScraperConfig, Scraper};
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    // ログ設定
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    // 環境変数から認証情報を取得（JSON形式）
    // 例: ETC_ACCOUNTS='[{"user_id":"user1","password":"pass1"},{"user_id":"user2","password":"pass2"}]'
    let accounts_json = std::env::var("ETC_ACCOUNTS")
        .expect("ETC_ACCOUNTS environment variable not set");

    let accounts: Vec<serde_json::Value> = serde_json::from_str(&accounts_json)
        .expect("Failed to parse ETC_ACCOUNTS JSON");

    println!("=== ETC Scraper Multi-Account Test ===\n");

    for (i, account) in accounts.iter().enumerate() {
        let username = account["user_id"].as_str().expect("user_id not found");
        let password = account["password"].as_str().expect("password not found");

        println!("--- Account {}: {} ---", i + 1, username);

        let config = ScraperConfig::new(username, password)
            .with_download_path(PathBuf::from("./downloads"))
            .with_headless(false);  // デバッグ用に表示モード

        let mut scraper = EtcScraper::new(config);

        match scraper.execute().await {
            Ok(path) => {
                println!("✓ 成功! CSV保存先: {:?}", path);
            }
            Err(e) => {
                eprintln!("✗ エラー: {}", e);
            }
        }

        println!();
    }

    println!("=== テスト完了 ===");
}
