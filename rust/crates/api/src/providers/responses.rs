//! Responses-protocol transport for any endpoint explicitly declaring support.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::Response;
use serde_json::{json, Value};

use crate::client::RateLimitState;
use crate::error::{ApiError, NonActionableResponse, ProviderFailureClass, ResponseOutcomeKind};
use crate::http_client::build_http_client_or_default;
use crate::types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    InputContentBlock, MessageDelta, MessageDeltaEvent, MessageRequest, MessageResponse,
    MessageStartEvent, MessageStopEvent, OutputContentBlock, StreamEvent, ToolChoice,
    ToolDefinition, Usage,
};

use super::openai_compat::{read_base_url, OpenAiCompatConfig};
use super::{preflight_message_request, Provider, ProviderFuture};

static HTTP_ATTEMPT_COUNTER: AtomicU64 = AtomicU64::new(0);

fn provider_trace(message: impl AsRef<str>) {
    if std::env::var("CLAW_PROVIDER_TRACE").as_deref() == Ok("1") {
        eprintln!("[provider-trace] {}", message.as_ref());
    }
}

#[derive(Debug, Clone)]
pub struct ResponsesClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    provider_name: &'static str,
}

impl ResponsesClient {
    #[must_use]
    pub fn new(api_key: impl Into<String>, config: OpenAiCompatConfig) -> Self {
        Self {
            http: build_http_client_or_default(),
            api_key: api_key.into(),
            base_url: read_base_url(config),
            provider_name: config.provider_name,
        }
    }

    pub fn from_env(config: OpenAiCompatConfig) -> Result<Self, ApiError> {
        let Some(api_key) = std::env::var(config.api_key_env)
            .ok()
            .filter(|value| !value.is_empty())
        else {
            let env_vars: &'static [&'static str] = match config.api_key_env {
                "OPENAI_API_KEY" => &["OPENAI_API_KEY"],
                "XAI_API_KEY" => &["XAI_API_KEY"],
                "DASHSCOPE_API_KEY" => &["DASHSCOPE_API_KEY"],
                _ => &[],
            };
            return Err(ApiError::missing_credentials(
                config.provider_name,
                env_vars,
            ));
        };
        Ok(Self::new(api_key, config))
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn endpoint(&self) -> String {
        let trimmed = self.base_url.trim_end_matches('/');
        if trimmed.ends_with("/responses") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/responses")
        }
    }

    async fn post(&self, request: &MessageRequest) -> Result<Response, ApiError> {
        let attempt_id = HTTP_ATTEMPT_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
        provider_trace(format!(
            "request_ready logical_call={} http_attempt_id={} api=responses path=/responses model={} tools={} tool_count={} reasoning_field={} reasoning={} stream={} max_output_tokens={} messages={}",
            request.provider_call_id.as_deref().unwrap_or("unassigned"),
            attempt_id,
            request.model,
            request.tools.is_some(),
            request.tools.as_ref().map_or(0, Vec::len),
            request.reasoning_effort.is_some(),
            request.reasoning_effort.as_deref().unwrap_or("none"),
            request.stream,
            request.max_tokens,
            request.messages.len(),
        ));
        let response = self
            .http
            .post(self.endpoint())
            .header("content-type", "application/json")
            .bearer_auth(&self.api_key)
            .json(&build_responses_request(request))
            .send()
            .await
            .map_err(ApiError::from)?;
        provider_trace(format!(
            "http_response logical_call={} http_attempt_id={} status={} request_id={}",
            request.provider_call_id.as_deref().unwrap_or("unassigned"),
            attempt_id,
            response.status().as_u16(),
            response
                .headers()
                .get("x-request-id")
                .or_else(|| response.headers().get("request-id"))
                .and_then(|value| value.to_str().ok())
                .unwrap_or("none"),
        ));
        if response.status().is_success() {
            Ok(response)
        } else {
            let status = response.status();
            let request_id = response
                .headers()
                .get("x-request-id")
                .or_else(|| response.headers().get("request-id"))
                .and_then(|value| value.to_str().ok())
                .map(ToOwned::to_owned);
            let body = response.text().await.unwrap_or_default();
            Err(ApiError::Api {
                status,
                error_type: None,
                message: Some(body.clone()),
                request_id,
                body,
                retryable: false,
            })
        }
    }

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        let request = MessageRequest {
            stream: false,
            ..request.clone()
        };
        preflight_message_request(&request)?;
        let response = self.post(&request).await?;
        let request_id = response
            .headers()
            .get("x-request-id")
            .or_else(|| response.headers().get("request-id"))
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let body = response.text().await.map_err(ApiError::from)?;
        let raw: Value = serde_json::from_str(&body).map_err(|error| {
            ApiError::json_deserialize(self.provider_name, &request.model, &body, error)
        })?;
        let mut normalized = normalize_response(&request.model, &raw);
        normalized.request_id = request_id;
        Ok(normalized)
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<ResponsesStream, ApiError> {
        preflight_message_request(request)?;
        let response = self.post(&request.clone().with_streaming()).await?;
        let rate_limit_state = RateLimitState::from_headers(response.headers());
        let request_id = response
            .headers()
            .get("x-request-id")
            .or_else(|| response.headers().get("request-id"))
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        Ok(ResponsesStream {
            request_id: request_id.clone(),
            rate_limit_state,
            response,
            buffer: Vec::new(),
            pending: Vec::new(),
            state: ResponseStreamState::new(request.model.clone(), request_id.clone()),
            done: false,
        })
    }
}

impl Provider for ResponsesClient {
    type Stream = ResponsesStream;

    fn send_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, MessageResponse> {
        Box::pin(async move { self.send_message(request).await })
    }

    fn stream_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, Self::Stream> {
        Box::pin(async move { self.stream_message(request).await })
    }
}

#[derive(Debug)]
pub struct ResponsesStream {
    request_id: Option<String>,
    rate_limit_state: Option<RateLimitState>,
    response: Response,
    buffer: Vec<u8>,
    pending: Vec<StreamEvent>,
    state: ResponseStreamState,
    done: bool,
}

impl ResponsesStream {
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    #[must_use]
    pub fn rate_limit_state(&self) -> Option<&RateLimitState> {
        self.rate_limit_state.as_ref()
    }

    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        loop {
            if let Some(event) = self.pending.pop() {
                return Ok(Some(event));
            }
            if self.done {
                return Ok(None);
            }
            if let Some(chunk) = self.response.chunk().await? {
                self.buffer.extend_from_slice(&chunk);
                let mut parsed_events = Vec::new();
                while let Some(frame) = take_sse_frame(&mut self.buffer) {
                    if let Some(event) = self.state.ingest(&frame)? {
                        parsed_events.extend(event);
                    }
                }
                self.pending.extend(parsed_events.into_iter().rev());
            } else {
                self.done = true;
                self.pending.extend(self.state.finish()?.into_iter().rev());
            }
        }
    }
}

#[derive(Debug)]
struct ResponseStreamState {
    model: String,
    request_id: Option<String>,
    response_id: Option<String>,
    status: Option<String>,
    output_types: Vec<String>,
    started: bool,
    text_started: bool,
    tool_calls: BTreeMap<String, ToolState>,
    item_to_call: BTreeMap<String, String>,
    usage: Usage,
    completed: bool,
    failure_class: Option<ProviderFailureClass>,
    provider_error_code: Option<String>,
    provider_error_message: Option<String>,
    incomplete_details: Option<String>,
}

#[derive(Debug, Default)]
struct ToolState {
    name: String,
    arguments: String,
    started: bool,
    stopped: bool,
}

impl ResponseStreamState {
    fn new(model: String, request_id: Option<String>) -> Self {
        Self {
            model,
            request_id,
            response_id: None,
            status: None,
            output_types: Vec::new(),
            started: false,
            text_started: false,
            tool_calls: BTreeMap::new(),
            item_to_call: BTreeMap::new(),
            usage: Usage::default(),
            completed: false,
            failure_class: None,
            provider_error_code: None,
            provider_error_message: None,
            incomplete_details: None,
        }
    }

    fn start(&mut self, id: &str) -> StreamEvent {
        self.started = true;
        self.response_id = Some(id.to_string());
        StreamEvent::MessageStart(MessageStartEvent {
            message: MessageResponse {
                id: id.to_string(),
                kind: "message".to_string(),
                role: "assistant".to_string(),
                content: Vec::new(),
                model: self.model.clone(),
                stop_reason: None,
                stop_sequence: None,
                usage: Usage::default(),
                request_id: None,
            },
        })
    }

    #[allow(clippy::too_many_lines)]
    fn ingest(&mut self, frame: &str) -> Result<Option<Vec<StreamEvent>>, ApiError> {
        let data = frame
            .lines()
            .find_map(|line| line.strip_prefix("data:").map(str::trim))
            .unwrap_or("");
        if data.is_empty() || data == "[DONE]" {
            return Ok(None);
        }
        let value: Value = serde_json::from_str(data)
            .map_err(|error| ApiError::json_deserialize("responses", &self.model, data, error))?;
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        let id = value
            .get("response")
            .and_then(|response| response.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("responses-stream");
        let mut events = Vec::new();
        if !self.started {
            events.push(self.start(id));
        }
        match kind {
            "response.output_text.delta" => {
                let delta = value.get("delta").and_then(Value::as_str).unwrap_or("");
                if !self.text_started {
                    self.text_started = true;
                    events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                        index: 0,
                        content_block: OutputContentBlock::Text {
                            text: String::new(),
                        },
                    }));
                }
                if !delta.is_empty() {
                    events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                        index: 0,
                        delta: ContentBlockDelta::TextDelta {
                            text: delta.to_string(),
                        },
                    }));
                }
            }
            "response.output_item.added" => {
                let item = value.get("item").cloned().unwrap_or_default();
                if let Some(item_type) = item.get("type").and_then(Value::as_str) {
                    self.record_output_type(item_type);
                    if let Some(contents) = item.get("content").and_then(Value::as_array) {
                        for content in contents {
                            if let Some(content_type) = content.get("type").and_then(Value::as_str)
                            {
                                self.record_output_type(content_type);
                            }
                        }
                    }
                }
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    let item_id = item.get("id").and_then(Value::as_str).unwrap_or("");
                    let call_id = item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("function-call")
                        .to_string();
                    if !item_id.is_empty() {
                        self.item_to_call
                            .insert(item_id.to_string(), call_id.clone());
                    }
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let state = self.tool_calls.entry(call_id.clone()).or_default();
                    state.name.clone_from(&name);
                    if !state.started {
                        state.started = true;
                        let index = 1 + self
                            .tool_calls
                            .keys()
                            .position(|key| key == &call_id)
                            .unwrap_or(0) as u32;
                        events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                            index,
                            content_block: OutputContentBlock::ToolUse {
                                id: call_id,
                                name,
                                input: json!({}),
                            },
                        }));
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                let item_id = value
                    .get("item_id")
                    .or_else(|| value.get("call_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("function-call");
                let call_id = self
                    .item_to_call
                    .get(item_id)
                    .map_or(item_id, String::as_str);
                let delta = value.get("delta").and_then(Value::as_str).unwrap_or("");
                let state = self.tool_calls.entry(call_id.to_string()).or_default();
                state.arguments.push_str(delta);
                let index = 1 + self
                    .tool_calls
                    .keys()
                    .position(|key| key == call_id)
                    .unwrap_or(0) as u32;
                events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                    index,
                    delta: ContentBlockDelta::InputJsonDelta {
                        partial_json: delta.to_string(),
                    },
                }));
            }
            "response.function_call_arguments.done" => {
                if let Some(item_id) = value
                    .get("item_id")
                    .or_else(|| value.get("call_id"))
                    .and_then(Value::as_str)
                {
                    let call_id = self
                        .item_to_call
                        .get(item_id)
                        .map_or(item_id, String::as_str);
                    if let Some(state) = self.tool_calls.get_mut(call_id) {
                        state.stopped = true;
                        let index = 1 + self
                            .tool_calls
                            .keys()
                            .position(|key| key == call_id)
                            .unwrap_or(0) as u32;
                        events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                            index,
                        }));
                    }
                }
            }
            "response.completed" => {
                let response = value.get("response").cloned().unwrap_or_default();
                self.record_response_metadata(&response);
                self.usage = parse_usage(response.get("usage"));
                self.completed = true;
            }
            "response.incomplete" | "response.failed" | "response.cancelled" => {
                let response = value.get("response").cloned().unwrap_or_default();
                self.record_response_metadata(&response);
                self.status = Some(kind.strip_prefix("response.").unwrap_or(kind).to_string());
                self.usage = parse_usage(response.get("usage"));
                self.completed = true;
            }
            _ => {}
        }
        Ok(Some(events))
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>, ApiError> {
        if !self.completed {
            return Err(ApiError::InvalidSseFrame(
                "responses stream ended before a terminal response event",
            ));
        }
        if let Some(kind) = match self.status.as_deref() {
            Some("incomplete") => Some(ResponseOutcomeKind::Incomplete),
            Some("failed") => Some(ResponseOutcomeKind::Failed),
            Some("cancelled" | "canceled") => Some(ResponseOutcomeKind::Cancelled),
            _ => None,
        } {
            return Err(self.non_actionable_error(kind));
        }
        if self.text_started || self.tool_calls.values().any(|call| call.started) {
            return Ok(vec![
                StreamEvent::MessageDelta(MessageDeltaEvent {
                    delta: MessageDelta {
                        stop_reason: Some("end_turn".to_string()),
                        stop_sequence: None,
                    },
                    usage: self.usage.clone(),
                }),
                StreamEvent::MessageStop(MessageStopEvent {}),
            ]);
        }
        let kind = match self.status.as_deref() {
            Some("incomplete") => ResponseOutcomeKind::Incomplete,
            Some("failed") => ResponseOutcomeKind::Failed,
            Some("cancelled" | "canceled") => ResponseOutcomeKind::Cancelled,
            _ if self.output_types.iter().any(|kind| kind == "refusal") => {
                ResponseOutcomeKind::Refusal
            }
            _ => ResponseOutcomeKind::Empty,
        };
        Err(self.non_actionable_error(kind))
    }

    fn non_actionable_error(&self, kind: ResponseOutcomeKind) -> ApiError {
        ApiError::NonActionableResponse(Box::new(NonActionableResponse {
            model: self.model.clone(),
            response_id: self.response_id.clone(),
            request_id: self.request_id.clone(),
            status: self.status.clone(),
            kind,
            output_types: self.output_types.clone(),
            input_tokens: self.usage.input_tokens,
            output_tokens: self.usage.output_tokens,
            cached_input_tokens: self.usage.cache_read_input_tokens,
            failure_class: self.failure_class,
            provider_error_code: self.provider_error_code.clone(),
            provider_error_message: self.provider_error_message.clone(),
            incomplete_details: self.incomplete_details.clone(),
        }))
    }

    fn record_output_type(&mut self, output_type: &str) {
        if !self.output_types.iter().any(|known| known == output_type) {
            self.output_types.push(output_type.to_string());
        }
    }

    fn record_response_metadata(&mut self, response: &Value) {
        self.response_id = response
            .get("id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| self.response_id.clone());
        self.status = response
            .get("status")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if let Some(error) = response.get("error") {
            self.provider_error_code = error
                .get("code")
                .and_then(Value::as_str)
                .map(|value| value.chars().take(256).collect());
            self.provider_error_message = error
                .get("message")
                .and_then(Value::as_str)
                .map(|value| value.chars().take(400).collect());
            self.failure_class = self
                .provider_error_code
                .as_deref()
                .map(classify_provider_failure);
        }
        self.incomplete_details = response.get("incomplete_details").map(|value| {
            let detail = value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned);
            detail.chars().take(400).collect()
        });
        if let Some(output) = response.get("output").and_then(Value::as_array) {
            for item in output {
                if let Some(item_type) = item.get("type").and_then(Value::as_str) {
                    self.record_output_type(item_type);
                }
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for block in content {
                        if let Some(content_type) = block.get("type").and_then(Value::as_str) {
                            self.record_output_type(content_type);
                        }
                    }
                }
            }
        }
    }
}

fn classify_provider_failure(code: &str) -> ProviderFailureClass {
    let code = code.to_ascii_lowercase();
    if code.contains("rate_limit") || code.contains("ratelimit") {
        ProviderFailureClass::RateLimit
    } else if [
        "server_error",
        "service_unavailable",
        "temporarily_unavailable",
        "timeout",
        "overloaded",
    ]
    .iter()
    .any(|marker| code.contains(marker))
    {
        ProviderFailureClass::Transient
    } else if code.contains("policy") || code.contains("safety") || code.contains("content") {
        ProviderFailureClass::Policy
    } else if code.contains("auth")
        || code.contains("credential")
        || code.contains("api_key")
        || code.contains("permission")
    {
        ProviderFailureClass::Authentication
    } else if code.contains("invalid") || code.contains("context_length") {
        ProviderFailureClass::Permanent
    } else {
        ProviderFailureClass::Unknown
    }
}

fn parse_usage(value: Option<&Value>) -> Usage {
    Usage {
        input_tokens: value
            .and_then(|v| v.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        output_tokens: value
            .and_then(|v| v.get("output_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: value
            .and_then(|v| v.get("input_tokens_details"))
            .and_then(|v| v.get("cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
    }
}

fn normalize_response(model: &str, value: &Value) -> MessageResponse {
    let mut content = Vec::new();
    if let Some(output) = value.get("output").and_then(Value::as_array) {
        for item in output {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    if let Some(contents) = item.get("content").and_then(Value::as_array) {
                        for block in contents {
                            if let Some(text) = block.get("text").and_then(Value::as_str) {
                                content.push(OutputContentBlock::Text {
                                    text: text.to_string(),
                                });
                            }
                        }
                    }
                }
                Some("function_call") => content.push(OutputContentBlock::ToolUse {
                    id: item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("function-call")
                        .to_string(),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    input: item.get("arguments").cloned().unwrap_or_else(|| json!({})),
                }),
                _ => {}
            }
        }
    }
    MessageResponse {
        id: value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("responses")
            .to_string(),
        kind: "message".to_string(),
        role: "assistant".to_string(),
        content,
        model: value
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(model)
            .to_string(),
        stop_reason: Some("end_turn".to_string()),
        stop_sequence: None,
        usage: parse_usage(value.get("usage")),
        request_id: None,
    }
}

fn build_responses_request(request: &MessageRequest) -> Value {
    let mut input = Vec::new();
    for message in &request.messages {
        for block in &message.content {
            match block {
                InputContentBlock::Text { text } => {
                    let content_type = if message.role == "assistant" {
                        "output_text"
                    } else {
                        "input_text"
                    };
                    input.push(json!({
                        "role": message.role,
                        "content": [{"type": content_type, "text": text}],
                    }));
                }
                InputContentBlock::Image { media_type, data } => input.push(json!({
                    "role": message.role,
                    "content": [{"type": "input_image", "image_url": format!("data:{media_type};base64,{data}")}],
                })),
                InputContentBlock::ToolResult { tool_use_id, content, .. } => input.push(json!({
                    "type": "function_call_output",
                    "call_id": tool_use_id,
                    "output": content.iter().map(|block| match block {
                        crate::types::ToolResultContentBlock::Text { text } => text.clone(),
                        crate::types::ToolResultContentBlock::Json { value } => value.to_string(),
                    }).collect::<Vec<_>>().join("\n"),
                })),
                InputContentBlock::ToolUse { id, name, input: arguments } => input.push(json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": arguments.to_string(),
                })),
            }
        }
    }
    let mut payload = json!({
        "model": strip_routing_prefix(&request.model),
        "input": input,
        "stream": request.stream,
        "max_output_tokens": request.max_tokens,
    });
    if let Some(system) = request.system.as_ref().filter(|text| !text.is_empty()) {
        payload["instructions"] = json!(system);
    }
    if let Some(tools) = request.tools.as_ref() {
        payload["tools"] = Value::Array(tools.iter().map(response_tool_definition).collect());
    }
    if let Some(choice) = request.tool_choice.as_ref() {
        payload["tool_choice"] = response_tool_choice(choice);
    }
    if let Some(effort) = request.reasoning_effort.as_ref() {
        payload["reasoning"] = json!({"effort": effort});
    }
    payload
}

fn response_tool_definition(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
    })
}

fn response_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::Any => json!("required"),
        ToolChoice::Tool { name } => json!({"type": "function", "name": name}),
    }
}

fn strip_routing_prefix(model: &str) -> &str {
    model.strip_prefix("openai/").unwrap_or(model)
}

fn take_sse_frame(buffer: &mut Vec<u8>) -> Option<String> {
    let position = buffer.windows(2).position(|window| window == b"\n\n")?;
    let frame = buffer.drain(..position + 2).collect::<Vec<_>>();
    Some(String::from_utf8_lossy(&frame[..frame.len() - 2]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::build_responses_request;
    use crate::types::{InputMessage, MessageRequest, ToolDefinition};
    use serde_json::json;

    #[test]
    fn renders_universal_request_as_responses_input() {
        let request = MessageRequest {
            model: "openai/gpt-5.6-luna".into(),
            max_tokens: 64,
            messages: vec![InputMessage::user_text("hello")],
            tools: Some(vec![ToolDefinition {
                name: "read_file".into(),
                description: Some("Read a file".into()),
                input_schema: json!({"type":"object"}),
            }]),
            reasoning_effort: Some("medium".into()),
            stream: true,
            ..Default::default()
        };
        let payload = build_responses_request(&request);
        assert_eq!(payload["model"], "gpt-5.6-luna");
        assert_eq!(payload["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(payload["tools"][0]["type"], "function");
        assert_eq!(payload["reasoning"]["effort"], "medium");
        assert_eq!(payload["max_output_tokens"], 64);
    }

    #[test]
    fn renders_assistant_text_as_responses_output_text() {
        let request = MessageRequest {
            model: "gpt-5.6-luna".into(),
            max_tokens: 64,
            messages: vec![InputMessage {
                role: "assistant".into(),
                content: vec![crate::types::InputContentBlock::Text {
                    text: "previous assistant response".into(),
                }],
            }],
            ..Default::default()
        };
        let payload = build_responses_request(&request);
        assert_eq!(payload["input"][0]["role"], "assistant");
        assert_eq!(payload["input"][0]["content"][0]["type"], "output_text");
    }
}
