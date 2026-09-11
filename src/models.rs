//! Scripted completion models, so the real Rig agent loop runs with no API key.
//!
//! This is Rig's own pattern for credential-free examples (`runtime_model_routing`):
//! a `CompletionModel` that returns a tool call, then, once the tool result is in
//! history, the next step. The agent loop, tool dispatch, and history handling are
//! all real; only the model's choices are pre-written.

use std::sync::Arc;

use futures::stream;
use rig::completion::{
    CompletionError, CompletionModel, CompletionRequest, CompletionResponse, Usage,
};
use rig::message::{AssistantContent, Message, ToolCall, ToolFunction, UserContent};
use rig::streaming::{
    RawStreamingChoice, RawStreamingToolCall, StreamFinal, StreamingCompletionResponse,
};

#[derive(Clone)]
pub enum Step {
    Call {
        tool: &'static str,
        args: serde_json::Value,
    },
    /// Several tool calls in one turn. Rig runs them concurrently under `tool_concurrency(n)`.
    Calls(Vec<(&'static str, serde_json::Value)>),
    /// Produce a response by inspecting the actual tool results in Rig's history.
    Respond(fn(&CompletionRequest) -> String),
}

/// Plays `steps` in order, one per model turn.
#[derive(Clone)]
pub struct ScriptedModel {
    name: &'static str,
    steps: Arc<Vec<Step>>,
}

impl ScriptedModel {
    pub fn new(name: &'static str, steps: Vec<Step>) -> Self {
        Self {
            name,
            steps: Arc::new(steps),
        }
    }

    fn next(&self, request: &CompletionRequest) -> (usize, Step) {
        // A completed model turn is represented by an assistant tool-call message.
        // Deriving state from history makes clones/retries independent and reusable.
        let turn = request
            .chat_history
            .iter()
            .filter(|message| {
                matches!(message, Message::Assistant { content, .. }
                    if content.iter().any(|item| matches!(item, AssistantContent::ToolCall(_))))
            })
            .count();
        let step = self
            .steps
            .get(turn)
            .cloned()
            .unwrap_or(Step::Respond(|_| "Done.".into()));
        (turn, step)
    }
}

fn tool_call(
    model: &str,
    tool: &'static str,
    args: serde_json::Value,
    turn: usize,
    i: usize,
) -> AssistantContent {
    AssistantContent::ToolCall(ToolCall::from_wire(
        format!("{model}-turn-{turn}-{tool}-call-{i}"),
        ToolFunction::new(tool.to_owned(), args),
    ))
}

fn stream_call(
    model: &str,
    tool: &'static str,
    args: serde_json::Value,
    turn: usize,
    i: usize,
) -> RawStreamingChoice {
    RawStreamingChoice::ToolCall(RawStreamingToolCall::new(
        format!("{model}-turn-{turn}-{tool}-call-{i}"),
        tool.to_owned(),
        args,
    ))
}

/// Flatten the model-visible results Rig supplied after tool execution.
pub fn tool_result_texts(request: &CompletionRequest) -> Vec<(&str, String)> {
    request
        .chat_history
        .iter()
        .filter_map(|message| match message {
            Message::User { content } => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|item| match item {
            UserContent::ToolResult(result) => Some(result),
            _ => None,
        })
        .map(|result| {
            let text = result
                .content
                .iter()
                .map(|content| {
                    content
                        .as_text()
                        .map(str::to_owned)
                        .or_else(|| content.as_json().map(serde_json::Value::to_string))
                        .unwrap_or_else(|| "[non-text result]".into())
                })
                .collect::<Vec<_>>()
                .join("\n");
            (result.name.as_str(), text)
        })
        .collect()
}

fn usage(total_tokens: u64) -> Usage {
    Usage {
        total_tokens,
        ..Usage::new()
    }
}

impl CompletionModel for ScriptedModel {
    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse, CompletionError> {
        let (turn, step) = self.next(&request);
        let choices = match step {
            Step::Call { tool, args } => vec![tool_call(self.name, tool, args, turn, 0)],
            Step::Calls(calls) => calls
                .into_iter()
                .enumerate()
                .map(|(i, (t, a))| tool_call(self.name, t, a, turn, i))
                .collect(),
            Step::Respond(respond) => vec![AssistantContent::text(respond(&request))],
        };
        Ok(CompletionResponse::new(choices, usage(1), self.name))
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<StreamingCompletionResponse, CompletionError> {
        let (turn, step) = self.next(&request);
        let mut items: Vec<Result<RawStreamingChoice, CompletionError>> = match step {
            Step::Call { tool, args } => {
                vec![Ok(stream_call(self.name, tool, args, turn, 0))]
            }
            Step::Calls(calls) => calls
                .into_iter()
                .enumerate()
                .map(|(i, (t, a))| Ok(stream_call(self.name, t, a, turn, i)))
                .collect(),
            Step::Respond(respond) => {
                vec![Ok(RawStreamingChoice::Message(respond(&request)))]
            }
        };
        items.push(Ok(RawStreamingChoice::FinalResponse(StreamFinal::new(
            self.name,
            usage(1),
        ))));
        Ok(StreamingCompletionResponse::stream(
            self.name,
            Box::pin(stream::iter(items)),
        ))
    }
}
