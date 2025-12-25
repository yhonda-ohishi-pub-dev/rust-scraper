# scraper-service

ETC利用照会サービス（etc-meisai.jp）から利用明細CSVを自動ダウンロードするスクレイパー。
Router（router-service）からInProcessで呼び出される。

## アーキテクチャ

```
[router-service] → tower::Service → [scraper-service]
                    InProcess呼び出し
```

## 使用方法

```rust
use scraper_service::{ScraperService, ScrapeRequest};
use tower::Service;

#[tokio::main]
async fn main() {
    let mut service = ScraperService::new();

    let request = ScrapeRequest::new("user_id", "password")
        .with_download_path("./downloads")
        .with_headless(false);

    let result = service.call(request).await.unwrap();
    println!("CSV downloaded: {:?}", result.csv_path);
}
```

## Scraper Trait

```rust
#[async_trait]
pub trait Scraper: Send + Sync {
    async fn initialize(&mut self) -> Result<(), ScraperError>;
    async fn login(&mut self) -> Result<(), ScraperError>;
    async fn download(&mut self) -> Result<PathBuf, ScraperError>;
    async fn close(&mut self) -> Result<(), ScraperError>;
}
```

## 設定

```rust
let config = ScraperConfig::new("user_id", "password")
    .with_download_path("./downloads")
    .with_headless(true)  // ヘッドレスモード
    .with_timeout(Duration::from_secs(60));
```

## 依存クレート

- `chromiumoxide`: Chrome DevTools Protocol クライアント（async/await対応）
- `tokio`: 非同期ランタイム
- `tower`: Service trait実装
- `async-trait`: 非同期トレイト
- `thiserror`: エラー型定義

## 注意事項

- Chromeがインストールされている必要があります
- `headless=false` でデバッグ可能
- ダウンロードタイムアウト: 30秒
- ファイル名はユーザーID付きでリネームされます

## ライセンス

MIT
