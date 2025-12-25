# CLAUDE.md - scraper-service 引継ぎドキュメント

## プロジェクト概要

ETC利用照会サービス（etc-meisai.jp）から利用明細CSVを自動ダウンロードするスクレイパー。
Router（router-service）からInProcessで呼び出される。

**参照元Go版:** https://github.com/yhonda-ohishi-pub-dev/scrape-vm

## 現在の状態

### 完了
- [x] Phase S1: 基盤構築（Cargo.toml, traits.rs, config.rs, error.rs）
- [x] Phase S2: ETC Scraper実装（chromiumoxide使用）
- [x] Phase S3: tower::Service実装
- [x] ビルド・テスト通過
- [x] GitHubプッシュ済み: https://github.com/yhonda-ohishi-pub-dev/rust-scraper
- [x] ログイン処理: JavaScriptで直接値設定する方式に修正済み
- [x] 検索処理: 検索条件指定→検索ボタンクリックまで動作確認済み
- [x] CSVリンククリック: 動作確認済み

### 未完了・問題点
- **confirmダイアログの処理が必要**
  - CSVリンクをクリックすると「全ての明細を対象に発行／出力します。よろしいですか？」というconfirmダイアログが表示される
  - このダイアログでOKをクリックする必要がある
  - Go版では `page.HandleJavaScriptDialog(true)` で自動処理している
  - chromiumoxideで `EventJavascriptDialogOpening` イベントをハンドルして `HandleJavaScriptDialogParams` で応答する必要がある

## 次のタスク

1. **JavaScriptダイアログハンドラの追加** (`src/etc/scraper.rs`)
   - importは追加済み:
     ```rust
     use chromiumoxide::cdp::browser_protocol::page::{EventJavascriptDialogOpening, HandleJavaScriptDialogParams};
     ```
   - initialize()でページ作成後にダイアログイベントリスナーを追加
   - ダイアログが開いたら自動的にOKをクリック

2. **テスト実行**
   ```bash
   ETC_USERNAME=ohishiexp ETC_PASSWORD=ohishi11 cargo run --example scrape_test
   ```

## 技術選定

### chromiumoxide を選択した理由
- `headless_chrome`は`ring`クレートに依存し、Windows環境でgccビルドエラーが発生
- `chromiumoxide`は`native-tls`フィーチャで`ring`依存を回避可能
- async/awaitネイティブ対応

### Cargo.toml 重要設定
```toml
chromiumoxide = { version = "0.8", default-features = false, features = ["tokio-runtime", "_fetcher-native-tokio"] }
```

## ディレクトリ構成

```
c:\rust\rust-scraper\
├── Cargo.toml
├── README.md
├── examples/
│   └── scrape_test.rs    # 実機テスト用（環境変数で認証情報を指定）
└── src/
    ├── lib.rs            # ライブラリエントリーポイント
    ├── traits.rs         # Scraper trait定義
    ├── config.rs         # ScraperConfig
    ├── error.rs          # ScraperError
    ├── service.rs        # tower::Service実装
    └── etc/
        ├── mod.rs
        └── scraper.rs    # ETC Scraper実装 ← ダイアログハンドラを追加
```

## Go版の参考コード

Go版 `scrapers/etc.go` のダイアログ処理:
```go
// ターゲットレベルのイベント（ダイアログ等）
chromedp.ListenTarget(s.Ctx, func(ev interface{}) {
    switch e := ev.(type) {
    case *page.EventJavascriptDialogOpening:
        s.Logger.Printf("Dialog: %s", e.Message)
        go chromedp.Run(s.Ctx, page.HandleJavaScriptDialog(true))
    }
})
```

## コマンド

```bash
# ビルド
cargo build

# テスト
cargo test

# 実機テスト（Chrome表示モード）
ETC_USERNAME=ohishiexp ETC_PASSWORD=ohishi11 cargo run --example scrape_test
```

## 関連プロジェクト

- **router-service**: `C:\rust\rust-router\router-service\`
  - このscraper-serviceを呼び出す側
  - Cargo.tomlで `scraper-service = { git = "..." }` として参照予定
