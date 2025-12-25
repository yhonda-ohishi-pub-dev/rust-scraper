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

### 未完了・問題点
- **download処理でエラー**: ログイン後の「検索ボタン」が見つからない
  - エラー: `Could not find node with given id`
  - ログインまでは成功している
  - Go版と同様のJavaScriptベースの要素操作に修正が必要

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
│   └── scrape_test.rs    # 実機テスト用
└── src/
    ├── lib.rs            # ライブラリエントリーポイント
    ├── traits.rs         # Scraper trait定義
    ├── config.rs         # ScraperConfig
    ├── error.rs          # ScraperError
    ├── service.rs        # tower::Service実装
    └── etc/
        ├── mod.rs
        └── scraper.rs    # ETC Scraper実装 ← ここを修正
```

## 次のタスク

1. **download()メソッドの修正** (`src/etc/scraper.rs`)
   - Go版（scrapers/etc.go）のJavaScriptベースのフローを参考に修正
   - ページ遷移後の待機時間を調整
   - 要素セレクタをGo版に合わせる

2. **テスト実行**
   ```bash
   ETC_USERNAME=your_user ETC_PASSWORD=your_pass cargo run --example scrape_test
   ```

## Go版の参考コード

Go版 `scrapers/etc.go` のdownload処理:
```go
// JavaScriptで「検索条件の指定」リンクをクリック
chromedp.Evaluate(`
    (function() {
        var links = document.querySelectorAll('a');
        for (var i = 0; i < links.length; i++) {
            if (links[i].textContent.indexOf('検索条件の指定') >= 0) {
                links[i].click();
                return true;
            }
        }
        return false;
    })()
`, nil)

// 検索ボタンはセレクタではなくJavaScript評価で
chromedp.Click(`input[name='focusTarget']`, chromedp.NodeVisible)
```

## コマンド

```bash
# ビルド
cargo build

# テスト
cargo test

# 実機テスト（Chrome起動）
cargo run --example scrape_test
```

## 関連プロジェクト

- **router-service**: `C:\rust\rust-router\router-service\`
  - このscraper-serviceを呼び出す側
  - Cargo.tomlで `scraper-service = { git = "..." }` として参照予定
