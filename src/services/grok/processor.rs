use std::convert::Infallible;
use std::time::Duration;

use async_stream::stream;
use bytes::Bytes;
use futures::Stream;
use serde_json::Value as JsonValue;

use crate::core::config::get_config;
use crate::services::grok::assets::DownloadService;
use crate::services::grok::media::VideoUpscaleService;

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 从 tool_usage_card 原始内容中提取工具文本
fn extract_tool_text(raw: &str, rollout_id: &str) -> String {
    let prefix = if rollout_id.is_empty() {
        String::new()
    } else {
        format!("[{rollout_id}]")
    };

    // 提取 <xai:tool_name>...</xai:tool_name>
    let tool_name = extract_xml_content(raw, "xai:tool_name").unwrap_or_default();
    // 提取 <xai:tool_args>...</xai:tool_args>（含 CDATA）
    let tool_args = extract_xml_content(raw, "xai:tool_args").unwrap_or_default();

    match tool_name.as_str() {
        "web_search" => {
            let query = extract_json_field(&tool_args, "query").unwrap_or(tool_args.clone());
            format!("{prefix}[WebSearch] {query}\n")
        }
        "search_images" => {
            let desc =
                extract_json_field(&tool_args, "description").unwrap_or(tool_args.clone());
            format!("{prefix}[SearchImage] {desc}\n")
        }
        "chatroom_send" => {
            let message =
                extract_json_field(&tool_args, "message").unwrap_or(tool_args.clone());
            format!("{prefix}[AgentThink] {message}\n")
        }
        _ => {
            if !tool_name.is_empty() {
                format!("{prefix}[{tool_name}] {tool_args}\n")
            } else {
                String::new()
            }
        }
    }
}

/// 提取 XML 标签内容，支持 CDATA
fn extract_xml_content(raw: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = raw.find(&open)?;
    let end = raw.find(&close)?;
    let inner = &raw[start + open.len()..end];
    // 去除 CDATA 包装
    let trimmed = inner.trim();
    if trimmed.starts_with("<![CDATA[") && trimmed.ends_with("]]>") {
        Some(trimmed[9..trimmed.len() - 3].to_string())
    } else {
        Some(trimmed.to_string())
    }
}

/// 从 JSON 字符串中提取指定字段值
fn extract_json_field(json_str: &str, field: &str) -> Option<String> {
    let val: JsonValue = serde_json::from_str(json_str).ok()?;
    val.get(field)?.as_str().map(|s| s.to_string())
}

/// 从 cardAttachment 中提取图片 markdown
fn parse_card_attachment(card: &JsonValue) -> Option<String> {
    let json_data = card.get("jsonData")?;
    let json_str = json_data.as_str()?;
    let card_data: JsonValue = serde_json::from_str(json_str.trim()).ok()?;
    let image = card_data.get("image")?;
    let original = image.get("original")?.as_str()?;
    if original.is_empty() {
        return None;
    }
    let title = image
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("image")
        .replace('\n', " ");
    let title = title.trim();
    if title.is_empty() {
        Some(format!("![image]({original})\n"))
    } else {
        Some(format!("![{title}]({original})\n"))
    }
}

/// 非流式：从 cardAttachmentsJson 数组构建 card_map
fn build_card_map(card_attachments: &[JsonValue]) -> std::collections::HashMap<String, (String, String)> {
    let mut map = std::collections::HashMap::new();
    for raw in card_attachments {
        let json_str = match raw.as_str() {
            Some(s) => s,
            None => continue,
        };
        let card_data: JsonValue = match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let card_id = card_data
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if card_id.is_empty() {
            continue;
        }
        if let Some(image) = card_data.get("image") {
            let title = image
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .replace('\n', " ");
            let original = image
                .get("original")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !original.is_empty() {
                map.insert(card_id, (title, original));
            }
        }
    }
    map
}

/// 非流式：替换 <grok:render card_id="..."> 为实际图片
fn replace_render_cards(content: &str, card_map: &std::collections::HashMap<String, (String, String)>) -> String {
    // 匹配 <grok:render ...card_id="ID"...>...</grok:render> 或自闭合
    let mut result = content.to_string();
    // 简单循环替换，避免 regex 依赖
    loop {
        let Some(start) = result.find("<grok:render") else {
            break;
        };
        // 找到 card_id
        let segment = &result[start..];
        let card_id = extract_attr(segment, "card_id");
        // 找到结束标签
        let end_pos = if let Some(close) = segment.find("</grok:render>") {
            start + close + "</grok:render>".len()
        } else if let Some(close) = segment.find("/>") {
            start + close + 2
        } else {
            break;
        };

        let replacement = if let Some((title, url)) = card_id.as_deref().and_then(|id| card_map.get(id)) {
            let t = if title.trim().is_empty() { "image" } else { title.trim() };
            format!("![{t}]({url})")
        } else {
            String::new()
        };
        result.replace_range(start..end_pos, &replacement);
    }
    result
}

/// 非流式：过滤 tool_usage_card 标签
fn filter_tool_usage_content(content: &str, rollout_id: &str) -> String {
    let open_tag = "<xai:tool_usage_card";
    let close_tag = "</xai:tool_usage_card>";
    let mut result = String::new();
    let mut remaining = content;

    loop {
        let Some(start) = remaining.find(open_tag) else {
            result.push_str(remaining);
            break;
        };
        result.push_str(&remaining[..start]);
        let after = &remaining[start..];
        if let Some(end) = after.find(close_tag) {
            let block = &after[..end + close_tag.len()];
            let tool_text = extract_tool_text(block, rollout_id);
            result.push_str(&tool_text);
            remaining = &after[end + close_tag.len()..];
        } else {
            // 不完整块，丢弃
            break;
        }
    }
    result
}

fn extract_attr(segment: &str, attr_name: &str) -> Option<String> {
    let pattern = format!("{attr_name}=\"");
    let start = segment.find(&pattern)?;
    let after = &segment[start + pattern.len()..];
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

pub struct BaseProcessor {
    pub model: String,
    pub token: String,
    pub created: i64,
    pub app_url: String,
}

impl BaseProcessor {
    pub async fn new(model: &str, token: &str) -> Self {
        let app_url: String = get_config("app.app_url", String::new()).await;
        Self {
            model: model.to_string(),
            token: token.to_string(),
            created: now_ts(),
            app_url,
        }
    }

    pub async fn process_url(&self, path: &str, media_type: &str) -> String {
        let mut url_path = path.to_string();
        if url_path.starts_with("http") {
            if let Ok(parsed) = url::Url::parse(&url_path) {
                url_path = parsed.path().to_string();
            }
        }
        if !url_path.starts_with('/') {
            url_path = format!("/{url_path}");
        }
        if self.app_url.is_empty() {
            return format!("https://assets.grok.com{url_path}");
        }
        let dl = DownloadService::new().await;
        let _ = dl.download(&url_path, &self.token, media_type).await;
        format!(
            "{}/v1/files/{media_type}{}",
            self.app_url.trim_end_matches('/'),
            url_path
        )
    }

    fn sse_chunk(
        &self,
        response_id: &str,
        fingerprint: &str,
        content: Option<&str>,
        role: Option<&str>,
        finish: Option<&str>,
    ) -> String {
        let mut delta = serde_json::json!({});
        if let Some(role) = role {
            delta["role"] = JsonValue::String(role.to_string());
            delta["content"] = JsonValue::String(String::new());
        } else if let Some(content) = content {
            delta["content"] = JsonValue::String(content.to_string());
        }
        let chunk = serde_json::json!({
            "id": response_id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "system_fingerprint": fingerprint,
            "choices": [{
                "index": 0,
                "delta": delta,
                "logprobs": null,
                "finish_reason": finish,
            }]
        });
        format!("data: {}\n\n", chunk)
    }
}

pub struct StreamProcessor {
    base: BaseProcessor,
    response_id: Option<String>,
    fingerprint: String,
    think_opened: bool,
    role_sent: bool,
    filter_tags: Vec<String>,
    image_format: String,
    show_think: bool,
    rollout_id: String,
    tool_usage_enabled: bool,
    tool_usage_opened: bool,
    tool_usage_buffer: String,
}

impl StreamProcessor {
    pub async fn new(model: &str, token: &str, think: Option<bool>) -> Self {
        let show = match think {
            Some(v) => v,
            None => get_config("grok.thinking", false).await,
        };
        let filter_tags: Vec<String> = get_config("grok.filter_tags", Vec::<String>::new()).await;
        let image_format: String = get_config("app.image_format", "url".to_string()).await;
        let tool_usage_enabled = filter_tags.iter().any(|t| t == "xai:tool_usage_card");
        Self {
            base: BaseProcessor::new(model, token).await,
            response_id: None,
            fingerprint: String::new(),
            think_opened: false,
            role_sent: false,
            filter_tags,
            image_format,
            show_think: show,
            rollout_id: String::new(),
            tool_usage_enabled,
            tool_usage_opened: false,
            tool_usage_buffer: String::new(),
        }
    }

    /// 过滤 token 中的 tool_usage_card 标签，返回过滤后的文本
    fn filter_tool_card(&mut self, token: &str) -> String {
        if !self.tool_usage_enabled {
            return token.to_string();
        }

        let mut output = String::new();
        let mut remaining = token;

        while !remaining.is_empty() {
            if self.tool_usage_opened {
                // 在 tool_usage_card 内部，查找结束标签
                if let Some(end) = remaining.find("</xai:tool_usage_card>") {
                    self.tool_usage_buffer
                        .push_str(&remaining[..end + "</xai:tool_usage_card>".len()]);
                    let tool_text =
                        extract_tool_text(&self.tool_usage_buffer, &self.rollout_id);
                    output.push_str(&tool_text);
                    self.tool_usage_opened = false;
                    self.tool_usage_buffer.clear();
                    remaining = &remaining[end + "</xai:tool_usage_card>".len()..];
                } else {
                    // 结束标签不在此 chunk 中，继续缓冲
                    self.tool_usage_buffer.push_str(remaining);
                    break;
                }
            } else if let Some(start) = remaining.find("<xai:tool_usage_card") {
                // 找到开始标签
                output.push_str(&remaining[..start]);
                self.tool_usage_opened = true;
                self.tool_usage_buffer = remaining[start..].to_string();
                // 检查是否在同一 chunk 中有结束标签
                let after_start = &remaining[start..];
                if let Some(end) = after_start.find("</xai:tool_usage_card>") {
                    self.tool_usage_buffer = after_start
                        [..end + "</xai:tool_usage_card>".len()]
                        .to_string();
                    let tool_text =
                        extract_tool_text(&self.tool_usage_buffer, &self.rollout_id);
                    output.push_str(&tool_text);
                    self.tool_usage_opened = false;
                    self.tool_usage_buffer.clear();
                    remaining = &after_start[end + "</xai:tool_usage_card>".len()..];
                } else {
                    break;
                }
            } else {
                output.push_str(remaining);
                break;
            }
        }
        output
    }

    pub fn process<S>(mut self, input: S) -> impl Stream<Item = Result<Bytes, Infallible>>
    where
        S: Stream<Item = String> + Send + 'static,
    {
        stream! {
            let heartbeat_interval: u64 = get_config("grok.stream_heartbeat_interval", 15u64).await;
            let mut stream = Box::pin(input);
            let mut has_content = false;
            let mut stream_error: Option<String> = None;
            loop {
                match tokio::time::timeout(
                    Duration::from_secs(heartbeat_interval),
                    stream.next()
                ).await {
                    Ok(Some(line)) => {
                // 原有处理逻辑
                if line.trim().is_empty() {
                    continue;
                }
                // 检测流错误标记
                if let Some(err_msg) = line.strip_prefix("{\"__stream_error__\":\"") {
                    stream_error = Some(err_msg.trim_end_matches("\"}").replace("\\\"", "\"").to_string());
                    break;
                }
                let data: JsonValue = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let resp = data.get("result").and_then(|v| v.get("response")).cloned().unwrap_or(JsonValue::Null);

                if let Some(llm) = resp.get("llmInfo") {
                    if self.fingerprint.is_empty() {
                        if let Some(hash) = llm.get("modelHash").and_then(|v| v.as_str()) {
                            self.fingerprint = hash.to_string();
                        }
                    }
                }
                if let Some(rid) = resp.get("responseId").and_then(|v| v.as_str()) {
                    self.response_id = Some(rid.to_string());
                }
                // 捕获 rolloutId
                if let Some(rid) = resp.get("rolloutId").and_then(|v| v.as_str()) {
                    if self.rollout_id.is_empty() {
                        self.rollout_id = rid.to_string();
                    }
                }

                if !self.role_sent {
                    let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                    let chunk = self.base.sse_chunk(&id, &self.fingerprint, None, Some("assistant"), None);
                    self.role_sent = true;
                    yield Ok(Bytes::from(chunk));
                }

                // 处理 streamingImageGenerationResponse
                if let Some(img) = resp.get("streamingImageGenerationResponse") {
                    if self.show_think {
                        if !self.think_opened {
                            let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                            let chunk = self.base.sse_chunk(&id, &self.fingerprint, Some("<think>\n"), None, None);
                            self.think_opened = true;
                            yield Ok(Bytes::from(chunk));
                        }
                        let idx = img.get("imageIndex").and_then(|v| v.as_i64()).unwrap_or(0) + 1;
                        let progress = img.get("progress").and_then(|v| v.as_i64()).unwrap_or(0);
                        let msg = format!("正在生成第{idx}张图片中，当前进度{progress}%\n");
                        let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                        let chunk = self.base.sse_chunk(&id, &self.fingerprint, Some(&msg), None, None);
                        yield Ok(Bytes::from(chunk));
                    }
                    continue;
                }

                // 处理 cardAttachment
                if let Some(card) = resp.get("cardAttachment") {
                    if let Some(md) = parse_card_attachment(card) {
                        let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                        let chunk = self.base.sse_chunk(&id, &self.fingerprint, Some(&md), None, None);
                        yield Ok(Bytes::from(chunk));
                    }
                    continue;
                }

                // 处理 modelResponse
                if let Some(mr) = resp.get("modelResponse") {
                    if self.think_opened && self.show_think {
                        if let Some(msg) = mr.get("message").and_then(|v| v.as_str()) {
                            let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                            let chunk = self.base.sse_chunk(&id, &self.fingerprint, Some(&(msg.to_string() + "\n")), None, None);
                            yield Ok(Bytes::from(chunk));
                        }
                        let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                        let chunk = self.base.sse_chunk(&id, &self.fingerprint, Some("</think>\n"), None, None);
                        self.think_opened = false;
                        yield Ok(Bytes::from(chunk));
                    }

                    if let Some(urls) = mr.get("generatedImageUrls").and_then(|v| v.as_array()) {
                        for url_val in urls {
                            if let Some(url) = url_val.as_str() {
                                let parts: Vec<&str> = url.split('/').collect();
                                let img_id = parts.get(parts.len().saturating_sub(2)).copied().unwrap_or("image");
                                let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                                if self.image_format == "base64" {
                                    let dl = DownloadService::new().await;
                                    if let Ok(b64) = dl.to_base64(url, &self.base.token, "image").await {
                                        let chunk = self.base.sse_chunk(&id, &self.fingerprint, Some(&format!("![{img_id}]({b64})\n")), None, None);
                                        yield Ok(Bytes::from(chunk));
                                    } else {
                                        let final_url = self.base.process_url(url, "image").await;
                                        let chunk = self.base.sse_chunk(&id, &self.fingerprint, Some(&format!("![{img_id}]({final_url})\n")), None, None);
                                        yield Ok(Bytes::from(chunk));
                                    }
                                } else {
                                    let final_url = self.base.process_url(url, "image").await;
                                    let chunk = self.base.sse_chunk(&id, &self.fingerprint, Some(&format!("![{img_id}]({final_url})\n")), None, None);
                                    yield Ok(Bytes::from(chunk));
                                }
                            }
                        }
                    }
                    if let Some(meta) = mr.get("metadata") {
                        if let Some(hash) = meta.get("llm_info").and_then(|v| v.get("modelHash")).and_then(|v| v.as_str()) {
                            self.fingerprint = hash.to_string();
                        }
                    }
                    continue;
                }

                // 处理普通 token —— isThinking 控制
                if let Some(token_val) = resp.get("token") {
                    if let Some(token) = token_val.as_str() {
                        if token.is_empty() {
                            continue;
                        }

                        let is_thinking = resp.get("isThinking").and_then(|v| v.as_bool()).unwrap_or(false);

                        if is_thinking {
                            if !self.show_think {
                                continue; // 隐藏思维内容
                            }
                            if !self.think_opened {
                                let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                                let chunk = self.base.sse_chunk(&id, &self.fingerprint, Some("<think>\n"), None, None);
                                self.think_opened = true;
                                yield Ok(Bytes::from(chunk));
                            }
                        } else if self.think_opened {
                            let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                            let chunk = self.base.sse_chunk(&id, &self.fingerprint, Some("\n</think>\n"), None, None);
                            self.think_opened = false;
                            yield Ok(Bytes::from(chunk));
                        }

                        // 过滤 tool_usage_card 和 filter_tags
                        let filtered = self.filter_tool_card(token);
                        if !filtered.is_empty() && !self.filter_tags.iter().any(|t| filtered.contains(t)) {
                            has_content = true;
                            let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                            let chunk = self.base.sse_chunk(&id, &self.fingerprint, Some(&filtered), None, None);
                            yield Ok(Bytes::from(chunk));
                        }
                    }
                }
                    }
                    Ok(None) => break, // 流结束
                    Err(_) => {
                        // 超时，发送 SSE 心跳注释保持连接
                        yield Ok(Bytes::from(": heartbeat\n\n"));
                        continue;
                    }
                }
            }
            if self.think_opened {
                let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                let chunk = self.base.sse_chunk(&id, &self.fingerprint, Some("</think>\n"), None, None);
                yield Ok(Bytes::from(chunk));
            }
            // 流异常中断且无有效内容时，输出错误提示
            if let Some(ref err) = stream_error {
                if !has_content {
                    let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                    if !self.role_sent {
                        let role_chunk = self.base.sse_chunk(&id, &self.fingerprint, None, Some("assistant"), None);
                        yield Ok(Bytes::from(role_chunk));
                    }
                    let error_msg = format!("[upstream error] 上游连接中断，未收到有效响应: {err}");
                    let chunk = self.base.sse_chunk(&id, &self.fingerprint, Some(&error_msg), None, None);
                    yield Ok(Bytes::from(chunk));
                }
            }
            let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
            let chunk = self.base.sse_chunk(&id, &self.fingerprint, None, None, Some("stop"));
            yield Ok(Bytes::from(chunk));
            yield Ok(Bytes::from("data: [DONE]\n\n"));
        }
    }
}

pub struct CollectProcessor {
    base: BaseProcessor,
    image_format: String,
}

impl CollectProcessor {
    pub async fn new(model: &str, token: &str) -> Self {
        let image_format: String = get_config("app.image_format", "url".to_string()).await;
        Self {
            base: BaseProcessor::new(model, token).await,
            image_format,
        }
    }

    pub fn process<S>(self, input: S) -> impl std::future::Future<Output = JsonValue>
    where
        S: Stream<Item = String> + Send + 'static,
    {
        async move {
            let mut response_id = String::new();
            let mut fingerprint = String::new();
            let mut content = String::new();
            let mut rollout_id = String::new();
            let mut stream_error: Option<String> = None;
            let mut stream = Box::pin(input);
            while let Some(line) = stream.next().await {
                if line.trim().is_empty() {
                    continue;
                }
                // 检测流错误标记
                if let Some(err_msg) = line.strip_prefix("{\"__stream_error__\":\"") {
                    stream_error = Some(err_msg.trim_end_matches("\"}").replace("\\\"", "\"").to_string());
                    break;
                }
                let data: JsonValue = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let resp = data
                    .get("result")
                    .and_then(|v| v.get("response"))
                    .cloned()
                    .unwrap_or(JsonValue::Null);
                if let Some(llm) = resp.get("llmInfo") {
                    if fingerprint.is_empty() {
                        if let Some(hash) = llm.get("modelHash").and_then(|v| v.as_str()) {
                            fingerprint = hash.to_string();
                        }
                    }
                }
                if let Some(rid) = resp.get("responseId").and_then(|v| v.as_str()) {
                    response_id = rid.to_string();
                }
                if let Some(rid) = resp.get("rolloutId").and_then(|v| v.as_str()) {
                    if rollout_id.is_empty() {
                        rollout_id = rid.to_string();
                    }
                }

                // 处理 cardAttachment（流式中出现的卡片）
                if let Some(card) = resp.get("cardAttachment") {
                    if let Some(md) = parse_card_attachment(card) {
                        content.push_str(&md);
                    }
                }

                if let Some(mr) = resp.get("modelResponse") {
                    if let Some(msg) = mr.get("message").and_then(|v| v.as_str()) {
                        content.push_str(msg);
                    }
                    // 处理 cardAttachmentsJson
                    if let Some(cards) = mr.get("cardAttachmentsJson").and_then(|v| v.as_array()) {
                        let card_map = build_card_map(cards);
                        if !card_map.is_empty() {
                            content = replace_render_cards(&content, &card_map);
                        }
                    }
                    if let Some(urls) = mr.get("generatedImageUrls").and_then(|v| v.as_array()) {
                        for url_val in urls {
                            if let Some(url) = url_val.as_str() {
                                let final_url = if self.image_format == "base64" {
                                    let dl = DownloadService::new().await;
                                    dl.to_base64(url, &self.base.token, "image")
                                        .await
                                        .unwrap_or_else(|_| url.to_string())
                                } else {
                                    self.base.process_url(url, "image").await
                                };
                                content.push_str(&format!("![]({})\n", final_url));
                            }
                        }
                    }
                }
                if let Some(token_val) = resp.get("token") {
                    if let Some(token) = token_val.as_str() {
                        content.push_str(token);
                    }
                }
            }

            // 非流式后处理：过滤 tool_usage_card
            content = filter_tool_usage_content(&content, &rollout_id);

            // 流异常中断且无有效内容时，填充错误信息
            if content.trim().is_empty() {
                if let Some(ref err) = stream_error {
                    content = format!("[upstream error] 上游连接中断，未收到有效响应: {err}");
                }
            }

            serde_json::json!({
                "id": response_id,
                "object": "chat.completion",
                "created": self.base.created,
                "model": self.base.model,
                "system_fingerprint": fingerprint,
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": content, "refusal": null, "annotations": []},
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 0,
                    "completion_tokens": 0,
                    "total_tokens": 0,
                    "prompt_tokens_details": {"cached_tokens": 0, "text_tokens": 0, "audio_tokens": 0, "image_tokens": 0},
                    "completion_tokens_details": {"text_tokens": 0, "audio_tokens": 0, "reasoning_tokens": 0}
                }
            })
        }
    }
}

pub struct VideoStreamProcessor {
    base: BaseProcessor,
    response_id: Option<String>,
    think_opened: bool,
    role_sent: bool,
    show_think: bool,
    upscale_on_finish: bool,
}

impl VideoStreamProcessor {
    pub async fn new(model: &str, token: &str, think: Option<bool>, upscale_on_finish: bool) -> Self {
        let show = match think {
            Some(v) => v,
            None => get_config("grok.thinking", false).await,
        };
        Self {
            base: BaseProcessor::new(model, token).await,
            response_id: None,
            think_opened: false,
            role_sent: false,
            show_think: show,
            upscale_on_finish,
        }
    }

    fn build_video_html(video_url: &str, thumbnail_url: &str) -> String {
        let poster = if thumbnail_url.is_empty() {
            "".to_string()
        } else {
            format!(" poster=\"{}\"", thumbnail_url)
        };
        format!(
            "<video id=\"video\" controls=\"\" preload=\"none\"{poster}>\n  <source id=\"mp4\" src=\"{video_url}\" type=\"video/mp4\">\n</video>"
        )
    }

    pub fn process<S>(mut self, input: S) -> impl Stream<Item = Result<Bytes, Infallible>>
    where
        S: Stream<Item = String> + Send + 'static,
    {
        stream! {
            let heartbeat_interval: u64 = get_config("grok.stream_heartbeat_interval", 15u64).await;
            let mut stream = Box::pin(input);
            loop {
                match tokio::time::timeout(
                    Duration::from_secs(heartbeat_interval),
                    stream.next()
                ).await {
                    Ok(Some(line)) => {
                if line.trim().is_empty() { continue; }
                let data: JsonValue = match serde_json::from_str(&line) { Ok(v) => v, Err(_) => continue };
                let resp = data.get("result").and_then(|v| v.get("response")).cloned().unwrap_or(JsonValue::Null);

                if let Some(rid) = resp.get("responseId").and_then(|v| v.as_str()) {
                    self.response_id = Some(rid.to_string());
                }
                if !self.role_sent {
                    let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                    let chunk = self.base.sse_chunk(&id, "", None, Some("assistant"), None);
                    self.role_sent = true;
                    yield Ok(Bytes::from(chunk));
                }

                if let Some(video_resp) = resp.get("streamingVideoGenerationResponse") {
                    let progress = video_resp.get("progress").and_then(|v| v.as_i64()).unwrap_or(0);
                    if self.show_think {
                        if !self.think_opened {
                            let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                            let chunk = self.base.sse_chunk(&id, "", Some("<think>\n"), None, None);
                            self.think_opened = true;
                            yield Ok(Bytes::from(chunk));
                        }
                        let msg = format!("正在生成视频中，当前进度{progress}%\n");
                        let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                        let chunk = self.base.sse_chunk(&id, "", Some(&msg), None, None);
                        yield Ok(Bytes::from(chunk));
                    }
                    if progress == 100 {
                        if self.think_opened && self.show_think {
                            let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                            let chunk = self.base.sse_chunk(&id, "", Some("</think>\n"), None, None);
                            self.think_opened = false;
                            yield Ok(Bytes::from(chunk));
                        }
                        let video_url = video_resp.get("videoUrl").and_then(|v| v.as_str()).unwrap_or("");
                        let thumb_url = video_resp.get("thumbnailImageUrl").and_then(|v| v.as_str()).unwrap_or("");
                        if !video_url.is_empty() {
                            // upscale 逻辑
                            let actual_video_url = if self.upscale_on_finish {
                                let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                                let chunk = self.base.sse_chunk(&id, "", Some("正在对视频进行超分辨率\n"), None, None);
                                yield Ok(Bytes::from(chunk));
                                VideoUpscaleService::upscale(&self.base.token, video_url)
                                    .await
                                    .unwrap_or_else(|| video_url.to_string())
                            } else {
                                video_url.to_string()
                            };
                            let final_video = self.base.process_url(&actual_video_url, "video").await;
                            let final_thumb = if thumb_url.is_empty() { String::new() } else { self.base.process_url(thumb_url, "image").await };
                            let html = Self::build_video_html(&final_video, &final_thumb);
                            let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                            let chunk = self.base.sse_chunk(&id, "", Some(&html), None, None);
                            yield Ok(Bytes::from(chunk));
                        }
                    }
                }
                    }
                    Ok(None) => break, // 流结束
                    Err(_) => {
                        // 超时，发送 SSE 心跳注释保持连接
                        yield Ok(Bytes::from(": heartbeat\n\n"));
                        continue;
                    }
                }
            }
            if self.think_opened {
                let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
                let chunk = self.base.sse_chunk(&id, "", Some("</think>\n"), None, None);
                yield Ok(Bytes::from(chunk));
            }
            let id = self.response_id.clone().unwrap_or_else(|| format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()));
            let chunk = self.base.sse_chunk(&id, "", None, None, Some("stop"));
            yield Ok(Bytes::from(chunk));
            yield Ok(Bytes::from("data: [DONE]\n\n"));
        }
    }
}

pub struct VideoCollectProcessor {
    base: BaseProcessor,
    upscale_on_finish: bool,
}

impl VideoCollectProcessor {
    pub async fn new(model: &str, token: &str, upscale_on_finish: bool) -> Self {
        Self {
            base: BaseProcessor::new(model, token).await,
            upscale_on_finish,
        }
    }

    fn build_video_html(video_url: &str, thumbnail_url: &str) -> String {
        let poster = if thumbnail_url.is_empty() {
            "".to_string()
        } else {
            format!(" poster=\"{}\"", thumbnail_url)
        };
        format!(
            "<video id=\"video\" controls=\"\" preload=\"none\"{poster}>\n  <source id=\"mp4\" src=\"{video_url}\" type=\"video/mp4\">\n</video>"
        )
    }

    pub fn process<S>(self, input: S) -> impl std::future::Future<Output = JsonValue>
    where
        S: Stream<Item = String> + Send + 'static,
    {
        async move {
            let mut response_id = String::new();
            let mut content = String::new();
            let mut stream = Box::pin(input);
            while let Some(line) = stream.next().await {
                if line.trim().is_empty() {
                    continue;
                }
                let data: JsonValue = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let resp = data
                    .get("result")
                    .and_then(|v| v.get("response"))
                    .cloned()
                    .unwrap_or(JsonValue::Null);
                if let Some(video_resp) = resp.get("streamingVideoGenerationResponse") {
                    if video_resp.get("progress").and_then(|v| v.as_i64()) == Some(100) {
                        response_id = resp
                            .get("responseId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let video_url = video_resp
                            .get("videoUrl")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let thumb_url = video_resp
                            .get("thumbnailImageUrl")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !video_url.is_empty() {
                            // upscale 逻辑
                            let actual_video_url = if self.upscale_on_finish {
                                VideoUpscaleService::upscale(&self.base.token, video_url)
                                    .await
                                    .unwrap_or_else(|| video_url.to_string())
                            } else {
                                video_url.to_string()
                            };
                            let final_video = self.base.process_url(&actual_video_url, "video").await;
                            let final_thumb = if thumb_url.is_empty() {
                                String::new()
                            } else {
                                self.base.process_url(thumb_url, "image").await
                            };
                            content = Self::build_video_html(&final_video, &final_thumb);
                        }
                    }
                }
            }
            serde_json::json!({
                "id": response_id,
                "object": "chat.completion",
                "created": self.base.created,
                "model": self.base.model,
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": content, "refusal": null},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}
            })
        }
    }
}

pub struct ImageStreamProcessor {
    base: BaseProcessor,
    partial_index: usize,
    n: usize,
    target_index: Option<usize>,
    return_base64: bool,
}

impl ImageStreamProcessor {
    pub async fn new(model: &str, token: &str, n: usize, return_base64: bool) -> Self {
        let target_index = if n == 1 {
            Some(rand::random::<usize>() % 2)
        } else {
            None
        };
        Self {
            base: BaseProcessor::new(model, token).await,
            partial_index: 0,
            n,
            target_index,
            return_base64,
        }
    }

    fn sse_event(event: &str, data: JsonValue) -> String {
        format!("event: {}\ndata: {}\n\n", event, data)
    }

    pub fn process<S>(self, input: S) -> impl Stream<Item = Result<Bytes, Infallible>>
    where
        S: Stream<Item = String> + Send + 'static,
    {
        stream! {
            let heartbeat_interval: u64 = get_config("grok.stream_heartbeat_interval", 15u64).await;
            let mut final_images: Vec<JsonValue> = Vec::new();
            let mut stream = Box::pin(input);
            loop {
                match tokio::time::timeout(
                    Duration::from_secs(heartbeat_interval),
                    stream.next()
                ).await {
                    Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                let data: JsonValue = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let resp = data
                    .get("result")
                    .and_then(|v| v.get("response"))
                    .cloned()
                    .unwrap_or(JsonValue::Null);

                if let Some(img) = resp.get("streamingImageGenerationResponse") {
                    let image_index = img.get("imageIndex").and_then(|v| v.as_i64()).unwrap_or(0) as usize;
                    let progress = img.get("progress").and_then(|v| v.as_i64()).unwrap_or(0);
                    if self.n == 1 {
                        if let Some(target) = self.target_index {
                            if image_index != target {
                                continue;
                            }
                        }
                    }
                    let out_index = if self.n == 1 { 0 } else { image_index };
                    let mut payload = serde_json::json!({
                        "type": "image_generation.partial_image",
                        "index": out_index,
                        "progress": progress,
                    });
                    if self.return_base64 {
                        payload["b64_json"] = JsonValue::String(String::new());
                    } else {
                        payload["url"] = JsonValue::String(String::new());
                    }
                    yield Ok(Bytes::from(Self::sse_event("image_generation.partial_image", payload)));
                    continue;
                }

                if let Some(mr) = resp.get("modelResponse") {
                    if let Some(urls) = mr.get("generatedImageUrls").and_then(|v| v.as_array()) {
                        for url in urls {
                            if let Some(url) = url.as_str() {
                                if self.return_base64 {
                                    let dl = DownloadService::new().await;
                                    if let Ok(b64) = dl.to_base64(url, &self.base.token, "image").await {
                                        let b64_str = if let Some(idx) = b64.find(',') {
                                            b64[idx + 1..].to_string()
                                        } else {
                                            b64
                                        };
                                        final_images.push(serde_json::json!({"b64_json": b64_str}));
                                    }
                                } else {
                                    let final_url = self.base.process_url(url, "image").await;
                                    final_images.push(serde_json::json!({"url": final_url}));
                                }
                            }
                        }
                    }
                }
                    }
                    Ok(None) => break, // 流结束
                    Err(_) => {
                        // 超时，发送 SSE 心跳注释保持连接
                        yield Ok(Bytes::from(": heartbeat\n\n"));
                        continue;
                    }
                }
            }

            for (index, image) in final_images.iter().enumerate() {
                let out_index = if self.n == 1 {
                    if let Some(target) = self.target_index {
                        if index != target {
                            continue;
                        }
                    }
                    0
                } else {
                    index
                };
                let mut payload = serde_json::json!({
                    "type": "image_generation.completed",
                    "index": out_index,
                    "usage": {
                        "total_tokens": 50,
                        "input_tokens": 25,
                        "output_tokens": 25,
                        "input_tokens_details": {"text_tokens": 5, "image_tokens": 20}
                    }
                });
                if let Some(b64) = image.get("b64_json").and_then(|v| v.as_str()) {
                    payload["b64_json"] = JsonValue::String(b64.to_string());
                }
                if let Some(url) = image.get("url").and_then(|v| v.as_str()) {
                    payload["url"] = JsonValue::String(url.to_string());
                }
                yield Ok(Bytes::from(Self::sse_event("image_generation.completed", payload)));
            }
        }
    }
}

pub struct ImageCollectProcessor {
    base: BaseProcessor,
    return_base64: bool,
}

impl ImageCollectProcessor {
    pub async fn new(model: &str, token: &str, return_base64: bool) -> Self {
        Self {
            base: BaseProcessor::new(model, token).await,
            return_base64,
        }
    }

    pub fn process<S>(self, input: S) -> impl std::future::Future<Output = Vec<JsonValue>>
    where
        S: Stream<Item = String> + Send + 'static,
    {
        async move {
            let mut images = Vec::new();
            let mut stream = Box::pin(input);
            while let Some(line) = stream.next().await {
                if line.trim().is_empty() {
                    continue;
                }
                let data: JsonValue = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let resp = data
                    .get("result")
                    .and_then(|v| v.get("response"))
                    .cloned()
                    .unwrap_or(JsonValue::Null);
                if let Some(mr) = resp.get("modelResponse") {
                    if let Some(urls) = mr.get("generatedImageUrls").and_then(|v| v.as_array()) {
                        for url in urls {
                            if let Some(url) = url.as_str() {
                                if self.return_base64 {
                                    let dl = DownloadService::new().await;
                                    if let Ok(b64) =
                                        dl.to_base64(url, &self.base.token, "image").await
                                    {
                                        let b64_str = if let Some(idx) = b64.find(',') {
                                            b64[idx + 1..].to_string()
                                        } else {
                                            b64
                                        };
                                        images.push(serde_json::json!({"b64_json": b64_str}));
                                    }
                                } else {
                                    let final_url = self.base.process_url(url, "image").await;
                                    images.push(serde_json::json!({"url": final_url}));
                                }
                            }
                        }
                    }
                }
            }
            images
        }
    }
}

use futures::StreamExt;
