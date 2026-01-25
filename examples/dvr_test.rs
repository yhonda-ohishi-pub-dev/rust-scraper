//! DVR Request_DvrFileList テスト
//!
//! 最小限のコードでRequest_DvrFileListの動作確認
//!
//! 実行方法:
//! ```
//! cd rust-scraper
//! cargo run --example dvr_test
//! ```

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotParams;
use futures::StreamExt;
use std::time::Duration;

const LOGIN_URL: &str = "http://theearth-np.com/F-OES1010[Login].aspx?mode=timeout";
const MAIN_URL: &str = "http://theearth-np.com/WebVenus/F-AAV0001[VenusMain].aspx";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ログ設定
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // .env読み込み
    if let Ok(env_path) = std::fs::canonicalize(".env") {
        println!("Loading .env from: {:?}", env_path);
        for line in std::fs::read_to_string(".env")?.lines() {
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim().trim_matches('\'').trim_matches('"');
                if !key.starts_with('#') && !key.is_empty() {
                    std::env::set_var(key, value);
                }
            }
        }
    }

    let comp_id = std::env::var("COMP_ID").expect("COMP_ID not set");
    let user_name = std::env::var("USER_NAME").expect("USER_NAME not set");
    let user_pass = std::env::var("USER_PASS").expect("USER_PASS not set");

    println!("=== DVR Request_DvrFileList Test ===");
    println!("Company ID: {}", comp_id);
    println!("User Name: {}", user_name);

    // ブラウザ起動
    let chrome_path = std::env::var("CHROME_PATH").ok();
    let mut builder = BrowserConfig::builder()
        .no_sandbox()
        .disable_default_args()
        .arg("--no-sandbox")
        .arg("--disable-gpu")
        .arg("--disable-dev-shm-usage")
        .arg("--disable-setuid-sandbox")
        .arg("--window-size=1920,1080")
        .arg("--lang=ja-JP");

    // ヘッドレス設定（環境変数で切り替え可能）
    let headless = std::env::var("HEADLESS").unwrap_or_else(|_| "true".to_string()) == "true";
    if headless {
        builder = builder.arg("--headless=new");
    }

    if let Some(path) = chrome_path {
        builder = builder.chrome_executable(path);
    }

    let config = builder.build().map_err(|e| format!("Browser config error: {}", e))?;
    let (browser, mut handler) = Browser::launch(config).await?;

    // ハンドラをバックグラウンドで実行
    tokio::spawn(async move {
        while let Some(event) = handler.next().await {
            let _ = event;
        }
    });

    let page = browser.new_page("about:blank").await?;

    // ログイン
    println!("\n[1/4] Navigating to login page...");
    page.goto(LOGIN_URL).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // ポップアップを閉じる
    let _ = page.evaluate(r#"
        try {
            document.querySelector("[id*='popup_1']")?.click();
        } catch(e) {}
    "#).await;

    println!("[2/4] Logging in...");
    let login_script = format!(
        r#"
        document.getElementById('txtID2').value = '{}';
        document.getElementById('txtID1').value = '{}';
        document.getElementById('txtPass').value = '{}';
        document.getElementById('imgLogin').click();
        "#,
        comp_id, user_name, user_pass
    );
    page.evaluate(login_script).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // 重複ログインポップアップ対応
    let _ = page.evaluate(r#"
        try {
            const btn = document.querySelector("[id*='popup_1']");
            if (btn) btn.click();
        } catch(e) {}
    "#).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // メインページに移動
    println!("[3/4] Navigating to main page...");
    page.goto(MAIN_URL).await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // VenusBridgeService確認
    let check_result = page.evaluate(r#"
        typeof VenusBridgeService !== 'undefined' &&
        typeof VenusBridgeService.Request_DvrFileList === 'function'
    "#).await?;
    let service_available: bool = check_result.into_value().unwrap_or(false);
    println!("VenusBridgeService.Request_DvrFileList available: {}", service_available);

    if !service_available {
        println!("ERROR: VenusBridgeService not available!");
        save_screenshot(&page, "error_no_service.png").await;
        return Ok(());
    }

    // 車両リストを取得して最初の車両IDを使う
    println!("\n[4/4] Testing Request_DvrFileList...");

    // まず車両データを取得
    let vehicle_script = r#"
        new Promise((resolve, reject) => {
            const timeout = setTimeout(() => reject('timeout'), 30000);
            VenusBridgeService.VehicleStateTableForBranchEx(0, 1, (result) => {
                clearTimeout(timeout);
                resolve(JSON.stringify(result));
            });
        })
    "#;

    let vehicle_result = page.evaluate(vehicle_script).await;

    let vehicle_cd: i64 = match vehicle_result {
        Ok(val) => {
            let json_str: String = val.into_value().unwrap_or_else(|_| "[]".to_string());
            println!("Vehicle data received, length: {}", json_str.len());

            // 車両リストを表示
            if let Ok(vehicles) = serde_json::from_str::<Vec<serde_json::Value>>(&json_str) {
                println!("Total vehicles: {}", vehicles.len());

                // 最初の車両の全フィールドを表示
                if let Some(first) = vehicles.first() {
                    println!("First vehicle all fields:");
                    if let Some(obj) = first.as_object() {
                        for (key, val) in obj.iter().take(20) {
                            println!("  {}: {:?}", key, val);
                        }
                    }
                }

                // 最初の5件の車両情報を表示
                println!("\nVehicle list:");
                for (i, v) in vehicles.iter().take(5).enumerate() {
                    println!("  {}: VehicleCD={}, VehicleName={}",
                        i + 1,
                        v.get("VehicleCD").and_then(|x| x.as_i64()).unwrap_or(-1),
                        v.get("VehicleName").and_then(|x| x.as_str()).unwrap_or("?")
                    );
                }

                // 最初の車両のVehicleCDを取得
                if let Some(first) = vehicles.first() {
                    first.get("VehicleCD")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0)
                } else {
                    0
                }
            } else {
                0
            }
        }
        Err(e) => {
            println!("Failed to get vehicle data: {}", e);
            0
        }
    };

    if vehicle_cd == 0 {
        println!("ERROR: Could not get vehicle ID!");
        save_screenshot(&page, "error_no_vehicle.png").await;
        return Ok(());
    }

    // 環境変数で車両IDを指定可能
    let vehicle_cd = std::env::var("VEHICLE_CD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(vehicle_cd);

    println!("Using vehicle_cd: {}", vehicle_cd);

    // === テスト1: 引数1つ（車両IDのみ） - test.py方式 ===
    println!("\n--- Test 1: Single arg (vehicle_cd only, like test.py) ---");
    let callback_script = format!(
        r#"
        new Promise((resolve, reject) => {{
            const timeout = setTimeout(() => resolve('TIMEOUT'), 30000);
            VenusBridgeService.Request_DvrFileList({}, function() {{
                clearTimeout(timeout);
                const args = Array.from(arguments);
                resolve(JSON.stringify({{
                    count: args.length,
                    types: args.map(a => typeof a),
                    arg0: args[0],
                    arg1: args[1],
                    arg2: args[2]
                }}));
            }});
        }})
        "#,
        vehicle_cd
    );

    let start = std::time::Instant::now();
    let result = page.evaluate(callback_script.as_str()).await;
    let elapsed = start.elapsed();

    match result {
        Ok(val) => {
            let result_str: String = val.into_value().unwrap_or_else(|_| "ERROR".to_string());
            println!("Result ({}ms): {}", elapsed.as_millis(),
                if result_str.len() > 200 {
                    format!("{}... ({} chars)", &result_str[..200], result_str.len())
                } else {
                    result_str.clone()
                }
            );

            if result_str != "TIMEOUT" && result_str != "[]" {
                println!("SUCCESS: Got DVR file list!");
                if let Ok(files) = serde_json::from_str::<Vec<serde_json::Value>>(&result_str) {
                    println!("Files count: {}", files.len());
                    for (i, f) in files.iter().take(3).enumerate() {
                        println!("  {}: {:?}", i+1, f.get("FileName"));
                    }
                }
            } else if result_str == "TIMEOUT" {
                println!("TIMEOUT: Callback was not called within 30s");
            } else {
                println!("Empty result (no files for this vehicle)");
            }
        }
        Err(e) => {
            println!("ERROR: {}", e);
        }
    }

    // === テスト2: scraper.rsの形式（成功/失敗コールバック） ===
    println!("\n--- Test 2: Two callbacks (like scraper.rs check_video_files) ---");
    let callback_script2 = format!(
        r#"
        new Promise((resolve, reject) => {{
            const timeout = setTimeout(() => resolve('TIMEOUT'), 30000);
            VenusBridgeService.Request_DvrFileList(
                {},
                (a, b, jsonData, d, e) => {{
                    clearTimeout(timeout);
                    resolve(JSON.stringify({{
                        success: true,
                        a: a, b: b, jsonData: jsonData, d: d, e: e
                    }}));
                }},
                (error) => {{
                    clearTimeout(timeout);
                    resolve(JSON.stringify({{ success: false, error: error }}));
                }}
            );
        }})
        "#,
        vehicle_cd
    );

    let start2 = std::time::Instant::now();
    let result2 = page.evaluate(callback_script2.as_str()).await;
    let elapsed2 = start2.elapsed();

    match result2 {
        Ok(val) => {
            let result_str: String = val.into_value().unwrap_or_else(|_| "ERROR".to_string());
            println!("Result ({}ms): {}",
                elapsed2.as_millis(),
                if result_str.len() > 300 { format!("{}...", &result_str[..300]) } else { result_str }
            );
        }
        Err(e) => println!("ERROR: {}", e),
    }

    // === テスト3: Monitoring_DvrNotification2（元の問題のAPI） ===
    println!("\n--- Test 3: Monitoring_DvrNotification2 (the original problem) ---");

    // まず利用可能か確認
    let check = page.evaluate(r#"
        typeof VenusBridgeService.Monitoring_DvrNotification2 === 'function'
    "#).await;
    let available: bool = check.map(|v| v.into_value().unwrap_or(false)).unwrap_or(false);
    println!("Monitoring_DvrNotification2 available: {}", available);

    if available {
        // 正しい引数形式: Monitoring_DvrNotification2(sort, callback)
        // sort = "fieldName,dir,pageIndex,pageSize" または空文字列
        let dvr_script = r#"
            new Promise((resolve, reject) => {
                const timeout = setTimeout(() => {
                    console.log('[DVR] Monitoring_DvrNotification2 TIMEOUT after 30s');
                    resolve('TIMEOUT');
                }, 30000);

                // sort引数を追加（空文字列でページング情報のみ指定）
                const sort = ",," + "0" + "," + "50";  // pageIndex=0, pageSize=50
                console.log('[DVR] Calling Monitoring_DvrNotification2 with sort:', sort);
                VenusBridgeService.Monitoring_DvrNotification2(sort, function(resultArray) {
                    clearTimeout(timeout);
                    console.log('[DVR] Monitoring_DvrNotification2 callback received!');
                    resolve(JSON.stringify({
                        success: true,
                        count: resultArray?.length || 0,
                        data: resultArray
                    }));
                });
            })
        "#;

        let start3 = std::time::Instant::now();
        let result3 = page.evaluate(dvr_script).await;
        let elapsed3 = start3.elapsed();

        match result3 {
            Ok(val) => {
                let result_str: String = val.into_value().unwrap_or_else(|_| "ERROR".to_string());
                let is_timeout = result_str == "TIMEOUT";
                println!("Result ({}ms): {}",
                    elapsed3.as_millis(),
                    if result_str.len() > 500 { format!("{}...", &result_str[..500]) } else { result_str }
                );

                if is_timeout {
                    println!("FAILED: Callback was not called (this is the known bug)");
                } else {
                    println!("SUCCESS: Callback was called!");
                }
            }
            Err(e) => println!("ERROR: {}", e),
        }
    }

    // スクリーンショット保存
    save_screenshot(&page, "dvr_test_result.png").await;

    println!("\nTest completed!");
    Ok(())
}

async fn save_screenshot(page: &chromiumoxide::Page, filename: &str) {
    let params = CaptureScreenshotParams::default();
    match page.screenshot(params).await {
        Ok(data) => {
            if let Err(e) = std::fs::write(filename, &data) {
                println!("Failed to save screenshot: {}", e);
            } else {
                println!("Screenshot saved: {}", filename);
            }
        }
        Err(e) => println!("Screenshot failed: {}", e),
    }
}
