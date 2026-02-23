use reqwest::header::HeaderMap;

use crate::core::config::get_config;
use crate::services::grok::statsig::StatsigService;

// ---------------------------------------------------------------------------
// 浏览器检测
// ---------------------------------------------------------------------------

/// 从 wreq_emulation 字符串中解析浏览器品牌与版本号
/// 返回 (brand, version)，无法识别时返回 None
fn detect_browser(emulation: &str) -> Option<(&'static str, String)> {
    let lower = emulation.to_ascii_lowercase();
    if lower.starts_with("chrome") {
        let ver = extract_version(&lower, "chrome");
        Some(("Google Chrome", ver))
    } else if lower.starts_with("edge") {
        let ver = extract_version(&lower, "edge");
        Some(("Microsoft Edge", ver))
    } else {
        // firefox / safari 等不发送 Client Hints
        None
    }
}

/// 从 "chrome_144" / "chrome144" / "edge_142" 中提取版本号
fn extract_version(lower: &str, prefix: &str) -> String {
    lower
        .strip_prefix(prefix)
        .unwrap_or("")
        .trim_start_matches('_')
        .trim_start_matches('-')
        .to_string()
}

// ---------------------------------------------------------------------------
// User-Agent 自动生成
// ---------------------------------------------------------------------------

/// 根据 emulation 生成匹配的 User-Agent
fn build_user_agent(emulation: &str) -> String {
    let lower = emulation.to_ascii_lowercase();
    if lower.starts_with("firefox") {
        let ver = extract_version(&lower, "firefox");
        let ver = if ver.is_empty() {
            "130".to_string()
        } else {
            ver
        };
        return format!(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:{ver}.0) \
             Gecko/20100101 Firefox/{ver}.0"
        );
    }

    // Chromium 系 (Chrome / Edge)
    let ver = if lower.starts_with("chrome") {
        let v = extract_version(&lower, "chrome");
        if v.is_empty() { "136".to_string() } else { v }
    } else if lower.starts_with("edge") {
        let v = extract_version(&lower, "edge");
        if v.is_empty() { "136".to_string() } else { v }
    } else {
        "136".to_string()
    };

    let platform_part = build_os_platform_ua();

    let mut ua = format!(
        "Mozilla/5.0 ({platform_part}) AppleWebKit/537.36 \
         (KHTML, like Gecko) Chrome/{ver}.0.0.0 Safari/537.36"
    );

    if lower.starts_with("edge") {
        ua.push_str(&format!(" Edg/{ver}.0.0.0"));
    }

    ua
}

/// 根据编译目标平台生成 User-Agent 中的操作系统片段
fn build_os_platform_ua() -> &'static str {
    if cfg!(target_os = "macos") {
        "Macintosh; Intel Mac OS X 10_15_7"
    } else if cfg!(target_os = "linux") {
        "X11; Linux x86_64"
    } else {
        // Windows 及其他
        "Windows NT 10.0; Win64; x64"
    }
}

/// 根据编译目标平台返回 Sec-Ch-Ua-Platform 的值
fn build_platform_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "\"macOS\""
    } else if cfg!(target_os = "linux") {
        "\"Linux\""
    } else {
        "\"Windows\""
    }
}

/// 根据编译目标平台返回 Sec-Ch-Ua-Arch 的值
fn build_arch_hint() -> &'static str {
    if cfg!(target_arch = "aarch64") || cfg!(target_arch = "arm") {
        "arm"
    } else {
        "x86"
    }
}

// ---------------------------------------------------------------------------
// Client Hints
// ---------------------------------------------------------------------------

/// 为 Chromium 系浏览器构建 Sec-Ch-Ua 头的值
fn build_sec_ch_ua(brand: &str, version: &str) -> String {
    format!("\"{brand}\";v=\"{version}\", \"Chromium\";v=\"{version}\", \"Not(A:Brand\";v=\"24\"")
}

// ---------------------------------------------------------------------------
// Cookie
// ---------------------------------------------------------------------------

/// 构建 cookie 字符串：sso + sso-rw + 可选 cf_clearance
pub async fn build_cookie(token: &str) -> String {
    let raw = token.strip_prefix("sso=").unwrap_or(token);
    let cf: String = get_config("grok.cf_clearance", String::new()).await;
    let cf = cf.trim();
    if cf.is_empty() {
        format!("sso={raw}; sso-rw={raw}")
    } else {
        format!("sso={raw}; sso-rw={raw}; cf_clearance={cf}")
    }
}

// ---------------------------------------------------------------------------
// 公共入口
// ---------------------------------------------------------------------------

/// 获取有效的 User-Agent：优先使用用户配置，为空则根据 emulation 自动生成
async fn resolve_user_agent() -> (String, String) {
    let emulation: String = get_config("grok.wreq_emulation", "chrome_136".to_string()).await;
    let custom_ua: String = get_config("grok.user_agent", String::new()).await;
    let ua = if custom_ua.trim().is_empty() {
        build_user_agent(&emulation)
    } else {
        custom_ua
    };
    (ua, emulation)
}

/// 构建与 wreq_emulation 匹配的完整 Grok 请求 headers
///
/// - `token`: SSO token（带或不带 "sso=" 前缀均可）
/// - `content_type`: 可选 Content-Type，默认 "application/json"
/// - `referer`: 可选 Referer，默认 "https://grok.com/"
pub async fn build_grok_headers(
    token: &str,
    content_type: Option<&str>,
    referer: Option<&str>,
) -> HeaderMap {
    let (user_agent, emulation) = resolve_user_agent().await;

    let mut headers = HeaderMap::new();

    headers.insert("Accept", "*/*".parse().unwrap());
    headers.insert(
        "Accept-Encoding",
        "gzip, deflate, br, zstd".parse().unwrap(),
    );
    headers.insert("Accept-Language", "zh-CN,zh;q=0.9".parse().unwrap());
    headers.insert(
        "Baggage",
        "sentry-environment=production,sentry-release=d6add6fb0460641fd482d767a335ef72b9b6abb8,\
         sentry-public_key=b311e0f2690c81f25e2c4cf6d4f7ce1c"
            .parse()
            .unwrap(),
    );
    headers.insert("Cache-Control", "no-cache".parse().unwrap());
    headers.insert(
        "Content-Type",
        content_type.unwrap_or("application/json").parse().unwrap(),
    );
    headers.insert("Origin", "https://grok.com".parse().unwrap());
    headers.insert("Pragma", "no-cache".parse().unwrap());
    headers.insert("Priority", "u=1, i".parse().unwrap());
    headers.insert(
        "Referer",
        referer.unwrap_or("https://grok.com/").parse().unwrap(),
    );

    // Chromium 系浏览器才发送 Client Hints
    if let Some((brand, version)) = detect_browser(&emulation) {
        let sec_ch_ua = build_sec_ch_ua(brand, &version);
        headers.insert("Sec-Ch-Ua", sec_ch_ua.parse().unwrap());
        headers.insert("Sec-Ch-Ua-Arch", build_arch_hint().parse().unwrap());
        headers.insert("Sec-Ch-Ua-Bitness", "64".parse().unwrap());
        headers.insert("Sec-Ch-Ua-Mobile", "?0".parse().unwrap());
        headers.insert("Sec-Ch-Ua-Model", "".parse().unwrap());
        headers.insert("Sec-Ch-Ua-Platform", build_platform_hint().parse().unwrap());
    }

    headers.insert("Sec-Fetch-Dest", "empty".parse().unwrap());
    headers.insert("Sec-Fetch-Mode", "cors".parse().unwrap());
    headers.insert("Sec-Fetch-Site", "same-origin".parse().unwrap());
    headers.insert("User-Agent", user_agent.parse().unwrap());

    let statsig = StatsigService::gen_id().await;
    headers.insert("x-statsig-id", statsig.parse().unwrap());
    headers.insert(
        "x-xai-request-id",
        uuid::Uuid::new_v4().to_string().parse().unwrap(),
    );

    let cookie = build_cookie(token).await;
    headers.insert("Cookie", cookie.parse().unwrap());

    headers
}

/// 构建下载用的 headers（assets 下载场景）
///
/// 仅包含必要的 User-Agent、Cookie、Sec-Fetch 等头，不含 Client Hints
pub async fn build_download_headers(token: &str) -> HeaderMap {
    let (user_agent, _) = resolve_user_agent().await;

    let mut headers = HeaderMap::new();
    headers.insert(
        "Accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"
            .parse()
            .unwrap(),
    );
    headers.insert("Sec-Fetch-Dest", "document".parse().unwrap());
    headers.insert("Sec-Fetch-Mode", "navigate".parse().unwrap());
    headers.insert("Sec-Fetch-Site", "same-site".parse().unwrap());
    headers.insert("Sec-Fetch-User", "?1".parse().unwrap());
    headers.insert("Upgrade-Insecure-Requests", "1".parse().unwrap());
    headers.insert("User-Agent", user_agent.parse().unwrap());

    let cookie = build_cookie(token).await;
    headers.insert("Cookie", cookie.parse().unwrap());
    headers.insert("Referer", "https://grok.com/".parse().unwrap());

    headers
}

/// 获取与 wreq_emulation 匹配的 User-Agent（供 nsfw/imagine_nsfw 等场景复用）
pub async fn get_matched_user_agent() -> String {
    resolve_user_agent().await.0
}
