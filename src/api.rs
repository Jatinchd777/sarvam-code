use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::config::Config;

#[derive(Debug, Clone, Default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON string produced by the model.
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
}

#[derive(Debug, Clone)]
pub enum Delta {
    Text(String),
    Reasoning(String),
}

/// Call the Sarvam chat-completions endpoint, preferring SSE streaming.
/// `on_delta` is called as tokens arrive; the complete reply is returned at the end.
pub async fn stream_chat<F>(
    client: &reqwest::Client,
    cfg: &Config,
    messages: &[Value],
    tools: &Value,
    cancel: &mut watch::Receiver<bool>,
    mut on_delta: F,
) -> Result<Reply>
where
    F: FnMut(Delta) + Send,
{
    let mut body = json!({
        "model": cfg.model,
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "max_tokens": cfg.max_tokens,
        "temperature": cfg.temperature,
        "stream": true,
    });
    if let Some(effort) = &cfg.effort {
        body["reasoning_effort"] = json!(effort);
    }

    let request = client
        .post(&cfg.api_url)
        .header("api-subscription-key", cfg.api_key.as_str())
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .json(&body);

    let response = tokio::select! {
        _ = wait_cancelled(cancel) => bail!("cancelled"),
        result = tokio::time::timeout(Duration::from_secs(cfg.http_timeout_secs), request.send()) => {
            match result {
                Err(_) => bail!("request timed out after {}s", cfg.http_timeout_secs),
                Ok(Err(e)) => return Err(e).context("cannot reach the Sarvam API"),
                Ok(Ok(response)) => response,
            }
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        bail!(
            "API error: HTTP {}: {}",
            status.as_u16(),
            first_chars(&text, 4000)
        );
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if content_type.contains("text/event-stream") {
        stream_sse(response, cfg, cancel, &mut on_delta).await
    } else {
        // The mock server (and some gateways) answer a streaming request with a
        // regular JSON body. Accept both shapes.
        let bytes = tokio::select! {
            _ = wait_cancelled(cancel) => bail!("cancelled"),
            result = tokio::time::timeout(
                Duration::from_secs(cfg.http_timeout_secs),
                response.bytes(),
            ) => {
                match result {
                    Err(_) => bail!("request timed out after {}s", cfg.http_timeout_secs),
                    Ok(Err(e)) => return Err(e).context("cannot read the response body"),
                    Ok(Ok(bytes)) => bytes,
                }
            }
        };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => parse_completion(&value, &mut on_delta),
            Err(_) => parse_buffered_sse(&bytes, &mut on_delta),
        }
    }
}

async fn stream_sse<F>(
    response: reqwest::Response,
    cfg: &Config,
    cancel: &mut watch::Receiver<bool>,
    on_delta: &mut F,
) -> Result<Reply>
where
    F: FnMut(Delta) + Send,
{
    let stream = response.bytes_stream();
    tokio::pin!(stream);

    let mut decoder = SseDecoder::new();
    let mut acc = Accumulator::default();
    let mut done = false;

    while !done {
        let next = tokio::select! {
            _ = wait_cancelled(cancel) => bail!("cancelled"),
            result = tokio::time::timeout(
                Duration::from_secs(cfg.http_timeout_secs),
                stream.next(),
            ) => result,
        };

        let chunk = match next {
            Err(_) => bail!("stream was idle for {}s", cfg.http_timeout_secs),
            Ok(None) => break,
            Ok(Some(Err(e))) => return Err(e).context("error while reading the stream"),
            Ok(Some(Ok(bytes))) => bytes,
        };

        for payload in decoder.push(&chunk) {
            if payload.trim() == "[DONE]" {
                done = true;
                break;
            }
            let value: Value = serde_json::from_str(&payload)
                .with_context(|| format!("invalid SSE payload: {}", first_chars(&payload, 300)))?;
            acc.apply_chunk(&value, on_delta)?;
        }
    }

    acc.finish()
}

fn parse_buffered_sse<F>(bytes: &[u8], on_delta: &mut F) -> Result<Reply>
where
    F: FnMut(Delta) + Send,
{
    let mut decoder = SseDecoder::new();
    let mut acc = Accumulator::default();
    for payload in decoder.push(bytes) {
        if payload.trim() == "[DONE]" {
            break;
        }
        let value: Value = serde_json::from_str(&payload)
            .with_context(|| format!("invalid SSE payload: {}", first_chars(&payload, 300)))?;
        acc.apply_chunk(&value, on_delta)?;
    }
    acc.finish()
}

fn parse_completion<F>(value: &Value, on_delta: &mut F) -> Result<Reply>
where
    F: FnMut(Delta) + Send,
{
    if let Some(error) = value.get("error") {
        bail!("API error: {error}");
    }
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .context("API returned no choices")?;

    let mut acc = Accumulator::default();
    if let Some(message) = choice.get("message") {
        acc.apply_message(message, on_delta);
    }
    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
        acc.finish_reason = Some(reason.to_string());
    }
    acc.finish()
}

#[derive(Default)]
struct Accumulator {
    content: String,
    reasoning: String,
    tool_calls: Vec<ToolCall>,
    finish_reason: Option<String>,
}

impl Accumulator {
    fn apply_chunk<F>(&mut self, value: &Value, on_delta: &mut F) -> Result<()>
    where
        F: FnMut(Delta) + Send,
    {
        if let Some(error) = value.get("error") {
            bail!("API error: {error}");
        }
        let Some(choices) = value.get("choices").and_then(Value::as_array) else {
            // Usage-only chunks have no choices.
            return Ok(());
        };
        for choice in choices {
            let message = choice
                .get("delta")
                .filter(|delta| !delta.is_null())
                .or_else(|| choice.get("message"))
                .cloned()
                .unwrap_or(Value::Null);
            self.apply_message(&message, on_delta);
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                self.finish_reason = Some(reason.to_string());
            }
        }
        Ok(())
    }

    fn apply_message<F>(&mut self, message: &Value, on_delta: &mut F)
    where
        F: FnMut(Delta) + Send,
    {
        if let Some(text) = message.get("content").and_then(Value::as_str)
            && !text.is_empty()
        {
            self.content.push_str(text);
            on_delta(Delta::Text(text.to_string()));
        }

        let reasoning = message
            .get("reasoning_content")
            .or_else(|| message.get("reasoning"))
            .and_then(Value::as_str);
        if let Some(text) = reasoning
            && !text.is_empty()
        {
            self.reasoning.push_str(text);
            on_delta(Delta::Reasoning(text.to_string()));
        }

        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                self.apply_tool_call(call);
            }
        }
    }

    fn apply_tool_call(&mut self, call: &Value) {
        let index = call
            .get("index")
            .and_then(Value::as_u64)
            .map(|i| i as usize)
            .unwrap_or(self.tool_calls.len());
        while self.tool_calls.len() <= index {
            self.tool_calls.push(ToolCall::default());
        }
        let slot = &mut self.tool_calls[index];

        if let Some(id) = call.get("id").and_then(Value::as_str)
            && !id.is_empty()
        {
            slot.id = id.to_string();
        }
        if let Some(function) = call.get("function") {
            if let Some(name) = function.get("name").and_then(Value::as_str)
                && !name.is_empty()
            {
                slot.name = name.to_string();
            }
            if let Some(arguments) = function.get("arguments") {
                match arguments {
                    Value::String(text) => slot.arguments.push_str(text),
                    Value::Null => {}
                    other => slot.arguments = other.to_string(),
                }
            }
        }
    }

    fn finish(mut self) -> Result<Reply> {
        for (index, call) in self.tool_calls.iter_mut().enumerate() {
            if call.name.trim().is_empty() {
                bail!("stream ended with an incomplete tool call");
            }
            if call.id.trim().is_empty() {
                call.id = format!("call_{index}");
            }
            if call.arguments.trim().is_empty() {
                call.arguments = "{}".to_string();
            }
        }
        Ok(Reply {
            content: self.content,
            reasoning: self.reasoning,
            tool_calls: self.tool_calls,
            finish_reason: self.finish_reason.unwrap_or_else(|| "stop".to_string()),
        })
    }
}

/// Incremental Server-Sent Events line decoder. Keeps a byte buffer so
/// UTF-8 characters and JSON lines may span arbitrary HTTP chunks.
pub struct SseDecoder {
    buffer: Vec<u8>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut payloads = Vec::new();
        while let Some(position) = self.buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=position).collect();
            let line = String::from_utf8_lossy(&line[..line.len().saturating_sub(1)]);
            let line = line.trim_end_matches('\r');
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.strip_prefix(' ').unwrap_or(data).trim_end();
                if !data.is_empty() {
                    payloads.push(data.to_string());
                }
            }
        }
        payloads
    }
}

async fn wait_cancelled(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

fn first_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_decoder_handles_split_chunks() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: {\"a\"").is_empty());
        let payloads = decoder.push(b":1}\n\ndata: [DONE]\n");
        assert_eq!(payloads, vec!["{\"a\":1}", "[DONE]"]);
    }

    #[test]
    fn sse_decoder_ignores_comments_and_crlf() {
        let mut decoder = SseDecoder::new();
        let payloads = decoder.push(b": keep-alive\r\nevent: message\r\ndata: {\"x\":2}\r\n\r\n");
        assert_eq!(payloads, vec!["{\"x\":2}"]);
    }

    #[test]
    fn accumulator_merges_tool_call_deltas() {
        let mut acc = Accumulator::default();
        let mut deltas = Vec::new();
        let mut sink = |d: Delta| match d {
            Delta::Text(t) => deltas.push(t),
            Delta::Reasoning(r) => deltas.push(r),
        };

        let first: Value = serde_json::from_str(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{\"pa"}}]},"finish_reason":null}]}"#,
        )
        .unwrap();
        let second: Value = serde_json::from_str(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"README.md\"}"}}]},"finish_reason":"tool_calls"}]}"#,
        )
        .unwrap();
        acc.apply_chunk(&first, &mut sink).unwrap();
        acc.apply_chunk(&second, &mut sink).unwrap();

        let reply = acc.finish().unwrap();
        assert_eq!(reply.finish_reason, "tool_calls");
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.tool_calls[0].id, "call_1");
        assert_eq!(reply.tool_calls[0].name, "read_file");
        assert_eq!(reply.tool_calls[0].arguments, "{\"path\":\"README.md\"}");
        assert!(deltas.is_empty());
    }

    #[test]
    fn non_streaming_completion_is_parsed() {
        let value: Value = serde_json::from_str(
            r#"{"choices":[{"message":{"role":"assistant","content":"hi","tool_calls":[{"id":"c1","type":"function","function":{"name":"bash","arguments":"{\"command\":\"ls\"}"}}]},"finish_reason":"tool_calls"}]}"#,
        )
        .unwrap();
        let mut text = String::new();
        let mut sink = |d: Delta| {
            if let Delta::Text(t) = d {
                text.push_str(&t);
            }
        };
        let reply = parse_completion(&value, &mut sink).unwrap();
        assert_eq!(text, "hi");
        assert_eq!(reply.content, "hi");
        assert_eq!(reply.tool_calls[0].name, "bash");
        assert_eq!(reply.finish_reason, "tool_calls");
    }
}
