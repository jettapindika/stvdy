//! The `LlmProvider` contract and the scripted double every chat test leans
//! on. If the double and the trait drift, every later phase's chat test drifts
//! with them, so the double's behaviour is asserted here rather than assumed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stvdy_core::traits::testing::ScriptedLlm;
use stvdy_core::traits::{ChatMsg, ChatRequest, LlmError, LlmProvider, Role, StreamEvent};

fn request(content: &str) -> ChatRequest {
    ChatRequest {
        model: "test-model".to_string(),
        messages: vec![ChatMsg {
            role: Role::User,
            content: content.to_string(),
        }],
        tools: Vec::new(),
        temperature: 0.0,
        max_tokens: 64,
        json_mode: false,
    }
}

#[tokio::test]
async fn complete_returns_the_scripted_text() {
    let provider = ScriptedLlm::new(vec!["a three-way handshake".to_string()]);

    let response = provider
        .complete(request("how does TCP connect"))
        .await
        .unwrap();

    assert_eq!(response.text, "a three-way handshake");
    assert!(response.tool_calls.is_empty());
    assert_eq!(response.usage.in_tokens, 0);
}

#[tokio::test]
async fn complete_yields_each_scripted_response_in_order() {
    let provider = ScriptedLlm::new(vec!["first".to_string(), "second".to_string()]);

    assert_eq!(
        provider.complete(request("one")).await.unwrap().text,
        "first"
    );
    assert_eq!(
        provider.complete(request("two")).await.unwrap().text,
        "second"
    );
}

#[tokio::test]
async fn stream_yields_tokens_in_order_then_done() {
    let provider = ScriptedLlm::new(vec!["Hello there friend".to_string()]);

    let mut stream = provider.stream(request("hi")).await.unwrap();
    let mut tokens = Vec::new();
    loop {
        match stream.recv().await {
            Some(StreamEvent::Token(token)) => tokens.push(token),
            Some(StreamEvent::Done) => break,
            Some(other) => panic!("unexpected stream event {other:?}"),
            None => panic!("stream ended without a Done event"),
        }
    }

    assert_eq!(tokens, vec!["Hello ", "there ", "friend"]);
    assert_eq!(
        tokens.concat(),
        "Hello there friend",
        "concatenated tokens must reproduce the scripted text exactly"
    );
}

#[tokio::test]
async fn a_request_is_recorded_verbatim() {
    let provider = ScriptedLlm::new(vec!["ok".to_string()]);

    let sent = request("cite [1] please");
    let _ = provider.complete(sent).await.unwrap();

    let recorded = provider.requests();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].messages[0].content, "cite [1] please");
    assert_eq!(recorded[0].messages[0].role, Role::User);
    assert_eq!(recorded[0].model, "test-model");
    assert_eq!(recorded[0].max_tokens, 64);
}

#[tokio::test]
async fn stream_records_the_request_too() {
    let provider = ScriptedLlm::new(vec!["ok".to_string()]);

    let _ = provider.stream(request("streamed prompt")).await.unwrap();

    assert_eq!(provider.requests().len(), 1);
    assert_eq!(
        provider.requests()[0].messages[0].content,
        "streamed prompt"
    );
}

#[tokio::test]
async fn a_scripted_failure_surfaces_as_provider_error() {
    let provider = ScriptedLlm::failing("upstream is down");

    match provider.complete(request("anything")).await {
        Err(LlmError::Provider(message)) => assert_eq!(message, "upstream is down"),
        other => panic!("expected LlmError::Provider, got {other:?}"),
    }

    // The request is still recorded: a failed call is exactly the one a test
    // wants to inspect the prompt of.
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn a_scripted_failure_surfaces_on_stream_too() {
    let provider = ScriptedLlm::failing("upstream is down");

    match provider.stream(request("anything")).await {
        Err(LlmError::Provider(message)) => assert_eq!(message, "upstream is down"),
        other => panic!("expected LlmError::Provider, got {other:?}"),
    }
}

#[tokio::test]
async fn a_scripted_stream_ends_with_done_even_when_empty() {
    let provider = ScriptedLlm::new(vec![String::new()]);

    let mut stream = provider.stream(request("hi")).await.unwrap();
    assert!(matches!(stream.recv().await, Some(StreamEvent::Done)));
    assert!(stream.recv().await.is_none());
}
