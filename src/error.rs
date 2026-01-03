use thiserror::Error;

#[derive(Error, Debug)]
pub enum ScraperError {
    #[error("ブラウザ初期化エラー: {0}")]
    BrowserInit(String),

    #[error("ナビゲーションエラー: {0}")]
    Navigation(String),

    #[error("ログインエラー: {0}")]
    Login(String),

    #[error("ダウンロードエラー: {0}")]
    Download(String),

    #[error("タイムアウト: {0}")]
    Timeout(String),

    #[error("要素が見つかりません: {0}")]
    ElementNotFound(String),

    #[error("ファイル操作エラー: {0}")]
    FileIO(#[from] std::io::Error),

    #[error("明細データなし: {0}")]
    NoUsageData(String),
}
