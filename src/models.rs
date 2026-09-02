//! Scripted completion models, so the real Rig agent loop runs with no API key.
//!
//! This is Rig's own pattern for credential-free examples (`runtime_model_routing`):
//! a `CompletionModel` that returns a tool call, then, once the tool result is in
//! history, the next step. The agent loop, tool dispatch, and history handling are
//! all real; only the model's choices are pre-written.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::stream;
use rig::completion::{CompletionError, CompletionModel, CompletionRequest, CompletionResponse, Usage};
use rig::message::{AssistantContent, ToolCall, ToolFunction};
use rig::streaming::{RawStreamingChoice, RawStreamingToolCall, StreamFinal, StreamingCompletionResponse};

#[derive(Clone)]
pub enum Step {
    Call { tool: &'static str, args: serde_json::Value },
    Say(&'static str),
}

/// Plays `steps` in order, one per model turn.
#[derive(Clone)]
pub struct ScriptedModel {
    name: &'static str,
    steps: Arc<Vec<Step>>,
    turn: Arc<AtomicUsize>,
}

impl ScriptedModel {
    pub fn new(name: &'static str, steps: Vec<Step>) -> Self {
        Self { name, steps: Arc::new(steps), turn: Arc::new(AtomicUsize::new(0)) }
    }

    fn next(&self) -> Step {
        let i = self.turn.fetch_add(1, Ordering::SeqCst);
        self.steps.get(i).cloned().unwrap_or(Step::Say("Done."))
    }
}

fn usage(total_tokens: u64) -> Usage {
    Usage { total_tokens, ..Usage::new() }
}

impl CompletionModel for ScriptedModel {
    async fn completion(&self, _request: CompletionRequest) -> Result<CompletionResponse, CompletionError> {
        let choice = match self.next() {
            Step::Call { tool, args } => AssistantContent::ToolCall(ToolCall::from_wire(
                format!("{tool}-call"),
                ToolFunction::new(tool.to_owned(), args),
            )),
            Step::Say(text) => AssistantContent::text(text),
        };
        Ok(CompletionResponse::new(vec![choice], usage(1), self.name))
    }

    async fn stream(&self, _request: CompletionRequest) -> Result<StreamingCompletionResponse, CompletionError> {
        let choice = match self.next() {
            Step::Call { tool, args } => RawStreamingChoice::ToolCall(RawStreamingToolCall::new(
                format!("{tool}-call"),
                tool.to_owned(),
                args,
            )),
            Step::Say(text) => RawStreamingChoice::Message(text.to_owned()),
        };
        Ok(StreamingCompletionResponse::stream(
            self.name,
            Box::pin(stream::iter([
                Ok(choice),
                Ok(RawStreamingChoice::FinalResponse(StreamFinal::new(self.name, usage(1)))),
            ])),
        ))
    }
}
