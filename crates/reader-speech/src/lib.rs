//! Native speech synthesis, cancellation, and sample-clock driven playback.

mod aivis;
mod aivis_assets;
mod aivis_japanese;
mod aivis_model;
mod aivis_native;
mod audio;
mod audiobook;
mod cache;
mod engine;
mod kokoro_frontend;
mod kokoro_notices;
mod kokoro_onnx;
mod kokoro_ort_assets;
mod kokoro_timing;
mod native;
mod pronunciations;
mod providers;
mod types;
mod voiceroid;

pub use aivis::{AivisVoice, aivis_voices, prepare_aivis};
pub use aivis_assets::{aivis_native_catalog, import_aivis_native_model};
pub use aivis_model::{
    AivisModelCatalog, AivisModelDiagnostic, AivisModelEntry, AivisModelInfo, AivisSpeaker,
    AivisStyle, inspect_model as inspect_aivis_model,
};
pub use aivis_native::{AivisNativeAudio, AivisNativeMark, AivisNativePaths, NativeAivis};
pub use audiobook::*;
pub use engine::{SpeechController, SpeechFeeder};
pub use kokoro_notices::nvidia_runtime_terms;
pub use native::{IRODORI_VOICE_SAMPLE_TEXT, irodori_reference_path};
pub use pronunciations::{pronunciation_source_mask, validate_pronunciations};
pub use providers::{TtsProvider, create_provider, validate_endpoint};
pub use types::*;
pub use voiceroid::{VoiceroidVoice, voiceroid2_helper_path, voiceroid2_voices};
