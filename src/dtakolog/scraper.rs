//! Dtakolog スクレイパー実装
//!
//! Vehicleデータを取得してgRPC経由でrust-logiに送信する

use std::collections::HashMap;
use std::time::Duration;

use chrono::{offset::FixedOffset, Utc};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::network::CookieParam;
use chromiumoxide::page::ScreenshotParams;
use chromiumoxide::Page;
use futures::StreamExt;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::error::ScraperError;

use super::types::{DtakologConfig, DtakologData, DtakologResult, GrpcResponse, VehicleData};

/// リトライ設定
const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF_MS: u64 = 1000;

/// Dtakolog スクレイパー
pub struct DtakologScraper {
    config: DtakologConfig,
    browser: Option<Browser>,
}

impl DtakologScraper {
    /// 新しいスクレイパーを作成
    pub fn new(config: DtakologConfig) -> Self {
        Self {
            config,
            browser: None,
        }
    }

    /// ブラウザを初期化
    pub async fn initialize(&mut self) -> Result<(), ScraperError> {
        info!("Initializing browser for dtakolog scraper...");

        // ユニークなユーザーデータディレクトリを生成
        let unique_id = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let user_data_dir = std::env::temp_dir().join(format!("dtakolog-{}", unique_id));

        // Chrome パスを取得
        let chrome_path = std::env::var("CHROME_PATH")
            .or_else(|_| std::env::var("CHROMIUM_PATH"))
            .unwrap_or_else(|_| "chromium".to_string());

        // ブラウザ設定を構築
        let mut builder = BrowserConfig::builder()
            .chrome_executable(chrome_path)
            .user_data_dir(&user_data_dir);

        if !self.config.headless {
            builder = builder.with_head();
        }

        builder = builder
            .no_sandbox()
            .arg("--disable-blink-features=AutomationControlled")
            .arg("--disable-dev-shm-usage")
            .arg("--disable-gpu");

        if self.config.debug {
            builder = builder.arg("--enable-logging=stderr").arg("--v=1");
        }

        let browser_config = builder
            .build()
            .map_err(|e| ScraperError::BrowserInit(e.to_string()))?;

        // ブラウザを起動
        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| ScraperError::BrowserInit(e.to_string()))?;

        // ハンドラータスクを起動
        tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                debug!("Browser event: {:?}", event);
            }
        });

        self.browser = Some(browser);
        info!("Browser initialized successfully");

        Ok(())
    }

    /// Vehicleデータを取得
    pub async fn scrape(
        &self,
        session_cookies: Option<Vec<(String, String, String, String)>>, // (name, value, domain, path)
        force_login: bool,
    ) -> Result<DtakologResult, ScraperError> {
        info!("Starting dtakolog scrape...");

        let browser = self
            .browser
            .as_ref()
            .ok_or_else(|| ScraperError::BrowserInit("Browser not initialized".to_string()))?;

        // 新しいページを作成
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| ScraperError::BrowserInit(e.to_string()))?;

        // セッションクッキーを復元
        if let Some(cookies) = session_cookies {
            if !force_login {
                for (name, value, domain, path) in cookies {
                    let cookie_param = CookieParam::builder()
                        .name(&name)
                        .value(&value)
                        .domain(&domain)
                        .path(&path)
                        .build();

                    if let Ok(param) = cookie_param {
                        if let Err(e) = page.set_cookie(param).await {
                            debug!("Failed to set cookie: {}", e);
                        }
                    }
                }
            }
        }

        // メインページにナビゲーション試行
        let session_id = match self.navigate_to_main(&page).await {
            Ok(_) => {
                info!("Navigation successful without login");
                format!("session_{}", Utc::now().timestamp())
            }
            Err(e) => {
                info!("First navigation failed, attempting login: {}", e);
                let sid = self.login(&page).await?;
                self.navigate_to_main(&page).await?;
                sid
            }
        };

        // データを抽出
        let (vehicles, raw_data) = self.extract_vehicle_data(&page).await?;

        // データをファイルに保存
        self.save_raw_data(&raw_data).await;

        // gRPC送信（設定がある場合）
        let grpc_response = if self.config.grpc_url.is_some() {
            match self.send_to_grpc_with_retry(&raw_data).await {
                Ok(resp) => Some(resp),
                Err(e) => {
                    warn!("Failed to send to gRPC: {}", e);
                    None
                }
            }
        } else {
            None
        };

        // ページを閉じる
        if let Err(e) = page.close().await {
            debug!("Failed to close page: {}", e);
        }

        Ok(DtakologResult {
            vehicles,
            raw_data,
            session_id,
            grpc_response,
        })
    }

    /// ログイン実行
    async fn login(&self, page: &Page) -> Result<String, ScraperError> {
        info!("Starting login process");
        info!(
            "Using credentials - Company: {}, User: {}",
            self.config.comp_id, self.config.user_name
        );

        // ログインページにナビゲート
        let login_url = "https://theearth-np.com/F-OES1010[Login].aspx?mode=timeout";
        page.goto(login_url)
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;

        sleep(Duration::from_secs(3)).await;

        // ログインフォームの存在確認
        let has_pass_field = page
            .evaluate("document.querySelector('#txtPass') !== null")
            .await
            .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

        if !has_pass_field.into_value::<bool>().unwrap_or(false) {
            return Err(ScraperError::Login("Login form not found".to_string()));
        }

        // ポップアップ処理
        if let Err(e) = page
            .evaluate(
                r#"
                const popup = document.querySelector('#popup_1');
                if (popup && popup.style.display !== 'none') {
                    popup.click();
                }
            "#,
            )
            .await
        {
            debug!("Failed to handle popup: {}", e);
        }

        sleep(Duration::from_secs(1)).await;

        // 認証情報を入力
        let fill_script = format!(
            r#"
            document.querySelector('#txtID2').value = '{}';
            document.querySelector('#txtID1').value = '{}';
            document.querySelector('#txtPass').value = '{}';
        "#,
            self.config.comp_id, self.config.user_name, self.config.user_pass
        );

        page.evaluate(fill_script.as_str())
            .await
            .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

        // デバッグスクリーンショット
        if self.config.debug {
            if let Ok(screenshot) = page
                .screenshot(ScreenshotParams::builder().full_page(true).build())
                .await
            {
                use base64::Engine;
                let encoded = base64::engine::general_purpose::STANDARD.encode(&screenshot);
                debug!("Login screenshot: data:image/png;base64,{}", encoded);
            }
        }

        // ログインボタンをクリック
        page.evaluate("document.querySelector('#imgLogin').click()")
            .await
            .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

        sleep(Duration::from_secs(5)).await;

        // ログイン成功確認
        let login_success = page
            .evaluate("document.querySelector('#Button1st_7') !== null")
            .await
            .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

        if !login_success.into_value::<bool>().unwrap_or(false) {
            // 既にログイン済みの場合
            let has_popup = page
                .evaluate("document.querySelector('#popup_1') !== null")
                .await
                .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

            if has_popup.into_value::<bool>().unwrap_or(false) {
                page.evaluate("document.querySelector('#popup_1').click()")
                    .await
                    .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

                sleep(Duration::from_secs(5)).await;
            } else {
                return Err(ScraperError::Login(
                    "Login verification failed".to_string(),
                ));
            }
        }

        let session_id = format!("session_{}", Utc::now().timestamp());
        info!("Login successful, session ID: {}", session_id);
        Ok(session_id)
    }

    /// メインページにナビゲート
    async fn navigate_to_main(&self, page: &Page) -> Result<(), ScraperError> {
        info!("Navigating to Venus Main page...");

        let main_url = "https://theearth-np.com/WebVenus/F-AAV0001[VenusMain].aspx";
        page.goto(main_url)
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;

        sleep(Duration::from_secs(5)).await;

        // 現在のURLを確認
        let current_url = page
            .evaluate("window.location.href")
            .await
            .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

        let url = current_url.into_value::<String>().unwrap_or_default();
        info!("Current URL: {}", url);

        if url.contains("Login") || url.contains("OES1010") {
            return Err(ScraperError::Session(
                "Redirected to login page - session expired".to_string(),
            ));
        }

        Ok(())
    }

    /// Vehicleデータを抽出
    async fn extract_vehicle_data(
        &self,
        page: &Page,
    ) -> Result<(Vec<VehicleData>, DtakologData), ScraperError> {
        // VenusBridgeService の存在確認
        let has_service = page
            .evaluate(
                r#"
                typeof VenusBridgeService !== 'undefined' &&
                typeof VenusBridgeService.VehicleStateTableForBranchEx === 'function'
            "#,
            )
            .await
            .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

        if !has_service.into_value::<bool>().unwrap_or(false) {
            return Err(ScraperError::Extraction(
                "VenusBridgeService not found".to_string(),
            ));
        }

        info!(
            "Calling VenusBridgeService with branchID='{}', filterID='{}'",
            self.config.branch_id, self.config.filter_id
        );

        sleep(Duration::from_secs(2)).await;

        // グリッドの出現を待機
        for i in 0..30 {
            let grid_exists = page
                .evaluate("document.querySelector('#igGrid-VenusMain-VehicleList') !== null")
                .await
                .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

            if grid_exists.into_value::<bool>().unwrap_or(false) {
                info!("Venus main grid detected");
                break;
            }

            if i % 5 == 0 {
                info!("Waiting for page structure... ({}/30)", i + 1);
            }
            sleep(Duration::from_secs(1)).await;
        }

        // ローディング表示の消失を待機
        for i in 0..30 {
            let has_loading = page
                .evaluate(
                    r#"
                    (() => {
                        const waitMsg = document.querySelector('#pMsg_wait, [id*="pMsg_wait"]');
                        if (!waitMsg) return false;
                        const style = window.getComputedStyle(waitMsg);
                        return style.display !== 'none' && style.visibility !== 'hidden';
                    })()
                "#,
                )
                .await
                .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

            if !has_loading.into_value::<bool>().unwrap_or(false) {
                break;
            }

            if i % 5 == 0 {
                info!("Loading message visible, waiting... ({}/30)", i + 1);
            }
            sleep(Duration::from_secs(1)).await;
        }

        sleep(Duration::from_secs(3)).await;

        // JavaScriptを実行してデータを取得
        let inject_script = format!(
            r#"
            window.__vehicleDataResult = null;
            window.__vehicleDataError = null;
            window.__vehicleDataCompleted = false;

            VenusBridgeService.VehicleStateTableForBranchEx('{}', '{}',
                (data) => {{
                    window.__vehicleDataResult = data;
                    window.__vehicleDataCompleted = true;
                }},
                (error) => {{
                    window.__vehicleDataError = error;
                    window.__vehicleDataCompleted = true;
                }}
            );
        "#,
            self.config.branch_id, self.config.filter_id
        );

        page.evaluate(inject_script.as_str())
            .await
            .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

        // 結果をポーリング
        info!("Waiting for vehicle data response...");
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(60);

        loop {
            if start.elapsed() > timeout {
                return Err(ScraperError::Timeout(format!(
                    "Vehicle data timeout after {:?}",
                    timeout
                )));
            }

            let completed = page
                .evaluate("window.__vehicleDataCompleted")
                .await
                .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

            if completed.into_value::<bool>().unwrap_or(false) {
                // エラーチェック
                let has_error = page
                    .evaluate("window.__vehicleDataError !== null")
                    .await
                    .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

                if has_error.into_value::<bool>().unwrap_or(false) {
                    let error_msg = page
                        .evaluate("window.__vehicleDataError")
                        .await
                        .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

                    return Err(ScraperError::Extraction(format!(
                        "Service error: {:?}",
                        error_msg.into_value::<String>()
                    )));
                }

                // 結果を取得
                let result = page
                    .evaluate("JSON.stringify(window.__vehicleDataResult)")
                    .await
                    .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

                let json_str = result.into_value::<String>().unwrap_or_default();
                info!("Got vehicle data after {:?}", start.elapsed());

                // JSONをパース
                let raw_data: DtakologData =
                    serde_json::from_str(&json_str).map_err(|e| ScraperError::Json(e.to_string()))?;

                // VehicleDataに変換
                let vehicles = self.parse_vehicle_data(&raw_data);
                info!("Extracted {} vehicles", vehicles.len());

                return Ok((vehicles, raw_data));
            }

            sleep(Duration::from_millis(100)).await;
        }
    }

    /// 生データをVehicleDataに変換
    fn parse_vehicle_data(&self, raw_data: &DtakologData) -> Vec<VehicleData> {
        raw_data
            .iter()
            .filter_map(|item| {
                let obj = item.as_object()?;

                let vehicle_cd = obj
                    .get("VehicleCD")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let vehicle_name = obj
                    .get("VehicleName")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let status = obj
                    .get("Status")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let mut metadata = HashMap::new();
                for (k, v) in obj {
                    if k != "VehicleCD" && k != "VehicleName" && k != "Status" {
                        metadata.insert(k.clone(), format!("{}", v));
                    }
                }

                Some(VehicleData {
                    vehicle_cd,
                    vehicle_name,
                    status,
                    metadata,
                })
            })
            .collect()
    }

    /// 生データをファイルに保存
    async fn save_raw_data(&self, raw_data: &DtakologData) {
        let jst = FixedOffset::east_opt(9 * 3600).unwrap();
        let timestamp = Utc::now().with_timezone(&jst).format("%Y%m%d_%H%M%S");
        let filename = format!("./data/vehicles_{}.json", timestamp);

        if let Err(e) = std::fs::create_dir_all("./data") {
            warn!("Failed to create data directory: {}", e);
            return;
        }

        match serde_json::to_string_pretty(raw_data) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&filename, json) {
                    error!("Failed to save vehicle data: {}", e);
                } else {
                    info!("Saved vehicle data to {}", filename);
                }
            }
            Err(e) => error!("Failed to serialize vehicle data: {}", e),
        }
    }

    /// リトライ付きでgRPCに送信
    async fn send_to_grpc_with_retry(
        &self,
        raw_data: &DtakologData,
    ) -> Result<GrpcResponse, ScraperError> {
        let mut last_error = None;

        for attempt in 0..MAX_RETRIES {
            match self.send_to_grpc(raw_data).await {
                Ok(resp) => return Ok(resp),
                Err(e) if e.is_retryable() => {
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt);
                    warn!(
                        "gRPC attempt {} failed, retrying in {}ms: {}",
                        attempt + 1,
                        backoff,
                        e
                    );
                    sleep(Duration::from_millis(backoff)).await;
                    last_error = Some(e);
                }
                Err(e) => return Err(e),
            }
        }

        Err(last_error.unwrap_or_else(|| ScraperError::GrpcConnectionFailed {
            retries: MAX_RETRIES,
            message: "Max retries exceeded".to_string(),
        }))
    }

    /// gRPCに送信（プレースホルダー - 実際の実装は grpc feature で有効化）
    async fn send_to_grpc(&self, _raw_data: &DtakologData) -> Result<GrpcResponse, ScraperError> {
        // この実装はプレースホルダー
        // 実際のgRPC送信は browser-render-rust の grpc feature を使用
        Err(ScraperError::Grpc(
            "gRPC sending not implemented in scraper library".to_string(),
        ))
    }

    /// ブラウザを閉じる
    pub async fn close(&mut self) -> Result<(), ScraperError> {
        self.browser = None;
        Ok(())
    }
}
