//! Shared pipeline and Assist LLM hook (no Tauri / WebView dependency).
//!
//! Standard mode stays offline. Assist may call an OpenAI-compatible provider
//! or the mock fixture backend used in CI.

pub mod assist;
pub mod pipeline;

pub use assist::{
    interpret_freeform_context, iso_gap_narrative, looks_like_freeform_context, AssistConfig,
    AssistRuntime, AssistStatus, LlmProvider, MockLlmProvider, OfflineProvider,
    OpenAiCompatibleProvider,
};
pub use pipeline::{
    run_pipeline, ContextRuleset, IsoRuleset, RunLogLine, RunRequest, RunResult, TemplateRuleset,
};

/// Re-export template preview / trait picker types for the Tauri shell.
pub use docx_core::{
    inspect_template, DetectedTrait, TemplatePreview, TemplatePreviewSummary, TraitKind,
    TraitSelection, TransformOptions,
};
