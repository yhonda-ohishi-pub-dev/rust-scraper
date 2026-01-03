use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::browser::SetDownloadBehaviorBehavior;
use chromiumoxide::cdp::browser_protocol::page::{EventJavascriptDialogOpening, HandleJavaScriptDialogParams};
use chromiumoxide::Page;
use futures::StreamExt;
use tracing::{debug, info, warn};

use crate::config::ScraperConfig;
use crate::error::ScraperError;
use crate::traits::Scraper;

const ETC_MEISAI_URL: &str = "https://www.etc-meisai.jp/";
const LOGIN_FUNC_CODE: &str = "funccode=1013000000";
const DOWNLOAD_WAIT_SECS: u64 = 120;

/// アカウント種別
#[derive(Debug, Clone, Copy, PartialEq)]
enum AccountType {
    Personal,   // 個人向け (/etc_user_meisai/)
    Corporate,  // 法人向け (/etc_corp_meisai/)
    Unknown,
}

pub struct EtcScraper {
    config: ScraperConfig,
    browser: Option<Browser>,
    page: Option<Arc<Page>>,
    account_type: AccountType,
}

impl EtcScraper {
    pub fn new(config: ScraperConfig) -> Self {
        Self {
            config,
            browser: None,
            page: None,
            account_type: AccountType::Unknown,
        }
    }

    fn get_page(&self) -> Result<&Arc<Page>, ScraperError> {
        self.page
            .as_ref()
            .ok_or_else(|| ScraperError::BrowserInit("ブラウザが初期化されていません".into()))
    }

    /// ダウンロードディレクトリの全ファイルを取得
    fn get_existing_files(&self) -> std::collections::HashSet<PathBuf> {
        let download_dir = &self.config.download_path;
        if !download_dir.exists() {
            return std::collections::HashSet::new();
        }

        std::fs::read_dir(download_dir)
            .ok()
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// ダウンロード完了を待機（既存ファイルを除外）
    async fn wait_for_download(
        &self,
        existing_files: &std::collections::HashSet<PathBuf>,
    ) -> Result<PathBuf, ScraperError> {
        let timeout = Duration::from_secs(DOWNLOAD_WAIT_SECS);
        let poll_interval = Duration::from_millis(500);
        let start = std::time::Instant::now();
        let download_dir = &self.config.download_path;

        debug!("ダウンロード待機開始... (既存ファイル数: {})", existing_files.len());

        loop {
            if let Ok(entries) = std::fs::read_dir(download_dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();

                    // 既存ファイルはスキップ
                    if existing_files.contains(&path) {
                        continue;
                    }

                    let filename = path.file_name().unwrap_or_default().to_string_lossy();

                    // ダウンロード中のファイルはスキップ
                    if filename.ends_with(".crdownload") || filename.ends_with(".tmp") {
                        debug!("ダウンロード中: {}", filename);
                        continue;
                    }

                    // CSVファイルを検出
                    if let Some(ext) = path.extension() {
                        if ext.to_ascii_lowercase() == "csv" {
                            info!("CSVファイル検出: {:?}", path);
                            return Ok(path);
                        }
                    }

                    // 拡張子がないファイル（GUID形式）で十分なサイズがあれば完了
                    if path.extension().is_none() {
                        if let Ok(metadata) = std::fs::metadata(&path) {
                            if metadata.len() > 100 {
                                // CSVにリネーム
                                let csv_path = path.with_extension("csv");
                                if std::fs::rename(&path, &csv_path).is_ok() {
                                    info!("GUIDファイルをリネーム: {:?}", csv_path);
                                    return Ok(csv_path);
                                }
                            }
                        }
                    }
                }
            }

            if start.elapsed() > timeout {
                // タイムアウト時にディレクトリ内のファイルをデバッグ出力
                let files: Vec<_> = std::fs::read_dir(download_dir)
                    .ok()
                    .map(|e| e.filter_map(|e| e.ok()).map(|e| e.path()).collect())
                    .unwrap_or_default();
                debug!("タイムアウト時のファイル一覧: {:?}", files);

                return Err(ScraperError::Timeout(format!(
                    "ダウンロードが{}秒以内に完了しませんでした",
                    DOWNLOAD_WAIT_SECS
                )));
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// CSVファイルをリネーム（user_id付与）
    fn rename_csv(&self, original_path: PathBuf) -> Result<PathBuf, ScraperError> {
        let filename = original_path
            .file_name()
            .ok_or_else(|| ScraperError::Download("ファイル名が取得できません".into()))?
            .to_string_lossy();

        let new_filename = format!("{}_{}", self.config.user_id, filename);
        let new_path = original_path.with_file_name(new_filename);

        std::fs::rename(&original_path, &new_path)?;
        info!("CSVファイルをリネーム: {:?} -> {:?}", original_path, new_path);

        Ok(new_path)
    }
}

#[async_trait]
impl Scraper for EtcScraper {
    async fn initialize(&mut self) -> Result<(), ScraperError> {
        info!("ブラウザを初期化中...");

        // ダウンロードディレクトリを作成
        std::fs::create_dir_all(&self.config.download_path)?;

        let download_path = self
            .config
            .download_path
            .canonicalize()
            .unwrap_or_else(|_| self.config.download_path.clone());

        // Windowsネイティブパスに変換（MSYS2のパスはChromeで認識されない）
        #[cfg(windows)]
        let download_path_str = {
            let path_str = download_path.to_string_lossy().to_string();
            // \\?\C:\... 形式を C:\... に変換
            path_str.trim_start_matches(r"\\?\").to_string()
        };
        #[cfg(not(windows))]
        let download_path_str = download_path.to_string_lossy().to_string();

        info!("ダウンロードパス: {}", download_path_str);

        // ブラウザ設定
        let mut builder = BrowserConfig::builder()
            .window_size(1280, 800)
            .arg(format!(
                "--download.default_directory={}",
                download_path_str
            ));

        // Chrome実行ファイルのパスを設定
        if let Some(ref chrome_path) = self.config.chrome_path {
            info!("Chrome実行ファイル: {:?}", chrome_path);
            builder = builder.chrome_executable(chrome_path);
            // headless-shell使用時はsandbox無効化が必要
            builder = builder.arg("--no-sandbox");
        }

        if self.config.headless {
            builder = builder.arg("--headless=new");
        } else {
            // headlessモードを無効化
            builder = builder.with_head();
        }

        let config = builder.build().map_err(|e| {
            ScraperError::BrowserInit(format!("ブラウザ設定エラー: {}", e))
        })?;

        let (browser, mut handler) = Browser::launch(config)
            .await
            .map_err(|e| ScraperError::BrowserInit(e.to_string()))?;

        // ブラウザイベントハンドラをバックグラウンドで実行
        tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                debug!("Browser event: {:?}", event);
            }
        });

        // 新しいページを作成
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| ScraperError::BrowserInit(e.to_string()))?;

        // JavaScriptダイアログハンドラを設定
        // confirmダイアログが開いたら自動的にOKをクリック
        let mut dialog_events = page.event_listener::<EventJavascriptDialogOpening>().await
            .map_err(|e| ScraperError::BrowserInit(format!("ダイアログリスナー設定エラー: {}", e)))?;

        let page_for_dialog = page.clone();
        tokio::spawn(async move {
            while let Some(event) = dialog_events.next().await {
                info!("ダイアログ検出: type={:?}, message={}", event.r#type, event.message);
                let params = HandleJavaScriptDialogParams::builder()
                    .accept(true)
                    .build()
                    .expect("HandleJavaScriptDialogParams build failed");
                if let Err(e) = page_for_dialog.execute(params).await {
                    warn!("ダイアログ応答エラー: {}", e);
                } else {
                    info!("ダイアログにOKで応答しました");
                }
            }
        });

        // ダウンロード先を設定
        let download_params =
            chromiumoxide::cdp::browser_protocol::browser::SetDownloadBehaviorParams::builder()
                .behavior(SetDownloadBehaviorBehavior::AllowAndName)
                .download_path(download_path_str)
                .events_enabled(true)
                .build()
                .map_err(|e| ScraperError::BrowserInit(format!("ダウンロード設定エラー: {}", e)))?;

        page.execute(download_params)
            .await
            .map_err(|e| ScraperError::BrowserInit(format!("ダウンロード設定エラー: {}", e)))?;

        self.browser = Some(browser);
        self.page = Some(Arc::new(page));

        info!("ブラウザ初期化完了");
        Ok(())
    }

    async fn login(&mut self) -> Result<(), ScraperError> {
        let page = self.get_page()?.clone();
        info!("ログイン処理開始...");

        // ETCメイセイトップページにアクセス
        page.goto(ETC_MEISAI_URL)
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;

        // ページ読み込み完了を待機
        tokio::time::sleep(Duration::from_secs(3)).await;
        debug!("トップページにアクセス完了");

        // ログインリンクが表示されるまで待機してクリック
        let login_link_selector = format!("a[href*='{}']", LOGIN_FUNC_CODE);
        for i in 0..10 {
            let exists: bool = page
                .evaluate(format!(r#"document.querySelector("{}") !== null"#, login_link_selector))
                .await
                .map(|v| v.into_value().unwrap_or(false))
                .unwrap_or(false);
            if exists {
                debug!("ログインリンク検出");
                break;
            }
            debug!("ログインリンク待機中... ({}/10)", i + 1);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        // クリックしてナビゲーションを待機
        let element = page.find_element(&login_link_selector)
            .await
            .map_err(|e| ScraperError::ElementNotFound(format!("ログインリンク: {}", e)))?;

        element.click()
            .await
            .map_err(|e| ScraperError::Navigation(format!("ログインリンククリック: {}", e)))?;

        // ページ読み込みを待機（wait_for_navigationはタイミングが難しいので固定待機）
        tokio::time::sleep(Duration::from_secs(5)).await;
        debug!("ログインページに遷移完了");

        // 現在のURLをデバッグ出力
        let url: String = page
            .evaluate("window.location.href")
            .await
            .map(|v| v.into_value().unwrap_or_default())
            .unwrap_or_default();
        debug!("現在のURL: {}", url);

        // 入力欄が表示されるまで待機
        for i in 0..10 {
            let exists: bool = page
                .evaluate(r#"document.querySelector("input[name='risLoginId']") !== null"#)
                .await
                .map(|v| v.into_value().unwrap_or(false))
                .unwrap_or(false);
            if exists {
                debug!("ログインフォーム検出");
                break;
            }
            debug!("ログインフォーム待機中... ({}/10)", i + 1);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        // それでも見つからない場合、ページの内容をデバッグ出力
        let form_exists: bool = page
            .evaluate(r#"document.querySelector("input[name='risLoginId']") !== null"#)
            .await
            .map(|v| v.into_value().unwrap_or(false))
            .unwrap_or(false);
        if !form_exists {
            let html: String = page
                .evaluate("document.body.innerHTML.substring(0, 1500)")
                .await
                .map(|v| v.into_value().unwrap_or_default())
                .unwrap_or_default();
            debug!("ページHTML: {}", html);
        }

        // ユーザーID入力（JavaScriptで直接設定）
        let user_id = &self.config.user_id;
        page.evaluate(format!(
            r#"document.querySelector("input[name='risLoginId']").value = '{}';"#,
            user_id
        ))
        .await
        .map_err(|e| ScraperError::Login(format!("ユーザーID入力: {}", e)))?;
        debug!("ユーザーID入力完了");

        // パスワード入力（JavaScriptで直接設定）
        let password = &self.config.password;
        page.evaluate(format!(
            r#"document.querySelector("input[name='risPassword']").value = '{}';"#,
            password
        ))
        .await
        .map_err(|e| ScraperError::Login(format!("パスワード入力: {}", e)))?;
        debug!("パスワード入力完了");

        // ログインボタンクリック
        page.find_element("input[type='button'][value='ログイン']")
            .await
            .map_err(|e| ScraperError::ElementNotFound(format!("ログインボタン: {}", e)))?
            .click()
            .await
            .map_err(|e| ScraperError::Login(format!("ログインボタンクリック: {}", e)))?;

        tokio::time::sleep(Duration::from_secs(3)).await;

        // ログイン後のURLを確認してアカウント種別を判定
        let current_url: String = page
            .evaluate("window.location.href")
            .await
            .map(|v| v.into_value().unwrap_or_default())
            .unwrap_or_default();
        debug!("ログイン後のURL: {}", current_url);

        // URL判定でアカウント種別を検出
        // 個人: /etc_user_meisai/ を含む
        // 法人: /etc_corp_meisai/ を含む
        if current_url.contains("/etc_corp_meisai/") {
            info!("法人アカウントを検出しました");
            self.account_type = AccountType::Corporate;
        } else if current_url.contains("/etc_user_meisai/") {
            info!("個人アカウントを検出しました");
            self.account_type = AccountType::Personal;
        } else {
            warn!("アカウント種別を判定できません: {}", current_url);
            // デフォルトは個人として扱う
            self.account_type = AccountType::Personal;
        }

        info!("ログイン完了");
        Ok(())
    }

    async fn download(&mut self) -> Result<PathBuf, ScraperError> {
        let page = self.get_page()?.clone();
        info!("CSVダウンロード処理開始... (アカウント種別: {:?})", self.account_type);

        // 現在のページ上のリンクをデバッグ出力
        let links_debug: String = page
            .evaluate(
                r#"
                (function() {
                    var links = document.querySelectorAll('a');
                    var texts = [];
                    for (var i = 0; i < links.length; i++) {
                        texts.push(links[i].textContent.trim());
                    }
                    return texts.join(' | ');
                })()
                "#,
            )
            .await
            .map(|v| v.into_value().unwrap_or_default())
            .unwrap_or_default();
        debug!("ログイン後のリンク一覧: {}", links_debug);

        // アカウント種別によってフロー分岐
        match self.account_type {
            AccountType::Corporate => self.download_corporate(&page).await,
            AccountType::Personal | AccountType::Unknown => self.download_personal(&page).await,
        }
    }

    async fn close(&mut self) -> Result<(), ScraperError> {
        info!("ブラウザを終了中...");

        // ページとブラウザの参照を解放
        self.page = None;
        self.browser = None;

        info!("ブラウザ終了完了");
        Ok(())
    }
}

impl EtcScraper {
    /// 個人向けダウンロード処理
    async fn download_personal(&self, page: &Arc<Page>) -> Result<PathBuf, ScraperError> {
        info!("個人向けダウンロード処理を開始...");

        // JavaScriptで「検索条件の指定」リンクをクリック
        let clicked: bool = page
            .evaluate(
                r#"
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
                "#,
            )
            .await
            .map(|v| v.into_value().unwrap_or(false))
            .unwrap_or(false);
        debug!("検索条件リンククリック: {}", clicked);

        tokio::time::sleep(Duration::from_secs(3)).await;

        // 「全て」オプションを選択（JavaScriptで）
        let _ = page
            .evaluate(
                r#"
                (function() {
                    var radio = document.querySelector("input[name='sokoKbn'][value='0']");
                    if (radio) {
                        radio.click();
                        return true;
                    }
                    return false;
                })()
                "#,
            )
            .await;
        debug!("「全て」オプション選択完了");
        tokio::time::sleep(Duration::from_secs(1)).await;

        // 設定保存ボタンをクリック（JavaScriptで）
        let _ = page
            .evaluate(
                r#"
                (function() {
                    var btn = document.querySelector("input[name='focusTarget_Save']");
                    if (btn) {
                        btn.click();
                        return true;
                    }
                    return false;
                })()
                "#,
            )
            .await;
        debug!("設定保存完了");
        tokio::time::sleep(Duration::from_secs(2)).await;

        // 検索ボタンをクリック（JavaScriptで）
        let search_clicked: bool = page
            .evaluate(
                r#"
                (function() {
                    var btn = document.querySelector("input[name='focusTarget']");
                    if (btn) {
                        btn.click();
                        return true;
                    }
                    return false;
                })()
                "#,
            )
            .await
            .map(|v| v.into_value().unwrap_or(false))
            .unwrap_or(false);

        if !search_clicked {
            let html: String = page
                .evaluate("document.body.innerHTML.substring(0, 2000)")
                .await
                .map(|v| v.into_value().unwrap_or_default())
                .unwrap_or_default();
            debug!("ページHTML (先頭2000文字): {}", html);
            return Err(ScraperError::ElementNotFound(
                "検索ボタン (input[name='focusTarget']) が見つかりません".into(),
            ));
        }
        debug!("検索ボタンクリック完了");

        tokio::time::sleep(Duration::from_secs(3)).await;

        // CSVダウンロード処理を実行
        self.download_csv(page).await
    }

    /// 法人向けダウンロード処理
    async fn download_corporate(&self, page: &Arc<Page>) -> Result<PathBuf, ScraperError> {
        info!("法人向けダウンロード処理を開始...");

        // 法人向けはトップページに既に明細リストがある場合がある
        // まず現在のページにCSVリンクがあるか確認
        let has_csv_link: bool = page
            .evaluate(
                r#"
                (function() {
                    var links = document.querySelectorAll('a');
                    for (var i = 0; i < links.length; i++) {
                        var text = links[i].textContent;
                        if (text.indexOf('明細') >= 0 && (text.indexOf('CSV') >= 0 || text.indexOf('ＣＳＶ') >= 0)) {
                            return true;
                        }
                    }
                    return false;
                })()
                "#,
            )
            .await
            .map(|v| v.into_value().unwrap_or(false))
            .unwrap_or(false);

        if has_csv_link {
            debug!("現在のページにCSVリンクが見つかりました");
        } else {
            // 検索条件ページへ移動が必要な場合
            debug!("検索条件ページへ移動します...");

            // 「検索条件の指定」または「利用明細検索」リンクをクリック
            let clicked: bool = page
                .evaluate(
                    r#"
                    (function() {
                        var links = document.querySelectorAll('a');
                        for (var i = 0; i < links.length; i++) {
                            var text = links[i].textContent;
                            if (text.indexOf('検索条件') >= 0 || text.indexOf('利用明細検索') >= 0) {
                                links[i].click();
                                return true;
                            }
                        }
                        return false;
                    })()
                    "#,
                )
                .await
                .map(|v| v.into_value().unwrap_or(false))
                .unwrap_or(false);
            debug!("検索リンククリック: {}", clicked);

            tokio::time::sleep(Duration::from_secs(3)).await;

            // 検索ボタンをクリック（法人向けは異なるセレクタの可能性）
            let search_clicked: bool = page
                .evaluate(
                    r#"
                    (function() {
                        // まず標準的なセレクタを試す
                        var btn = document.querySelector("input[name='focusTarget']");
                        if (btn) {
                            btn.click();
                            return true;
                        }
                        // 検索ボタンを探す（value属性で）
                        var inputs = document.querySelectorAll("input[type='button'], input[type='submit']");
                        for (var i = 0; i < inputs.length; i++) {
                            if (inputs[i].value.indexOf('検索') >= 0) {
                                inputs[i].click();
                                return true;
                            }
                        }
                        return false;
                    })()
                    "#,
                )
                .await
                .map(|v| v.into_value().unwrap_or(false))
                .unwrap_or(false);

            if !search_clicked {
                // ページのリンク一覧をデバッグ出力
                let links: String = page
                    .evaluate(
                        r#"
                        (function() {
                            var links = document.querySelectorAll('a');
                            var texts = [];
                            for (var i = 0; i < links.length; i++) {
                                texts.push(links[i].textContent.trim());
                            }
                            return texts.join(' | ');
                        })()
                        "#,
                    )
                    .await
                    .map(|v| v.into_value().unwrap_or_default())
                    .unwrap_or_default();
                debug!("ページのリンク一覧: {}", links);

                let inputs: String = page
                    .evaluate(
                        r#"
                        (function() {
                            var inputs = document.querySelectorAll("input[type='button'], input[type='submit']");
                            var texts = [];
                            for (var i = 0; i < inputs.length; i++) {
                                texts.push(inputs[i].name + '=' + inputs[i].value);
                            }
                            return texts.join(' | ');
                        })()
                        "#,
                    )
                    .await
                    .map(|v| v.into_value().unwrap_or_default())
                    .unwrap_or_default();
                debug!("ページのボタン一覧: {}", inputs);

                return Err(ScraperError::ElementNotFound(
                    "法人向け検索ボタンが見つかりません".into(),
                ));
            }
            debug!("検索ボタンクリック完了");

            tokio::time::sleep(Duration::from_secs(3)).await;
        }

        // CSVダウンロード処理を実行
        self.download_csv(page).await
    }

    /// CSVダウンロード共通処理
    async fn download_csv(&self, page: &Arc<Page>) -> Result<PathBuf, ScraperError> {
        // JavaScriptが完全に読み込まれるまで待機
        debug!("ページスクリプトの読み込みを待機中...");
        for i in 0..30 {
            let ready: bool = page
                .evaluate("(typeof goOutput === 'function' || typeof submitOpenPage === 'function')")
                .await
                .map(|v| v.into_value().unwrap_or(false))
                .unwrap_or(false);

            if ready {
                debug!("スクリプト読み込み完了");
                break;
            }
            debug!("スクリプト待機中... ({}/30)", i + 1);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        // 検索結果ページのリンク一覧をデバッグ出力
        let result_links: String = page
            .evaluate(
                r#"
                (function() {
                    var links = document.querySelectorAll('a');
                    var texts = [];
                    for (var i = 0; i < links.length; i++) {
                        texts.push(links[i].textContent.trim());
                    }
                    return texts.join(' | ');
                })()
                "#,
            )
            .await
            .map(|v| v.into_value().unwrap_or_default())
            .unwrap_or_default();
        debug!("検索結果ページのリンク一覧: {}", result_links);

        // 「当該月のご利用はありません」をチェック
        let no_usage: bool = page
            .evaluate(
                r#"
                (function() {
                    var captions = document.querySelectorAll('.meisaicaption, span.meisaicaption');
                    for (var i = 0; i < captions.length; i++) {
                        if (captions[i].textContent.indexOf('当該月のご利用はありません') >= 0) {
                            return true;
                        }
                    }
                    // テキスト全体からも検索
                    return document.body.innerText.indexOf('当該月のご利用はありません') >= 0;
                })()
                "#,
            )
            .await
            .map(|v| v.into_value().unwrap_or(false))
            .unwrap_or(false);

        if no_usage {
            info!("明細データなし（当該月のご利用はありません）- スキップします");
            return Err(ScraperError::NoUsageData("当該月のご利用はありません".into()));
        }

        // 既存ファイルを記録（新しいファイルを検出するため）
        let existing_files = self.get_existing_files();

        // CSVダウンロードリンクをクリック（JavaScriptで）
        let csv_clicked: bool = page
            .evaluate(
                r#"
                (function() {
                    var links = document.querySelectorAll('a');
                    for (var i = 0; i < links.length; i++) {
                        var text = links[i].textContent;
                        if (text.indexOf('明細') >= 0 && (text.indexOf('CSV') >= 0 || text.indexOf('ＣＳＶ') >= 0)) {
                            console.log('Found CSV link: ' + text);
                            links[i].click();
                            return true;
                        }
                    }
                    return false;
                })()
                "#,
            )
            .await
            .map(|v| v.into_value().unwrap_or(false))
            .unwrap_or(false);

        info!("CSVリンククリック: {}", csv_clicked);

        if !csv_clicked {
            return Err(ScraperError::ElementNotFound(
                "CSVダウンロードリンクが見つかりません".into(),
            ));
        }

        // ダウンロード完了を待機
        let csv_path = self.wait_for_download(&existing_files).await?;

        // ファイルをリネーム
        let renamed_path = self.rename_csv(csv_path)?;

        info!("CSVダウンロード完了: {:?}", renamed_path);
        Ok(renamed_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_etc_scraper_new() {
        let config = ScraperConfig::new("test_user", "test_password");
        let scraper = EtcScraper::new(config);
        assert!(scraper.browser.is_none());
        assert!(scraper.page.is_none());
    }

    #[test]
    fn test_config_builder() {
        let config = ScraperConfig::new("user", "pass")
            .with_headless(false)
            .with_download_path("/tmp/downloads")
            .with_timeout(Duration::from_secs(120));

        assert_eq!(config.user_id, "user");
        assert_eq!(config.password, "pass");
        assert!(!config.headless);
        assert_eq!(config.download_path, PathBuf::from("/tmp/downloads"));
        assert_eq!(config.timeout, Duration::from_secs(120));
    }
}
