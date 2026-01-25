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

use super::types::{
    DtakologConfig, DtakologData, DtakologResult, DvrFileInfo, DvrNotification, GrpcResponse,
    VehicleData, VideoNotificationResult,
};

/// リトライ設定
const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF_MS: u64 = 1000;

/// ネットワークアイドル待機のタイムアウト（ミリ秒）
const NETWORK_IDLE_TIMEOUT_MS: u64 = 30000;
/// ネットワークアイドル判定のインターバル（ミリ秒）
const NETWORK_IDLE_CHECK_INTERVAL_MS: u64 = 500;
/// ページ安定待機のタイムアウト（ミリ秒）
const PAGE_STABLE_TIMEOUT_MS: u64 = 10000;

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
            .request_timeout(Duration::from_secs(60)) // CDPリクエストタイムアウトを延長
            .arg("--disable-blink-features=AutomationControlled")
            .arg("--disable-dev-shm-usage")
            .arg("--disable-gpu")
            .arg("--disable-web-security") // CORS制限を無効化
            .arg("--allow-running-insecure-content");

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

        // 映像通知処理前にページ安定化を待機（ヘッドレスモードで重要）
        info!("Waiting for page to stabilize after vehicle data extraction...");
        self.wait_request_idle(&page).await?;
        self.wait_stable(&page).await?;

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

        // 映像通知の動画処理（エラーがあってもジョブ失敗にはしない）
        let video_notifications = match self.process_video_notifications(&page).await {
            Ok(notifications) => notifications,
            Err(e) => {
                warn!("Video notification processing failed: {}", e);
                Vec::new()
            }
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
            video_notifications,
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
        info!("Clicking login button...");
        page.evaluate("document.querySelector('#imgLogin').click()")
            .await
            .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

        // ナビゲーション完了を待機（Go の WaitRequestIdle 相当）
        info!("Waiting for navigation after login...");
        self.wait_request_idle(page).await?;
        sleep(Duration::from_secs(5)).await;

        // ログイン成功確認（リトライ付き）
        let mut login_success = false;
        for i in 0..5 {
            match page
                .evaluate("document.querySelector('#Button1st_7') !== null")
                .await
            {
                Ok(result) => {
                    login_success = result.into_value::<bool>().unwrap_or(false);
                    if login_success {
                        break;
                    }
                }
                Err(e) => {
                    debug!("Login check attempt {} failed: {}", i + 1, e);
                }
            }
            sleep(Duration::from_secs(1)).await;
        }
        let login_success = login_success;

        // ログイン成功時、ホームボタンをクリックしてメインページへ遷移
        if login_success {
            info!("Login successful, clicking home button to navigate to main page...");
            page.evaluate("document.querySelector('#Button1st_7').click()")
                .await
                .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

            // ナビゲーション完了を待機
            self.wait_request_idle(page).await?;
            sleep(Duration::from_secs(5)).await;
        }

        if !login_success {
            // 既にログイン済みの場合（ポップアップが表示される）
            info!("Button1st_7 not found, checking for popup...");
            let has_popup = page
                .evaluate("document.querySelector('#popup_1') !== null")
                .await
                .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

            if has_popup.into_value::<bool>().unwrap_or(false) {
                info!("Popup found, clicking to dismiss...");
                page.evaluate("document.querySelector('#popup_1').click()")
                    .await
                    .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

                sleep(Duration::from_secs(3)).await;
                self.wait_request_idle(page).await?;

                // ポップアップ閉じた後の状態を確認
                let current_url = page
                    .evaluate("window.location.href")
                    .await
                    .map_err(|e| ScraperError::JavaScript(e.to_string()))?;
                let url = current_url.into_value::<String>().unwrap_or_default();
                info!("URL after popup dismiss: {}", url);

                // ホームボタンを確認
                let has_home = page
                    .evaluate("document.querySelector('#Button1st_7') !== null")
                    .await
                    .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

                if has_home.into_value::<bool>().unwrap_or(false) {
                    info!("Home button found, clicking...");
                    page.evaluate("document.querySelector('#Button1st_7').click()")
                        .await
                        .map_err(|e| ScraperError::JavaScript(e.to_string()))?;
                    self.wait_request_idle(page).await?;
                    sleep(Duration::from_secs(3)).await;
                } else {
                    info!("Home button not found after popup, trying to click login button again...");
                    // ログインボタンをもう一度クリック
                    if let Ok(_) = page
                        .evaluate("document.querySelector('#imgLogin').click()")
                        .await
                    {
                        info!("Clicked login button again, waiting for navigation...");
                        self.wait_request_idle(page).await?;
                        sleep(Duration::from_secs(5)).await;

                        // ログイン後にホームボタンを再確認
                        for i in 0..5 {
                            let has_home_retry = page
                                .evaluate("document.querySelector('#Button1st_7') !== null")
                                .await
                                .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

                            if has_home_retry.into_value::<bool>().unwrap_or(false) {
                                info!("Home button found after retry (attempt {}), clicking...", i + 1);
                                page.evaluate("document.querySelector('#Button1st_7').click()")
                                    .await
                                    .map_err(|e| ScraperError::JavaScript(e.to_string()))?;
                                self.wait_request_idle(page).await?;
                                sleep(Duration::from_secs(3)).await;
                                break;
                            }
                            sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
            } else {
                return Err(ScraperError::Login(
                    "Login verification failed".to_string(),
                ));
            }
        }

        // ログイン成功後、ページが安定するまで待機
        info!("Login completed, waiting for page to stabilize...");
        self.wait_request_idle(page).await?;
        self.wait_stable(page).await?;

        // ログイン後のページURLを確認（デバッグ用）
        let current_url = page
            .evaluate("window.location.href")
            .await
            .map_err(|e| ScraperError::JavaScript(e.to_string()))?;
        info!(
            "Post-login URL: {}",
            current_url.into_value::<String>().unwrap_or_default()
        );

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

        // ページの完全なロードを待機
        for i in 0..30 {
            let ready_state = page
                .evaluate("document.readyState")
                .await
                .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

            let state = ready_state.into_value::<String>().unwrap_or_default();
            if state == "complete" {
                info!("Page load complete after {}s", i + 1);
                break;
            }

            if i % 5 == 0 {
                info!("Waiting for page load... ({}/30) state={}", i + 1, state);
            }
            sleep(Duration::from_secs(1)).await;
        }

        // ネットワークアイドル待機（Go の WaitRequestIdle 相当）
        self.wait_request_idle(page).await?;

        // 追加の安定待機（Goと同じ5秒）
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

        // VenusBridgeServiceの初期化を待機（最大15秒）
        info!("Waiting for VenusBridgeService initialization...");
        let mut service_ready = false;
        for i in 0..15 {
            let has_service = page
                .evaluate(
                    r#"
                    typeof VenusBridgeService !== 'undefined' &&
                    typeof VenusBridgeService.VehicleStateTableForBranchEx === 'function'
                "#,
                )
                .await
                .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

            if has_service.into_value::<bool>().unwrap_or(false) {
                info!("VenusBridgeService ready after {}s in navigate_to_main", i + 1);
                service_ready = true;
                break;
            }

            if i % 3 == 0 {
                info!("VenusBridgeService not ready yet... ({}/15)", i + 1);
            }
            sleep(Duration::from_secs(1)).await;
        }

        if !service_ready {
            warn!("VenusBridgeService not ready after 15s in navigate_to_main, proceeding anyway...");
        }

        Ok(())
    }

    /// Vehicleデータを抽出
    async fn extract_vehicle_data(
        &self,
        page: &Page,
    ) -> Result<(Vec<VehicleData>, DtakologData), ScraperError> {
        // VenusBridgeService のロードを待機（最大30秒）
        let mut has_service = false;
        for i in 0..30 {
            let result = page
                .evaluate(
                    r#"
                    typeof VenusBridgeService !== 'undefined' &&
                    typeof VenusBridgeService.VehicleStateTableForBranchEx === 'function'
                "#,
                )
                .await
                .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

            if result.into_value::<bool>().unwrap_or(false) {
                has_service = true;
                break;
            }

            if i % 5 == 0 {
                info!("Waiting for VenusBridgeService... ({}/30)", i + 1);
            }
            sleep(Duration::from_secs(1)).await;
        }

        if !has_service {
            return Err(ScraperError::Extraction(
                "VenusBridgeService not found after 30s".to_string(),
            ));
        }

        info!(
            "Calling VenusBridgeService with branchID='{}', filterID='{}'",
            self.config.branch_id, self.config.filter_id
        );

        // ページ安定待機（Go の WaitStable 相当）
        self.wait_stable(page).await?;
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
        info!("Checking for loading messages...");
        let mut loading_cleared = false;
        for i in 0..30 {
            let has_loading = page
                .evaluate(
                    r#"
                    (() => {
                        const waitMsg = document.querySelector('#pMsg_wait, [id*="pMsg_wait"], [id*="pMsg"], [class*="pMsg"]');
                        const loadingDivs = document.querySelectorAll('[id*="loading"], [id*="Loading"], .loading-message, .wait-message');
                        const allLoading = waitMsg ? [waitMsg, ...loadingDivs] : [...loadingDivs];

                        const visibleLoading = allLoading.filter(elem => {
                            if (!elem) return false;
                            const style = window.getComputedStyle(elem);
                            const rect = elem.getBoundingClientRect();
                            return style.display !== 'none' &&
                                   style.visibility !== 'hidden' &&
                                   style.opacity !== '0' &&
                                   (rect.width > 0 || rect.height > 0);
                        });
                        return visibleLoading.length > 0;
                    })()
                "#,
                )
                .await
                .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

            if !has_loading.into_value::<bool>().unwrap_or(false) {
                loading_cleared = true;
                info!("No loading messages detected, proceeding...");
                break;
            }

            if i % 5 == 0 {
                info!("Loading message still visible, waiting... ({}/30)", i + 1);
            }
            sleep(Duration::from_secs(1)).await;
        }

        if !loading_cleared {
            warn!("Loading message timeout after 30 seconds, proceeding anyway...");
        }

        sleep(Duration::from_secs(3)).await;

        // JavaScriptを実行してデータを取得（Promiseでラップ）
        info!("Fetching vehicle data via VenusBridgeService...");
        let start = std::time::Instant::now();

        let promise_script = format!(
            r#"
            new Promise((resolve, reject) => {{
                const timeout = setTimeout(() => {{
                    reject(new Error('Vehicle data fetch timeout after 60s'));
                }}, 60000);

                VenusBridgeService.VehicleStateTableForBranchEx('{}', '{}',
                    (data) => {{
                        clearTimeout(timeout);
                        resolve(JSON.stringify(data));
                    }},
                    (error) => {{
                        clearTimeout(timeout);
                        reject(new Error(error || 'Unknown service error'));
                    }}
                );
            }})
        "#,
            self.config.branch_id, self.config.filter_id
        );

        let result = page
            .evaluate(promise_script.as_str())
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

        Ok((vehicles, raw_data))
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

    /// ネットワークリクエストがアイドル状態になるまで待機（Go の WaitRequestIdle 相当）
    async fn wait_request_idle(&self, page: &Page) -> Result<(), ScraperError> {
        info!("Waiting for network to become idle...");
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(NETWORK_IDLE_TIMEOUT_MS);

        // Performance API を使ってアクティブなリクエストを監視
        let mut idle_count = 0;
        const REQUIRED_IDLE_CHECKS: u32 = 3; // 連続3回アイドルでOK

        while start.elapsed() < timeout {
            let result = page
                .evaluate(
                    r#"
                    (() => {
                        // Performance API でリソースエントリを取得
                        const entries = performance.getEntriesByType('resource');
                        const now = performance.now();

                        // 直近500ms以内に開始されたリクエストがあるか
                        const recentRequests = entries.filter(e => {
                            return (now - e.startTime) < 500 && e.duration === 0;
                        });

                        // XMLHttpRequest や fetch のペンディング状態も確認
                        const hasPending = window.__pendingRequests > 0;

                        return recentRequests.length === 0 && !hasPending;
                    })()
                "#,
                )
                .await;

            match result {
                Ok(val) => {
                    if val.into_value::<bool>().unwrap_or(false) {
                        idle_count += 1;
                        if idle_count >= REQUIRED_IDLE_CHECKS {
                            info!(
                                "Network idle after {:?} ({} consecutive checks)",
                                start.elapsed(),
                                idle_count
                            );
                            return Ok(());
                        }
                    } else {
                        idle_count = 0;
                    }
                }
                Err(e) => {
                    debug!("Network idle check error: {}", e);
                    idle_count = 0;
                }
            }

            sleep(Duration::from_millis(NETWORK_IDLE_CHECK_INTERVAL_MS)).await;
        }

        warn!(
            "Network idle timeout after {:?}, proceeding anyway",
            start.elapsed()
        );
        Ok(())
    }

    /// ページが安定するまで待機（Go の WaitStable 相当）
    async fn wait_stable(&self, page: &Page) -> Result<(), ScraperError> {
        info!("Waiting for page to stabilize...");
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(PAGE_STABLE_TIMEOUT_MS);

        let mut last_html_len: Option<usize> = None;
        let mut stable_count = 0;
        const REQUIRED_STABLE_CHECKS: u32 = 3; // 連続3回同じでOK

        while start.elapsed() < timeout {
            let result = page
                .evaluate("document.documentElement.outerHTML.length")
                .await;

            match result {
                Ok(val) => {
                    let current_len = val.into_value::<usize>().unwrap_or(0);

                    match last_html_len {
                        Some(last) if last == current_len => {
                            stable_count += 1;
                            if stable_count >= REQUIRED_STABLE_CHECKS {
                                info!(
                                    "Page stable after {:?} ({} consecutive checks)",
                                    start.elapsed(),
                                    stable_count
                                );
                                return Ok(());
                            }
                        }
                        _ => {
                            stable_count = 0;
                        }
                    }

                    last_html_len = Some(current_len);
                }
                Err(e) => {
                    debug!("Page stable check error: {}", e);
                    stable_count = 0;
                }
            }

            sleep(Duration::from_millis(300)).await;
        }

        warn!(
            "Page stable timeout after {:?}, proceeding anyway",
            start.elapsed()
        );
        Ok(())
    }

    // ========================================
    // 映像通知（動画）処理メソッド
    // ========================================

    /// 映像通知リストを取得（Monitoring_DvrNotification2）
    async fn get_video_notifications(
        &self,
        page: &Page,
    ) -> Result<Vec<DvrNotification>, ScraperError> {
        info!("Fetching video notifications...");

        // Step 1: API呼び出しを開始し、結果をグローバル変数に保存
        let init_script = r#"
            (() => {
                window.__dvrResult = null;
                window.__dvrError = null;
                window.__dvrCalled = false;

                if (typeof VenusBridgeService === 'undefined' ||
                    typeof VenusBridgeService.Monitoring_DvrNotification2 !== 'function') {
                    window.__dvrError = "VenusBridgeService.Monitoring_DvrNotification2 not available";
                    window.__dvrCalled = true;
                    return "not_available";
                }

                // sort引数形式: "fieldName,dir,pageIndex,pageSize"
                // 空のソート設定でページング情報のみ指定
                const sort = ",," + "0" + "," + "100";  // pageIndex=0, pageSize=100
                console.log('[DVR] Calling Monitoring_DvrNotification2 with sort:', sort);
                VenusBridgeService.Monitoring_DvrNotification2(sort, function(resultArray) {
                    console.log('[DVR] Callback received:', resultArray);
                    window.__dvrResult = resultArray;
                    window.__dvrCalled = true;
                });
                return "initiated";
            })()
        "#;

        // API呼び出しを開始
        let init_result = page
            .evaluate(init_script)
            .await
            .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

        let init_status = init_result.into_value::<String>().unwrap_or_default();
        info!("DVR API call status: {}", init_status);

        if init_status == "not_available" {
            return Ok(Vec::new());
        }

        // Step 2: 結果をポーリング（最大60秒）
        let poll_script = r#"
            JSON.stringify({
                called: window.__dvrCalled || false,
                result: window.__dvrResult,
                error: window.__dvrError
            })
        "#;

        let mut json_str = r#"{"data":[],"error":"Timeout after 60s"}"#.to_string();

        for i in 0..120 {
            sleep(Duration::from_millis(500)).await;

            let poll_result = page
                .evaluate(poll_script)
                .await
                .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

            let poll_str = poll_result.into_value::<String>().unwrap_or_default();

            if let Ok(poll_data) = serde_json::from_str::<serde_json::Value>(&poll_str) {
                if poll_data.get("called").and_then(|v| v.as_bool()).unwrap_or(false) {
                    if let Some(err) = poll_data.get("error").and_then(|v| v.as_str()) {
                        if !err.is_empty() {
                            json_str = format!(r#"{{"data":[],"error":"{}"}}"#, err);
                            info!("DVR error received after {}ms: {}", (i + 1) * 500, err);
                            break;
                        }
                    }
                    if let Some(result) = poll_data.get("result") {
                        if result.is_array() {
                            let arr = result.as_array().unwrap();
                            if arr.len() >= 2 {
                                let count = arr[0].as_str().unwrap_or("0");
                                let json_data = arr[1].as_str().unwrap_or("[]");
                                json_str = format!(r#"{{"data":{},"error":null,"count":{}}}"#, json_data, count);
                                info!("DVR result received after {}ms", (i + 1) * 500);
                                break;
                            }
                        }
                        json_str = format!(r#"{{"data":[],"error":"Invalid result format","raw":{}}}"#, result);
                        info!("DVR invalid result after {}ms", (i + 1) * 500);
                        break;
                    }
                }
            }

            // 10秒ごとにログ出力
            if i > 0 && i % 20 == 0 {
                info!("Still waiting for DVR callback... ({}s elapsed)", (i + 1) / 2);
            }
        }

        // エラー情報付きのレスポンスをパース
        #[derive(serde::Deserialize)]
        struct DebugInfo {
            #[serde(rename = "hasVenusBridgeService")]
            has_venus_bridge_service: Option<bool>,
            #[serde(rename = "hasMethod")]
            has_method: Option<bool>,
        }

        #[derive(serde::Deserialize)]
        struct VideoNotificationResponse {
            data: Vec<DvrNotification>,
            error: Option<String>,
            #[serde(default)]
            count: Option<i32>,
            #[serde(default)]
            raw: Option<String>,
            #[serde(default)]
            debug: Option<DebugInfo>,
        }

        let response: VideoNotificationResponse = serde_json::from_str(&json_str).unwrap_or_else(|e| {
            warn!("Failed to parse video notification response: {}, raw: {}", e, json_str);
            VideoNotificationResponse { data: Vec::new(), error: Some(format!("Parse error: {}", e)), count: None, raw: Some(json_str.clone()), debug: None }
        });

        // エラーがあればログ出力
        if let Some(ref err) = response.error {
            warn!("Video notification API error: {}", err);
            if let Some(ref raw) = response.raw {
                debug!("Raw response: {}", raw);
            }
            // デバッグ情報があれば出力
            if let Some(ref dbg) = response.debug {
                warn!("Debug info - VenusBridgeService: {:?}, Monitoring_DvrNotification2: {:?}",
                    dbg.has_venus_bridge_service, dbg.has_method);
            }
        }

        if let Some(count) = response.count {
            info!("Video notification count from API: {}", count);
        }

        info!("Found {} video notifications", response.data.len());
        Ok(response.data)
    }

    /// 動画ファイル一覧を取得（Request_DvrFileList）
    async fn check_video_files(
        &self,
        page: &Page,
        vehicle_cd: i64,
    ) -> Result<Vec<DvrFileInfo>, ScraperError> {
        let script = format!(
            r#"
            new Promise((resolve, reject) => {{
                const timeout = setTimeout(() => {{
                    reject(new Error('Video file list fetch timeout after 30s'));
                }}, 30000);

                VenusBridgeService.Request_DvrFileList(
                    {},
                    (a, b, jsonData, d, e) => {{
                        clearTimeout(timeout);
                        resolve(jsonData || "[]");
                    }},
                    (error) => {{
                        clearTimeout(timeout);
                        reject(new Error(error || 'Unknown error'));
                    }}
                );
            }})
        "#,
            vehicle_cd
        );

        let result = page
            .evaluate(script.as_str())
            .await
            .map_err(|e| ScraperError::JavaScript(e.to_string()))?;

        let json_str = result.into_value::<String>().unwrap_or_else(|_| "[]".to_string());

        let files: Vec<DvrFileInfo> = serde_json::from_str(&json_str).unwrap_or_else(|e| {
            debug!("Failed to parse video file list: {}", e);
            Vec::new()
        });

        Ok(files)
    }

    /// 動画ダウンロードをリクエスト（Request_DvrFileTransfer_MultiTarget）
    async fn request_video_download(
        &self,
        page: &Page,
        serial_no: &str,
        file_name: &str,
    ) -> Result<bool, ScraperError> {
        let script = format!(
            r#"
            new Promise((resolve, reject) => {{
                const timeout = setTimeout(() => {{
                    reject(new Error('Video download request timeout after 30s'));
                }}, 30000);

                VenusBridgeService.Request_DvrFileTransfer_MultiTarget(
                    '{}',
                    '{}',
                    (result) => {{
                        clearTimeout(timeout);
                        resolve(true);
                    }},
                    (error) => {{
                        clearTimeout(timeout);
                        reject(new Error(error || 'Unknown error'));
                    }}
                );
            }})
        "#,
            serial_no, file_name
        );

        let result = page.evaluate(script.as_str()).await;

        match result {
            Ok(_) => Ok(true),
            Err(e) => {
                warn!("Video download request failed: {}", e);
                Ok(false)
            }
        }
    }

    /// 動画URLを構築
    fn build_video_url(file_path: &str, file_name: &str) -> String {
        let base_name = file_name.replace(".vdf", "");
        format!(
            "http://theearth-np.com/dvrData/{}/{}-1.mp4",
            file_path, base_name
        )
    }

    /// 映像通知の動画を処理（メインエントリ）
    /// 準備完了した動画のVideoNotificationResultリストを返す
    pub async fn process_video_notifications(
        &self,
        page: &Page,
    ) -> Result<Vec<VideoNotificationResult>, ScraperError> {
        info!("Processing video notifications...");

        // ネットワークアイドル待機（多層防御）
        self.wait_request_idle(page).await?;
        self.wait_stable(page).await?;

        // ページ安定化のため少し待機（ヘッドレスモードでの問題回避）
        sleep(Duration::from_secs(2)).await;

        // 映像通知リストを取得
        let notifications = self.get_video_notifications(page).await?;

        if notifications.is_empty() {
            info!("No video notifications to process");
            return Ok(Vec::new());
        }

        let mut results: Vec<VideoNotificationResult> = Vec::new();

        for notification in notifications {
            // 通知にFilePathがあれば直接URL構築可能
            if !notification.file_path.is_empty() {
                let url = Self::build_video_url(&notification.file_path, &notification.file_name);
                info!(
                    "Video ready: vehicle={}, event={}, datetime={}, mp4={}",
                    notification.vehicle_name,
                    notification.event_type,
                    notification.dvr_datetime,
                    url
                );
                results.push(VideoNotificationResult {
                    vehicle_cd: notification.vehicle_cd,
                    vehicle_name: notification.vehicle_name.clone(),
                    serial_no: notification.serial_no.clone(),
                    file_name: notification.file_name.clone(),
                    event_type: notification.event_type.clone(),
                    dvr_datetime: notification.dvr_datetime.clone(),
                    driver_name: notification.driver_name.clone(),
                    mp4_url: url,
                });
                continue;
            }

            // FilePathが空の場合、Request_DvrFileListで確認
            let files = self
                .check_video_files(page, notification.vehicle_cd)
                .await?;

            // 通知のFileNameと一致するファイルを探す
            let matching_file = files
                .iter()
                .find(|f| f.file_name == notification.file_name && !f.file_path.is_empty());

            if let Some(file) = matching_file {
                let url = Self::build_video_url(&file.file_path, &file.file_name);
                info!(
                    "Video ready: vehicle={}, event={}, datetime={}, mp4={}",
                    notification.vehicle_name,
                    notification.event_type,
                    notification.dvr_datetime,
                    url
                );
                results.push(VideoNotificationResult {
                    vehicle_cd: notification.vehicle_cd,
                    vehicle_name: notification.vehicle_name.clone(),
                    serial_no: notification.serial_no.clone(),
                    file_name: notification.file_name.clone(),
                    event_type: notification.event_type.clone(),
                    dvr_datetime: notification.dvr_datetime.clone(),
                    driver_name: notification.driver_name.clone(),
                    mp4_url: url,
                });
            } else {
                // ダウンロードリクエスト送信
                let success = self
                    .request_video_download(page, &notification.serial_no, &notification.file_name)
                    .await?;

                if success {
                    info!(
                        "Video download requested: vehicle={}, event={}, datetime={}",
                        notification.vehicle_name,
                        notification.event_type,
                        notification.dvr_datetime
                    );
                } else {
                    warn!(
                        "Video download request failed: vehicle={}, event={}",
                        notification.vehicle_name, notification.event_type
                    );
                }
            }
        }

        info!(
            "Video notification processing completed: {} ready videos",
            results.len()
        );
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // 実環境テスト用: cargo test -p scraper-service test_dtakolog_scraper -- --ignored --nocapture
    async fn test_dtakolog_scraper() {
        // トレーシング初期化
        tracing_subscriber::fmt()
            .with_env_filter("info,scraper_service=debug")
            .init();

        // 環境変数から認証情報を読み込み (.envのキー名に合わせる)
        let comp_id = std::env::var("COMP_ID").expect("COMP_ID not set");
        let user_name = std::env::var("USER_NAME").expect("USER_NAME not set");
        let user_pass = std::env::var("USER_PASS").expect("USER_PASS not set");

        let config = DtakologConfig {
            comp_id,
            user_name,
            user_pass,
            headless: true, // ヘッドレスモード
            debug: true,
            ..Default::default()
        };

        let mut scraper = DtakologScraper::new(config);
        scraper.initialize().await.expect("Failed to initialize browser");

        let result = scraper.scrape(None, true).await;

        match result {
            Ok(data) => {
                println!("\n=== Scrape Result ===");
                println!("Vehicles: {}", data.vehicles.len());
                println!("Video notifications: {}", data.video_notifications.len());
                for v in &data.video_notifications {
                    println!("  - {} ({}) @ {}: {}", v.vehicle_name, v.event_type, v.dvr_datetime, v.mp4_url);
                }
            }
            Err(e) => {
                panic!("Scrape failed: {:?}", e);
            }
        }
    }
}
