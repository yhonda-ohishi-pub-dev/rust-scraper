//! Dtakolog 関連の型定義

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Vehicleデータ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VehicleData {
    #[serde(rename = "VehicleCD")]
    pub vehicle_cd: String,
    #[serde(rename = "VehicleName")]
    pub vehicle_name: String,
    #[serde(rename = "Status")]
    pub status: String,
    #[serde(rename = "Metadata")]
    pub metadata: HashMap<String, String>,
}

/// 生データ (JSON形式で保持)
pub type DtakologData = Vec<serde_json::Value>;

/// gRPC送信結果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcResponse {
    pub success: bool,
    pub records_added: i32,
    pub total_records: i32,
    pub message: String,
}

/// Dtakolog スクレイプ結果
#[derive(Debug, Clone)]
pub struct DtakologResult {
    /// 抽出したVehicleデータ
    pub vehicles: Vec<VehicleData>,
    /// 生のJSONデータ
    pub raw_data: DtakologData,
    /// 新規/更新されたセッションID
    pub session_id: String,
    /// gRPC送信結果（送信した場合）
    pub grpc_response: Option<GrpcResponse>,
    /// 映像通知結果（mp4 URL付き）
    pub video_notifications: Vec<VideoNotificationResult>,
}

/// 映像通知結果（rust-logi送信用、mp4 URL付き）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoNotificationResult {
    pub vehicle_cd: i64,
    pub vehicle_name: String,
    pub serial_no: String,
    pub file_name: String,
    pub event_type: String,
    pub dvr_datetime: String,
    pub driver_name: String,
    pub mp4_url: String,
}

/// 映像通知データ（Monitoring_DvrNotification2 の結果）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DvrNotification {
    #[serde(rename = "VehicleCD")]
    pub vehicle_cd: i64,
    #[serde(rename = "VehicleName")]
    pub vehicle_name: String,
    #[serde(rename = "SerialNo")]
    pub serial_no: String,
    #[serde(rename = "FileName")]
    pub file_name: String,
    #[serde(rename = "FilePath")]
    pub file_path: String,
    #[serde(rename = "EventType")]
    pub event_type: String,
    #[serde(rename = "DvrDatetime")]
    pub dvr_datetime: String,
    #[serde(rename = "DriverName")]
    pub driver_name: String,
}

/// DVRファイル情報（Request_DvrFileList の結果）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DvrFileInfo {
    #[serde(rename = "FilePath")]
    pub file_path: String,
    #[serde(rename = "FileName")]
    pub file_name: String,
}

/// Dtakolog スクレイプ設定
#[derive(Debug, Clone)]
pub struct DtakologConfig {
    /// 会社ID
    pub comp_id: String,
    /// ユーザー名
    pub user_name: String,
    /// パスワード
    pub user_pass: String,
    /// ブランチID (デフォルト: "00000000")
    pub branch_id: String,
    /// フィルターID (デフォルト: "0")
    pub filter_id: String,
    /// ヘッドレスモード
    pub headless: bool,
    /// デバッグモード
    pub debug: bool,
    /// セッションTTL（秒）
    pub session_ttl_secs: u64,
    /// gRPC URL (例: "http://localhost:50051")
    pub grpc_url: Option<String>,
    /// gRPC組織ID
    pub grpc_organization_id: Option<String>,
}

impl Default for DtakologConfig {
    fn default() -> Self {
        Self {
            comp_id: String::new(),
            user_name: String::new(),
            user_pass: String::new(),
            branch_id: "00000000".to_string(),
            filter_id: "0".to_string(),
            headless: true,
            debug: false,
            session_ttl_secs: 3600,
            grpc_url: None,
            grpc_organization_id: None,
        }
    }
}
