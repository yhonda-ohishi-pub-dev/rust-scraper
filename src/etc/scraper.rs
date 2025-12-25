use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::browser::SetDownloadBehaviorBehavior;
use chromiumoxide::Page;
use futures::StreamExt;
use tracing::{debug, info};

use crate::config::ScraperConfig;
use crate::error::ScraperError;
use crate::traits::Scraper;

const ETC_MEISAI_URL: &str = "https://www.etc-meisai.jp/";
const LOGIN_FUNC_CODE: &str = "funccode=1013000000";
const DOWNLOAD_WAIT_SECS: u64 = 30;

pub struct EtcScraper {
    config: ScraperConfig,
    browser: Option<Browser>,
    page: Option<Arc<Page>>,
}

impl EtcScraper {
    pub fn new(config: ScraperConfig) -> Self {
        Self {
            config,
            browser: None,
            page: None,
        }
    }

    fn get_page(&self) -> Result<&Arc<Page>, ScraperError> {
        self.page
            .as_ref()
            .ok_or_else(|| ScraperError::BrowserInit("ブラウザが初期化されていません".into()))
    }

    /// ダウンロードディレクトリにCSVファイルが存在するか確認
    fn find_csv_file(&self) -> Option<PathBuf> {
        let download_dir = &self.config.download_path;
        if !download_dir.exists() {
            return None;
        }

        std::fs::read_dir(download_dir)
            .ok()?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .find(|path| {
                path.extension()
                    .map(|ext| ext.to_ascii_lowercase() == "csv")
                    .unwrap_or(false)
            })
    }

    /// ダウンロード完了を待機
    async fn wait_for_download(&self) -> Result<PathBuf, ScraperError> {
        let timeout = Duration::from_secs(DOWNLOAD_WAIT_SECS);
        let poll_interval = Duration::from_millis(500);
        let start = std::time::Instant::now();

        loop {
            if let Some(path) = self.find_csv_file() {
                // ファイルが完全にダウンロードされたか確認（.crdownloadなどがないか）
                let filename = path.file_name().unwrap_or_default().to_string_lossy();
                if !filename.ends_with(".crdownload") && !filename.ends_with(".tmp") {
                    info!("CSVファイル検出: {:?}", path);
                    return Ok(path);
                }
            }

            if start.elapsed() > timeout {
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

        // ブラウザ設定
        let mut builder = BrowserConfig::builder()
            .window_size(1280, 800)
            .arg(format!(
                "--download.default_directory={}",
                download_path.display()
            ));

        if self.config.headless {
            builder = builder.arg("--headless=new");
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

        // ダウンロード先を設定
        let download_params =
            chromiumoxide::cdp::browser_protocol::browser::SetDownloadBehaviorParams::builder()
                .behavior(SetDownloadBehaviorBehavior::AllowAndName)
                .download_path(download_path.to_string_lossy().to_string())
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

        page.wait_for_navigation()
            .await
            .map_err(|e| ScraperError::Navigation(e.to_string()))?;
        debug!("トップページにアクセス完了");

        // ログインリンクをクリック
        let login_link_selector = format!("a[href*='{}']", LOGIN_FUNC_CODE);
        page.find_element(&login_link_selector)
            .await
            .map_err(|e| ScraperError::ElementNotFound(format!("ログインリンク: {}", e)))?
            .click()
            .await
            .map_err(|e| ScraperError::Navigation(format!("ログインリンククリック: {}", e)))?;

        tokio::time::sleep(Duration::from_secs(3)).await;
        debug!("ログインページに遷移完了");

        // ユーザーID入力
        page.find_element("input[name='risLoginId']")
            .await
            .map_err(|e| ScraperError::ElementNotFound(format!("ユーザーID入力欄: {}", e)))?
            .type_str(&self.config.user_id)
            .await
            .map_err(|e| ScraperError::Login(format!("ユーザーID入力: {}", e)))?;
        debug!("ユーザーID入力完了");

        // パスワード入力
        page.find_element("input[name='risPassword']")
            .await
            .map_err(|e| ScraperError::ElementNotFound(format!("パスワード入力欄: {}", e)))?
            .type_str(&self.config.password)
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

        info!("ログイン完了");
        Ok(())
    }

    async fn download(&mut self) -> Result<PathBuf, ScraperError> {
        let page = self.get_page()?.clone();
        info!("CSVダウンロード処理開始...");

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
            // 検索ボタンが見つからない場合、ページのHTMLをデバッグ出力
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

        // JavaScriptが完全に読み込まれるまで待機
        debug!("ページスクリプトの読み込みを待機中...");
        for i in 0..30 {
            let ready: bool = page
                .evaluate("(typeof goOutput === 'function' && typeof submitOpenPage === 'function')")
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

        // ダウンロード完了を待機
        let csv_path = self.wait_for_download().await?;

        // ファイルをリネーム
        let renamed_path = self.rename_csv(csv_path)?;

        info!("CSVダウンロード完了: {:?}", renamed_path);
        Ok(renamed_path)
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
