use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::{Mutex as StdMutex, OnceLock};

use api::{
    ApiError, ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent,
    ContentBlockStopEvent, EndpointCapabilities, InputContentBlock, InputMessage,
    MessageDeltaEvent, MessageRequest, OpenAiCompatClient, OpenAiCompatConfig, OpenAiCompatProfile,
    OpenAiCompatProtocol, OutputContentBlock, ParameterCapabilities, ProviderAuthMode,
    ProviderClient, ProviderConnectionConfig, ProviderFailureClass, ReasoningCapability,
    ResponseOutcomeKind, ResponsesClient, StreamEvent, ToolChoice, ToolDefinition,
};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[tokio::test]
async fn send_message_uses_openai_compatible_endpoint_and_auth() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let body = concat!(
        "{",
        "\"id\":\"chatcmpl_test\",",
        "\"model\":\"grok-3\",",
        "\"choices\":[{",
        "\"message\":{\"role\":\"assistant\",\"content\":\"Hello from Grok\",\"tool_calls\":[]},",
        "\"finish_reason\":\"stop\"",
        "}],",
        "\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":5}",
        "}"
    );
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response("200 OK", "application/json", body)],
    )
    .await
    else {
        return;
    };

    let client = OpenAiCompatClient::new("xai-test-key", OpenAiCompatConfig::xai())
        .with_base_url(server.base_url());
    let response = client
        .send_message(&sample_request(false))
        .await
        .expect("request should succeed");

    assert_eq!(response.model, "grok-3");
    assert_eq!(response.total_tokens(), 16);
    assert_eq!(
        response.content,
        vec![OutputContentBlock::Text {
            text: "Hello from Grok".to_string(),
        }]
    );

    let captured = state.lock().await;
    let request = captured.first().expect("server should capture request");
    assert_eq!(request.path, "/chat/completions");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer xai-test-key")
    );
    let body: serde_json::Value = serde_json::from_str(&request.body).expect("json body");
    assert_eq!(body["model"], json!("grok-3"));
    assert_eq!(body["messages"][0]["role"], json!("system"));
    assert_eq!(body["tools"][0]["type"], json!("function"));
}

#[tokio::test]
async fn configured_chat_profile_uses_opaque_model_custom_auth_and_parameters() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let body = "{\"id\":\"chatcmpl_custom\",\"model\":\"vendor/model:v3\",\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"custom gateway\",\"tool_calls\":[]},\"finish_reason\":\"stop\"}]}";
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response("200 OK", "application/json", body)],
    )
    .await
    else {
        return;
    };

    let profile = OpenAiCompatProfile {
        connection: ProviderConnectionConfig {
            provider: Some("local-gateway".to_string()),
            base_url: Some(server.base_url()),
            auth: ProviderAuthMode::None,
            headers: BTreeMap::from([("x-gateway-profile".to_string(), "fixture".to_string())]),
            ..Default::default()
        },
        protocol: OpenAiCompatProtocol::ChatCompletions,
        capabilities: EndpointCapabilities::default(),
        reasoning: ReasoningCapability::default(),
        parameters: ParameterCapabilities {
            max_output_tokens_parameter: "max_completion_tokens".to_string(),
            temperature: false,
            ..Default::default()
        },
    };
    let client = OpenAiCompatClient::from_profile(&profile)
        .expect("custom profile should construct without credentials");
    let mut request = sample_request(false);
    request.model = "vendor/model:v3".to_string();
    request.temperature = Some(0.2);
    request.reasoning_effort = Some("high".to_string());
    let response = client
        .send_message(&request)
        .await
        .expect("custom profile request should succeed");
    assert_eq!(response.model, "vendor/model:v3");

    let captured = state.lock().await;
    let request = captured.first().expect("server should capture request");
    assert_eq!(request.path, "/chat/completions");
    assert!(!request.headers.contains_key("authorization"));
    assert_eq!(
        request.headers.get("x-gateway-profile").map(String::as_str),
        Some("fixture")
    );
    let body: serde_json::Value = serde_json::from_str(&request.body).expect("json body");
    assert_eq!(body["model"], json!("vendor/model:v3"));
    assert_eq!(body["max_completion_tokens"], json!(64));
    assert!(body.get("max_tokens").is_none());
    assert!(body.get("temperature").is_none());
    assert!(body.get("reasoning_effort").is_none());
}

#[tokio::test]
async fn configured_responses_profile_uses_custom_model_and_reasoning_policy() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let body = "{\"id\":\"resp_custom\",\"model\":\"edge/model@1\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"responses gateway\"}]}],\"usage\":{\"input_tokens\":4,\"output_tokens\":2}}";
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response("200 OK", "application/json", body)],
    )
    .await
    else {
        return;
    };

    let profile = OpenAiCompatProfile {
        connection: ProviderConnectionConfig {
            provider: Some("third-party".to_string()),
            base_url: Some(server.base_url()),
            auth: ProviderAuthMode::None,
            ..Default::default()
        },
        protocol: OpenAiCompatProtocol::Responses,
        capabilities: EndpointCapabilities {
            chat_completions: false,
            responses: true,
            ..EndpointCapabilities::default()
        },
        reasoning: ReasoningCapability {
            supported: true,
            default_effort: Some("low".to_string()),
            allowed_efforts: vec!["low".to_string(), "high".to_string()],
            ..Default::default()
        },
        parameters: ParameterCapabilities::default(),
    };
    let client =
        ResponsesClient::from_profile(&profile).expect("custom Responses profile should construct");
    let mut request = sample_request(false);
    request.model = "edge/model@1".to_string();
    request.reasoning_effort = Some("high".to_string());
    let response = client
        .send_message(&request)
        .await
        .expect("custom Responses request should succeed");
    assert_eq!(response.model, "edge/model@1");

    let captured = state.lock().await;
    let request = captured.first().expect("server should capture request");
    assert_eq!(request.path, "/responses");
    let body: serde_json::Value = serde_json::from_str(&request.body).expect("json body");
    assert_eq!(body["model"], json!("edge/model@1"));
    assert_eq!(body["reasoning"]["effort"], json!("high"));
}

#[tokio::test]
async fn chat_stream_retains_optional_rate_limit_metadata() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = "data: {\"id\":\"chatcmpl_limits\",\"model\":\"opaque\",\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let Some(server) = spawn_server(
        state,
        vec![http_response_with_headers(
            "200 OK",
            "text/event-stream",
            sse,
            &[
                ("x-ratelimit-limit-tokens", "500000"),
                ("x-ratelimit-remaining-tokens", "490000"),
                ("x-ratelimit-reset-tokens", "1.2s"),
            ],
        )],
    )
    .await
    else {
        return;
    };

    let client = OpenAiCompatClient::new("key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let stream = client
        .stream_message(&sample_request(true))
        .await
        .expect("stream should start");
    let limits = stream
        .rate_limit_state()
        .expect("optional rate-limit headers should be retained");
    assert_eq!(limits.token_limit, Some(500_000));
    assert_eq!(limits.token_remaining, Some(490_000));
    assert_eq!(limits.token_reset_after_seconds, Some(2));
}

#[tokio::test]
async fn responses_transport_normalizes_tool_stream_and_uses_responses_endpoint() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = concat!(
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"item_1\",\"call_id\":\"call_1\",\"name\":\"read_file\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"item_1\",\"delta\":\"{\\\"path\\\":\\\"x\\\"}\"}\n\n",
        "data: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"item_1\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"model\":\"custom-model\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\n\n"
    );
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response("200 OK", "text/event-stream", sse)],
    )
    .await
    else {
        return;
    };

    let client = ResponsesClient::new("responses-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(true))
        .await
        .expect("responses request should succeed");
    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await.expect("event should parse") {
        events.push(event);
    }
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            content_block: OutputContentBlock::ToolUse { id, name, .. },
            ..
        }) if id == "call_1" && name == "read_file"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::MessageDelta(MessageDeltaEvent { usage, .. })
            if usage.input_tokens == 3 && usage.output_tokens == 2
    )));

    let captured = state.lock().await;
    let request = captured.first().expect("server should capture request");
    assert_eq!(request.path, "/responses");
    let body: serde_json::Value = serde_json::from_str(&request.body).expect("json body");
    assert_eq!(body["model"], "grok-3");
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["max_output_tokens"], 64);
}

#[tokio::test]
async fn responses_transport_uses_completed_output_text_when_no_delta_was_streamed() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_snapshot_text\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"snapshot answer\",\"annotations\":[]}]}],\"usage\":{\"input_tokens\":7,\"output_tokens\":4}}}\n\n";
    let Some(server) = spawn_server(
        state,
        vec![http_response("200 OK", "text/event-stream", sse)],
    )
    .await
    else {
        return;
    };

    let client = ResponsesClient::new("responses-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(true))
        .await
        .expect("responses request should succeed");
    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await.expect("event should parse") {
        events.push(event);
    }

    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            delta: ContentBlockDelta::TextDelta { text },
            ..
        }) if text == "snapshot answer"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::MessageDelta(MessageDeltaEvent { usage, .. })
            if usage.input_tokens == 7 && usage.output_tokens == 4
    )));
    assert!(matches!(events.last(), Some(StreamEvent::MessageStop(_))));
}

#[tokio::test]
async fn responses_transport_uses_finalized_output_item_text_without_delta() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = concat!(
        "data: {\"type\":\"response.output_item.added\",\"response\":{\"id\":\"resp_finalized\"},\"item\":{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"in_progress\",\"content\":[]}}\n\n",
        "data: {\"type\":\"response.content_part.added\",\"response\":{\"id\":\"resp_finalized\"},\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"part\":{\"type\":\"output_text\",\"text\":\"\"}}\n\n",
        "data: {\"type\":\"response.output_text.done\",\"response\":{\"id\":\"resp_finalized\"},\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"text\":\"finalized answer\"}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"response\":{\"id\":\"resp_finalized\"},\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"finalized answer\",\"annotations\":[]}]}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_finalized\",\"status\":\"completed\",\"usage\":{\"input_tokens\":7,\"output_tokens\":4}}}\n\n"
    );
    let Some(server) = spawn_server(
        state,
        vec![http_response("200 OK", "text/event-stream", sse)],
    )
    .await
    else {
        return;
    };

    let client = ResponsesClient::new("responses-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(true))
        .await
        .expect("responses request should succeed");
    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await.expect("event should parse") {
        events.push(event);
    }

    let text_events = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                    delta: ContentBlockDelta::TextDelta { text },
                    ..
                }) if text == "finalized answer"
            )
        })
        .count();
    assert_eq!(text_events, 1);
    assert!(matches!(events.last(), Some(StreamEvent::MessageStop(_))));
}

#[tokio::test]
async fn responses_transport_does_not_lose_finalized_text_after_empty_delta() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = concat!(
        "data: {\"type\":\"response.output_item.added\",\"response\":{\"id\":\"resp_empty_delta\"},\"item\":{\"type\":\"message\",\"id\":\"msg_1\",\"content\":[]}}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"response\":{\"id\":\"resp_empty_delta\"},\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"\"}\n\n",
        "data: {\"type\":\"response.output_text.done\",\"response\":{\"id\":\"resp_empty_delta\"},\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"text\":\"finalized after empty delta\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_empty_delta\",\"status\":\"completed\",\"usage\":{\"input_tokens\":7,\"output_tokens\":4}}}\n\n"
    );
    let Some(server) = spawn_server(
        state,
        vec![http_response("200 OK", "text/event-stream", sse)],
    )
    .await
    else {
        return;
    };

    let client = ResponsesClient::new("responses-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(true))
        .await
        .expect("responses request should succeed");
    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await.expect("event should parse") {
        events.push(event);
    }

    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            delta: ContentBlockDelta::TextDelta { text },
            ..
        }) if text == "finalized after empty delta"
    )));
    assert!(matches!(events.last(), Some(StreamEvent::MessageStop(_))));
}

#[tokio::test]
async fn responses_transport_accepts_text_only_streams() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"response\":{\"id\":\"resp_text\"},\"delta\":\"hello\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_text\",\"status\":\"completed\",\"usage\":{\"input_tokens\":5,\"output_tokens\":1}}}\n\n"
    );
    let Some(server) = spawn_server(
        state,
        vec![http_response("200 OK", "text/event-stream", sse)],
    )
    .await
    else {
        return;
    };
    let client = ResponsesClient::new("responses-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(true))
        .await
        .expect("responses request should succeed");
    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await.expect("event should parse") {
        events.push(event);
    }
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            delta: ContentBlockDelta::TextDelta { text },
            ..
        }) if text == "hello"
    )));
    assert!(events
        .iter()
        .any(|event| matches!(event, StreamEvent::MessageStop(_))));
}

#[tokio::test]
async fn responses_transport_classifies_completed_empty_output_with_metadata() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_empty\",\"status\":\"completed\",\"output\":[{\"type\":\"reasoning\"}],\"usage\":{\"input_tokens\":7,\"output_tokens\":2,\"input_tokens_details\":{\"cached_tokens\":3}}}}\n\n";
    let Some(server) = spawn_server(
        state,
        vec![http_response("200 OK", "text/event-stream", sse)],
    )
    .await
    else {
        return;
    };
    let client = ResponsesClient::new("responses-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(true))
        .await
        .expect("responses request should succeed");
    let error = loop {
        match stream.next_event().await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("empty response should be classified as an error"),
            Err(error) => break error,
        }
    };
    match error {
        ApiError::NonActionableResponse(response) => {
            assert_eq!(response.response_id.as_deref(), Some("resp_empty"));
            assert_eq!(response.kind, ResponseOutcomeKind::Empty);
            assert_eq!(response.output_types, vec!["reasoning"]);
            assert_eq!(response.input_tokens, 7);
            assert_eq!(response.output_tokens, 2);
            assert_eq!(response.cached_input_tokens, 3);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn responses_transport_does_not_retry_refusal_outcomes() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_refusal\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"refusal\",\"refusal\":\"cannot comply\"}]}]}}\n\n";
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response("200 OK", "text/event-stream", sse)],
    )
    .await
    else {
        return;
    };
    let client = ResponsesClient::new("responses-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(true))
        .await
        .expect("responses request should succeed");
    let error = loop {
        match stream.next_event().await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("refusal should be classified as an error"),
            Err(error) => break error,
        }
    };
    match error {
        ApiError::NonActionableResponse(response) => {
            assert_eq!(response.kind, ResponseOutcomeKind::Refusal);
            assert!(
                !ApiError::NonActionableResponse(Box::new(api::NonActionableResponse {
                    model: "model".to_string(),
                    response_id: None,
                    request_id: None,
                    status: Some("completed".to_string()),
                    kind: response.kind,
                    output_types: vec!["message".to_string(), "refusal".to_string()],
                    input_tokens: 0,
                    output_tokens: 0,
                    cached_input_tokens: 0,
                    failure_class: None,
                    provider_error_code: None,
                    provider_error_message: None,
                    incomplete_details: None,
                }))
                .is_retryable()
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(state.lock().await.len(), 1);
}

#[tokio::test]
async fn responses_transport_classifies_transient_failed_output_for_bounded_recovery() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = "data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp_failed\",\"status\":\"failed\",\"error\":{\"code\":\"server_error\",\"message\":\"temporary upstream failure\"}}}\n\n";
    let Some(server) = spawn_server(
        state,
        vec![http_response("200 OK", "text/event-stream", sse)],
    )
    .await
    else {
        return;
    };
    let client = ResponsesClient::new("responses-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(true))
        .await
        .expect("responses request should succeed");
    let error = loop {
        match stream.next_event().await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("failed response should be classified as an error"),
            Err(error) => break error,
        }
    };
    let ApiError::NonActionableResponse(response) = error else {
        panic!("unexpected error type")
    };
    assert_eq!(response.kind, ResponseOutcomeKind::Failed);
    assert_eq!(response.response_id.as_deref(), Some("resp_failed"));
    assert_eq!(
        response.failure_class,
        Some(ProviderFailureClass::Transient)
    );
    assert_eq!(
        response.provider_error_code.as_deref(),
        Some("server_error")
    );
    assert_eq!(
        response.provider_error_message.as_deref(),
        Some("temporary upstream failure")
    );
    assert!(ApiError::NonActionableResponse(response).is_retryable());
}

#[tokio::test]
async fn responses_transport_does_not_retry_permanent_failed_output() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = "data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp_invalid\",\"status\":\"failed\",\"error\":{\"code\":\"invalid_request_error\",\"message\":\"request is invalid\"}}}\n\n";
    let Some(server) = spawn_server(
        state,
        vec![http_response("200 OK", "text/event-stream", sse)],
    )
    .await
    else {
        return;
    };
    let client = ResponsesClient::new("responses-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(true))
        .await
        .expect("responses request should succeed");
    let error = loop {
        match stream.next_event().await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("failed response should be classified as an error"),
            Err(error) => break error,
        }
    };
    let ApiError::NonActionableResponse(response) = error else {
        panic!("unexpected error type")
    };
    assert_eq!(
        response.failure_class,
        Some(ProviderFailureClass::Permanent)
    );
    assert!(!ApiError::NonActionableResponse(response).is_retryable());
}

#[tokio::test]
async fn responses_transport_rejects_partial_output_from_incomplete_response() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"response\":{\"id\":\"resp_incomplete\"},\"delta\":\"partial\"}\n\n",
        "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_incomplete\",\"status\":\"incomplete\",\"usage\":{\"input_tokens\":2,\"output_tokens\":1}}}\n\n"
    );
    let Some(server) = spawn_server(
        state,
        vec![http_response("200 OK", "text/event-stream", sse)],
    )
    .await
    else {
        return;
    };
    let client = ResponsesClient::new("responses-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(true))
        .await
        .expect("responses request should succeed");
    let error = loop {
        match stream.next_event().await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("incomplete response should be classified as an error"),
            Err(error) => break error,
        }
    };
    match error {
        ApiError::NonActionableResponse(response) => {
            assert_eq!(response.kind, ResponseOutcomeKind::Incomplete);
            assert_eq!(response.status.as_deref(), Some("incomplete"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn explicit_profile_capabilities_are_protocol_not_provider_identity() {
    let capabilities = EndpointCapabilities {
        chat_completions: false,
        responses: true,
        ..EndpointCapabilities::default()
    };
    assert!(capabilities.responses);
    assert!(!capabilities.chat_completions);
}

#[tokio::test]
async fn send_message_preserves_generic_reasoning_before_text() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let body = concat!(
        "{\"id\":\"chatcmpl_reasoning\",\"model\":\"qwen3:latest\",",
        "\"choices\":[{\"message\":{\"role\":\"assistant\",",
        "\"reasoning\":\"Think locally\",\"content\":\"Answer locally\",",
        "\"tool_calls\":[]},\"finish_reason\":\"stop\"}],",
        "\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}"
    );
    let Some(server) = spawn_server(
        state,
        vec![http_response("200 OK", "application/json", body)],
    )
    .await
    else {
        return;
    };

    let client = OpenAiCompatClient::new("ollama-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let response = client
        .send_message(&sample_request(false))
        .await
        .expect("request should succeed");

    assert_eq!(
        response.content,
        vec![
            OutputContentBlock::Thinking {
                thinking: "Think locally".to_string(),
                signature: None,
            },
            OutputContentBlock::Text {
                text: "Answer locally".to_string(),
            },
        ]
    );
}

#[tokio::test]
async fn stream_message_preserves_generic_reasoning_before_text() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = concat!(
        "data: {\"id\":\"chatcmpl_reasoning_stream\",\"model\":\"qwen3:latest\",\"choices\":[{\"delta\":{\"reasoning\":\"Think\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl_reasoning_stream\",\"choices\":[{\"delta\":{\"content\":\" answer\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let Some(server) = spawn_server(
        state,
        vec![http_response("200 OK", "text/event-stream", sse)],
    )
    .await
    else {
        return;
    };

    let client = OpenAiCompatClient::new("ollama-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(true))
        .await
        .expect("stream should start");
    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await.expect("event should parse") {
        events.push(event);
    }

    assert!(matches!(
        events[1],
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index: 0,
            content_block: OutputContentBlock::Thinking { .. },
        })
    ));
    assert!(matches!(
        events[2],
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: 0,
            delta: ContentBlockDelta::ThinkingDelta { .. },
        })
    ));
    assert!(matches!(
        events[3],
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 0 })
    ));
    assert!(matches!(
        events[4],
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index: 1,
            content_block: OutputContentBlock::Text { .. },
        })
    ));
}

#[tokio::test]
async fn stream_message_accepts_reasoning_then_tool_call_without_text() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = [
        json!({
            "id": "chatcmpl_tool_reasoning",
            "model": "qwen3.5:4b",
            "choices": [{"delta": {"reasoning": "Think"}}]
        }),
        json!({
            "id": "chatcmpl_tool_reasoning",
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": "call_1",
                "function": {"name": "write_file", "arguments": "{\"path\":\"result.txt\""}
            }]}}]
        }),
        json!({
            "id": "chatcmpl_tool_reasoning",
            "choices": [{
                "delta": {"tool_calls": [{
                    "index": 0,
                    "function": {"arguments": ":\"OK\"}"}
                }]},
                "finish_reason": "tool_calls"
            }]
        }),
    ]
    .into_iter()
    .fold(String::new(), |mut sse, chunk| {
        let _ = writeln!(sse, "data: {chunk}");
        sse.push('\n');
        sse
    }) + "data: [DONE]\n\n";
    let Some(server) = spawn_server(
        state,
        vec![http_response("200 OK", "text/event-stream", &sse)],
    )
    .await
    else {
        return;
    };

    let client = OpenAiCompatClient::new("ollama-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(true))
        .await
        .expect("stream should start");
    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await.expect("event should parse") {
        events.push(event);
    }

    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            delta: ContentBlockDelta::ThinkingDelta { .. },
            ..
        })
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            delta: ContentBlockDelta::InputJsonDelta { partial_json },
            ..
        }) if partial_json.contains("OK")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::MessageDelta(MessageDeltaEvent {
            delta: api::MessageDelta {
                stop_reason: Some(reason),
                ..
            },
            ..
        }) if reason == "tool_use"
    )));
}

#[tokio::test]
async fn send_message_blocks_oversized_xai_requests_before_the_http_call() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response("200 OK", "application/json", "{}")],
    )
    .await
    else {
        return;
    };

    let client = OpenAiCompatClient::new("xai-test-key", OpenAiCompatConfig::xai())
        .with_base_url(server.base_url());
    let error = client
        .send_message(&MessageRequest {
            model: "grok-3".to_string(),
            max_tokens: 64_000,
            messages: vec![InputMessage {
                role: "user".to_string(),
                content: vec![InputContentBlock::Text {
                    text: "x".repeat(300_000),
                }],
            }],
            system: Some("Keep the answer short.".to_string()),
            tools: None,
            tool_choice: None,
            stream: false,
            ..Default::default()
        })
        .await
        .expect_err("oversized request should fail local context-window preflight");

    assert!(matches!(error, ApiError::ContextWindowExceeded { .. }));
    assert!(
        state.lock().await.is_empty(),
        "preflight failure should avoid any upstream HTTP request"
    );
}

#[tokio::test]
async fn send_message_accepts_full_chat_completions_endpoint_override() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let body = concat!(
        "{",
        "\"id\":\"chatcmpl_full_endpoint\",",
        "\"model\":\"grok-3\",",
        "\"choices\":[{",
        "\"message\":{\"role\":\"assistant\",\"content\":\"Endpoint override works\",\"tool_calls\":[]},",
        "\"finish_reason\":\"stop\"",
        "}],",
        "\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3}",
        "}"
    );
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response("200 OK", "application/json", body)],
    )
    .await
    else {
        return;
    };

    let endpoint_url = format!("{}/chat/completions", server.base_url());
    let client = OpenAiCompatClient::new("xai-test-key", OpenAiCompatConfig::xai())
        .with_base_url(endpoint_url);
    let response = client
        .send_message(&sample_request(false))
        .await
        .expect("request should succeed");

    assert_eq!(response.total_tokens(), 10);

    let captured = state.lock().await;
    let request = captured.first().expect("server should capture request");
    assert_eq!(request.path, "/chat/completions");
}

#[tokio::test]
async fn stream_message_normalizes_text_and_multiple_tool_calls() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = concat!(
        "data: {\"id\":\"chatcmpl_stream\",\"model\":\"grok-3\",\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl_stream\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"weather\",\"arguments\":\"{\\\"city\\\":\\\"Paris\\\"}\"}},{\"index\":1,\"id\":\"call_2\",\"function\":{\"name\":\"clock\",\"arguments\":\"{\\\"zone\\\":\\\"UTC\\\"}\"}}]}}]}\n\n",
        "data: {\"id\":\"chatcmpl_stream\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response_with_headers(
            "200 OK",
            "text/event-stream",
            sse,
            &[("x-request-id", "req_grok_stream")],
        )],
    )
    .await
    else {
        return;
    };

    let client = OpenAiCompatClient::new("xai-test-key", OpenAiCompatConfig::xai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(false))
        .await
        .expect("stream should start");

    assert_eq!(stream.request_id(), Some("req_grok_stream"));

    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await.expect("event should parse") {
        events.push(event);
    }

    assert!(matches!(events[0], StreamEvent::MessageStart(_)));
    assert!(matches!(
        events[1],
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            content_block: OutputContentBlock::Text { .. },
            ..
        })
    ));
    assert!(matches!(
        events[2],
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            delta: ContentBlockDelta::TextDelta { .. },
            ..
        })
    ));
    assert!(matches!(
        events[3],
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index: 1,
            content_block: OutputContentBlock::ToolUse { .. },
        })
    ));
    assert!(matches!(
        events[4],
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: 1,
            delta: ContentBlockDelta::InputJsonDelta { .. },
        })
    ));
    assert!(matches!(
        events[5],
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index: 2,
            content_block: OutputContentBlock::ToolUse { .. },
        })
    ));
    assert!(matches!(
        events[6],
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: 2,
            delta: ContentBlockDelta::InputJsonDelta { .. },
        })
    ));
    assert!(matches!(
        events[7],
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 1 })
    ));
    assert!(matches!(
        events[8],
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 2 })
    ));
    assert!(matches!(
        events[9],
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 0 })
    ));
    assert!(matches!(events[10], StreamEvent::MessageDelta(_)));
    assert!(matches!(events[11], StreamEvent::MessageStop(_)));

    let captured = state.lock().await;
    let request = captured.first().expect("captured request");
    assert_eq!(request.path, "/chat/completions");
    assert!(request.body.contains("\"stream\":true"));
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn openai_streaming_requests_opt_into_usage_chunks() {
    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let sse = concat!(
        "data: {\"id\":\"chatcmpl_openai_stream\",\"model\":\"gpt-5\",\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl_openai_stream\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"id\":\"chatcmpl_openai_stream\",\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4}}\n\n",
        "data: [DONE]\n\n"
    );
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response_with_headers(
            "200 OK",
            "text/event-stream",
            sse,
            &[("x-request-id", "req_openai_stream")],
        )],
    )
    .await
    else {
        return;
    };

    let client = OpenAiCompatClient::new("openai-test-key", OpenAiCompatConfig::openai())
        .with_base_url(server.base_url());
    let mut stream = client
        .stream_message(&sample_request(false))
        .await
        .expect("stream should start");

    assert_eq!(stream.request_id(), Some("req_openai_stream"));

    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await.expect("event should parse") {
        events.push(event);
    }

    assert!(matches!(events[0], StreamEvent::MessageStart(_)));
    assert!(matches!(
        events[1],
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            content_block: OutputContentBlock::Text { .. },
            ..
        })
    ));
    assert!(matches!(
        events[2],
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            delta: ContentBlockDelta::TextDelta { .. },
            ..
        })
    ));
    assert!(matches!(
        events[3],
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 0 })
    ));
    assert!(matches!(
        events[4],
        StreamEvent::MessageDelta(MessageDeltaEvent { .. })
    ));
    assert!(matches!(events[5], StreamEvent::MessageStop(_)));

    match &events[4] {
        StreamEvent::MessageDelta(MessageDeltaEvent { usage, .. }) => {
            assert_eq!(usage.input_tokens, 9);
            assert_eq!(usage.output_tokens, 4);
        }
        other => panic!("expected message delta, got {other:?}"),
    }

    let captured = state.lock().await;
    let request = captured.first().expect("captured request");
    assert_eq!(request.path, "/chat/completions");
    let body: serde_json::Value = serde_json::from_str(&request.body).expect("json body");
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn provider_client_dispatches_xai_requests_from_env() {
    let _lock = env_lock();
    let _api_key = ScopedEnvVar::set("XAI_API_KEY", "xai-test-key");

    let state = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let Some(server) = spawn_server(
        state.clone(),
        vec![http_response(
            "200 OK",
            "application/json",
            "{\"id\":\"chatcmpl_provider\",\"model\":\"grok-3\",\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"Through provider client\",\"tool_calls\":[]},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4}}",
        )],
    )
    .await
    else {
        return;
    };
    let _base_url = ScopedEnvVar::set("XAI_BASE_URL", server.base_url());

    let client =
        ProviderClient::from_model("grok").expect("xAI provider client should be constructed");
    assert!(matches!(client, ProviderClient::Xai(_)));

    let response = client
        .send_message(&sample_request(false))
        .await
        .expect("provider-dispatched request should succeed");

    assert_eq!(response.total_tokens(), 13);

    let captured = state.lock().await;
    let request = captured.first().expect("captured request");
    assert_eq!(request.path, "/chat/completions");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer xai-test-key")
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedRequest {
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

struct TestServer {
    base_url: String,
    join_handle: tokio::task::JoinHandle<()>,
}

impl TestServer {
    fn base_url(&self) -> String {
        self.base_url.clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

async fn spawn_server(
    state: Arc<Mutex<Vec<CapturedRequest>>>,
    responses: Vec<String>,
) -> Option<TestServer> {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return None;
        }
        Err(error) => {
            panic!("listener should bind: {error}");
        }
    };
    let address = listener.local_addr().expect("listener addr");
    let join_handle = tokio::spawn(async move {
        for response in responses {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = Vec::new();
            let mut header_end = None;
            loop {
                let mut chunk = [0_u8; 1024];
                let read = socket.read(&mut chunk).await.expect("read request");
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(position) = find_header_end(&buffer) {
                    header_end = Some(position);
                    break;
                }
            }

            let header_end = header_end.expect("headers should exist");
            let (header_bytes, remaining) = buffer.split_at(header_end);
            let header_text = String::from_utf8(header_bytes.to_vec()).expect("utf8 headers");
            let mut lines = header_text.split("\r\n");
            let request_line = lines.next().expect("request line");
            let path = request_line
                .split_whitespace()
                .nth(1)
                .expect("path")
                .to_string();
            let mut headers = HashMap::new();
            let mut content_length = 0_usize;
            for line in lines {
                if line.is_empty() {
                    continue;
                }
                let (name, value) = line.split_once(':').expect("header");
                let value = value.trim().to_string();
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.parse().expect("content length");
                }
                headers.insert(name.to_ascii_lowercase(), value);
            }

            let mut body = remaining[4..].to_vec();
            while body.len() < content_length {
                let mut chunk = vec![0_u8; content_length - body.len()];
                let read = socket.read(&mut chunk).await.expect("read body");
                if read == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..read]);
            }

            state.lock().await.push(CapturedRequest {
                path,
                headers,
                body: String::from_utf8(body).expect("utf8 body"),
            });

            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        }
    });

    Some(TestServer {
        base_url: format!("http://{address}"),
        join_handle,
    })
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn http_response(status: &str, content_type: &str, body: &str) -> String {
    http_response_with_headers(status, content_type, body, &[])
}

fn http_response_with_headers(
    status: &str,
    content_type: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> String {
    let mut extra_headers = String::new();
    for (name, value) in headers {
        use std::fmt::Write as _;
        write!(&mut extra_headers, "{name}: {value}\r\n").expect("header write");
    }
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\n{extra_headers}content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn sample_request(stream: bool) -> MessageRequest {
    MessageRequest {
        model: "grok-3".to_string(),
        max_tokens: 64,
        messages: vec![InputMessage {
            role: "user".to_string(),
            content: vec![InputContentBlock::Text {
                text: "Say hello".to_string(),
            }],
        }],
        system: Some("Use tools when needed".to_string()),
        tools: Some(vec![ToolDefinition {
            name: "weather".to_string(),
            description: Some("Fetches weather".to_string()),
            input_schema: json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"]
            }),
        }]),
        tool_choice: Some(ToolChoice::Auto),
        stream,
        ..Default::default()
    }
}

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| StdMutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<OsString>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}
