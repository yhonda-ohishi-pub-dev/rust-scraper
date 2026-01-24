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

    // Dtakolog 固有のエラー
    #[error("データ抽出エラー: {0}")]
    Extraction(String),

    #[error("JavaScript実行エラー: {0}")]
    JavaScript(String),

    #[error("セッションエラー: {0}")]
    Session(String),

    #[error("gRPCエラー: {0}")]
    Grpc(String),

    #[error("gRPC接続失敗（リトライ{retries}回後）: {message}")]
    GrpcConnectionFailed { retries: u32, message: String },

    #[error("JSONシリアライズエラー: {0}")]
    Json(String),
}

impl ScraperError {
    /// リトライ可能なエラーかどうか
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ScraperError::Navigation(_)
                | ScraperError::Timeout(_)
                | ScraperError::Grpc(_)
                | ScraperError::BrowserInit(_)
        )
    }
}
