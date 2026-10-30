//! Typed, independent speech provider settings. No models are loaded or downloaded.
use reader_library::Settings;
use reader_speech::ProviderConfig;
use serde::Serialize;
use std::{collections::BTreeMap, path::PathBuf};

const PROVIDERS: [&str; 9] = [
    "irodori",
    "kokoro",
    "aivis",
    "aivis_native",
    "voiceroid2",
    "system",
    "openai",
    "voicevox",
    "command",
];

#[derive(Serialize)]
struct SavedProfiles<'a> {
    version: u32,
    profiles: &'a BTreeMap<String, ProviderConfig>,
}

// Deliberately no Debug: provider profiles may contain API keys.
pub struct SpeechProfiles {
    profiles: BTreeMap<String, ProviderConfig>,
}

impl SpeechProfiles {
    /// Loading supplies typed defaults and checks candidate native asset paths.
    /// It never constructs a synthesizer, reads model weights, or downloads assets.
    pub fn load(settings: &Settings) -> Self {
        let mut profiles: BTreeMap<_, _> = PROVIDERS
            .into_iter()
            .map(|id| (id.to_owned(), default_profile(id)))
            .collect();
        let mut envelope = false;
        let mut active_config_present = false;
        if let Some(saved) = &settings.tts.provider_config {
            if saved.get("version").and_then(serde_json::Value::as_u64) == Some(1)
                && let Some(entries) = saved.get("profiles").and_then(serde_json::Value::as_object)
            {
                envelope = true;
                // Parse individually: one malformed entry must not discard a
                // valid API key, command, or model path for another provider.
                for (id, value) in entries {
                    if let Ok(config) = serde_json::from_value::<ProviderConfig>(value.clone())
                        && provider_id(&config) == id
                    {
                        profiles.insert(id.clone(), config);
                    }
                }
            } else if let Ok(config) = serde_json::from_value::<ProviderConfig>(saved.clone()) {
                let id = provider_id(&config);
                active_config_present = id == settings.tts.provider_id;
                profiles.insert(id.to_owned(), config);
            }
        }
        // The oldest settings only stored one voice alongside provider_id.
        // Import it once for that provider; an envelope's typed profiles always
        // win over this compatibility mirror after migration.
        if !envelope
            && !active_config_present
            && let Some(voice) = settings
                .tts
                .voice
                .as_ref()
                .filter(|voice| !voice.is_empty())
        {
            match profiles.get_mut(&settings.tts.provider_id) {
                Some(ProviderConfig::System { voice: stored }) => *stored = Some(voice.clone()),
                Some(ProviderConfig::OpenAi { voice: stored, .. }) => *stored = voice.clone(),
                Some(ProviderConfig::Kokoro { voice: stored, .. }) => *stored = voice.clone(),
                Some(ProviderConfig::Voiceroid2 { voice: stored, .. }) => *stored = voice.clone(),
                Some(ProviderConfig::Voicevox { speaker, .. }) => {
                    if let Ok(value) = voice.parse() {
                        *speaker = value;
                    }
                }
                Some(ProviderConfig::AivisSpeech { speaker, .. }) => {
                    if let Ok(value) = voice.parse() {
                        *speaker = value;
                    }
                }
                _ => {}
            }
        }
        Self { profiles }
    }

    pub fn get(&self, id: &str) -> ProviderConfig {
        self.profiles
            .get(id)
            .cloned()
            .unwrap_or_else(|| default_profile(id))
    }

    /// The enum is authoritative if a caller accidentally supplies a different
    /// ID. Never place OpenAI credentials in another provider's profile.
    pub fn set(&mut self, id: &str, config: ProviderConfig) {
        let actual = provider_id(&config);
        let key = if actual == id { id } else { actual };
        self.profiles.insert(key.to_owned(), config);
    }

    /// Update only speech selection/configuration. Reading speed, auto-follow,
    /// appearance, and other unrelated settings remain owned by the caller.
    pub fn write(&self, active_id: &str, settings: &mut Settings) {
        let active = self.get(active_id);
        if let Ok(saved) = serde_json::to_value(SavedProfiles {
            version: 1,
            profiles: &self.profiles,
        }) {
            settings.tts.provider_id = provider_id(&active).to_owned();
            settings.tts.provider_config = Some(saved);
            // Compatibility mirror for older application versions. Profiles
            // retain each provider's voice independently.
            settings.tts.voice = match active {
                ProviderConfig::System { voice } => voice,
                ProviderConfig::OpenAi { voice, .. } => (!voice.is_empty()).then_some(voice),
                ProviderConfig::Kokoro { voice, .. } => (!voice.is_empty()).then_some(voice),
                ProviderConfig::Voiceroid2 { voice, .. } => (!voice.is_empty()).then_some(voice),
                ProviderConfig::Voicevox { speaker, .. } => Some(speaker.to_string()),
                ProviderConfig::AivisSpeech { speaker, .. } => Some(speaker.to_string()),
                _ => None,
            };
        }
    }
}

fn default_profile(id: &str) -> ProviderConfig {
    match id {
        "aivis" => ProviderConfig::aivis_default(),
        "aivis_native" => ProviderConfig::aivis_native_default(),
        "voiceroid2" => ProviderConfig::voiceroid2_default(),
        "kokoro" => ProviderConfig::kokoro_default(),
        "system" => ProviderConfig::System { voice: None },
        "openai" => ProviderConfig::OpenAi {
            base_url: "http://127.0.0.1:8880/v1".into(),
            api_key: String::new(),
            model: "kokoro".into(),
            voice: "jf_alpha".into(),
        },
        "voicevox" => ProviderConfig::Voicevox {
            base_url: "http://127.0.0.1:50021".into(),
            speaker: 0,
        },
        "command" => ProviderConfig::Command {
            program: PathBuf::new(),
            args: Vec::new(),
        },
        _ => ProviderConfig::default(),
    }
}

pub fn provider_id(config: &ProviderConfig) -> &'static str {
    match config {
        ProviderConfig::Irodori { .. } => "irodori",
        ProviderConfig::Kokoro { .. } => "kokoro",
        ProviderConfig::System { .. } => "system",
        ProviderConfig::OpenAi { .. } => "openai",
        ProviderConfig::Voicevox { .. } => "voicevox",
        ProviderConfig::AivisSpeech { .. } => "aivis",
        ProviderConfig::AivisNative { .. } => "aivis_native",
        ProviderConfig::Voiceroid2 { .. } => "voiceroid2",
        ProviderConfig::Command { .. } => "command",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_aivis_local_voice_ids_and_models_are_separate_from_http_style_ids() {
        let config = ProviderConfig::AivisNative {
            runtime_dir: "native runtime".into(),
            model_path: "custom/日本語.aivmx".into(),
            bert_dir: "native bert".into(),
            speaker: 2,
            style: 6,
            backend: "cuda".into(),
            auto_download: false,
            allow_nvidia_download: true,
        };
        let mut settings = Settings::default();
        let mut profiles = SpeechProfiles::load(&settings);
        let http = profiles.get("aivis");
        profiles.set("aivis_native", config.clone());
        profiles.write("aivis_native", &mut settings);
        assert_eq!(settings.tts.provider_id, "aivis_native");
        let mut reloaded = SpeechProfiles::load(&settings);
        assert_eq!(json(reloaded.get("aivis_native")), json(config));
        assert_eq!(json(reloaded.get("aivis")), json(http));
        reloaded.set("aivis_native", ProviderConfig::aivis_native_default());
        assert_ne!(json(reloaded.get("aivis_native"))["style"], 6);
    }

    fn json(config: ProviderConfig) -> serde_json::Value {
        serde_json::to_value(config).unwrap()
    }
    fn custom_openai() -> ProviderConfig {
        ProviderConfig::OpenAi {
            base_url: "https://speech.example.test/custom/v1".into(),
            api_key: "fixture-key-never-sent".into(),
            model: "custom-japanese-model".into(),
            voice: "ja-custom-voice".into(),
        }
    }
    fn custom_irodori() -> ProviderConfig {
        ProviderConfig::Irodori {
            runtime_dir: PathBuf::from("custom speech/audio.cpp"),
            model_path: PathBuf::from("custom speech/日本語モデル.gguf"),
            backend: "cpu".into(),
            auto_download: false,
            aligner_path: Some(PathBuf::from("custom speech/aligner.gguf")),
            reference_audio: Some(PathBuf::from("custom speech/決めた声.wav")),
            voice_instruction: "落ち着いた柔らかい声".into(),
            seed: 123,
        }
    }

    #[test]
    fn all_advanced_profiles_roundtrip_through_unrelated_settings_saves() {
        let mut settings = Settings::default();
        let mut profiles = SpeechProfiles::load(&settings);
        let command = ProviderConfig::Command {
            program: PathBuf::from("C:/speech engine/音声.exe"),
            args: vec![
                "--config".into(),
                "path with spaces/config.json".into(),
                "literal$argument".into(),
            ],
        };
        profiles.set("irodori", custom_irodori());
        profiles.set("openai", custom_openai());
        profiles.set("command", command.clone());
        profiles.write("voicevox", &mut settings);
        settings.font_size = 31.;
        settings.tts.speed = 1.75;
        settings.tts.auto_follow = false;
        let disk = serde_json::to_string(&settings).unwrap();
        let mut restored: Settings = serde_json::from_str(&disk).unwrap();
        let profiles = SpeechProfiles::load(&restored);
        profiles.write("system", &mut restored);
        let reloaded = SpeechProfiles::load(&restored);
        assert_eq!(json(reloaded.get("irodori")), json(custom_irodori()));
        assert_eq!(json(reloaded.get("openai")), json(custom_openai()));
        assert_eq!(json(reloaded.get("command")), json(command));
        assert_eq!(restored.font_size, 31.);
        assert_eq!(restored.tts.speed, 1.75);
        assert!(!restored.tts.auto_follow);
        assert_eq!(restored.tts.provider_config.as_ref().unwrap()["version"], 1);
    }

    #[test]
    fn migrates_legacy_direct_configuration_without_losing_advanced_fields() {
        for config in [
            custom_irodori(),
            custom_openai(),
            ProviderConfig::Command {
                program: PathBuf::from("tts.exe"),
                args: vec!["--ja".into(), "space value".into()],
            },
        ] {
            let id = provider_id(&config);
            let mut settings = Settings::default();
            settings.tts.provider_id = id.into();
            settings.tts.provider_config = Some(json(config.clone()));
            settings.tts.voice = Some("obsolete-global-voice".into());
            let profiles = SpeechProfiles::load(&settings);
            profiles.write("system", &mut settings);
            let restored = SpeechProfiles::load(&settings);
            assert_eq!(json(restored.get(id)), json(config));
        }
    }

    #[test]
    fn voices_remain_independent_when_active_provider_changes() {
        let mut settings = Settings::default();
        let mut profiles = SpeechProfiles::load(&settings);
        profiles.set(
            "system",
            ProviderConfig::System {
                voice: Some("Microsoft Haruka Desktop".into()),
            },
        );
        profiles.set("openai", custom_openai());
        profiles.set(
            "voicevox",
            ProviderConfig::Voicevox {
                base_url: "http://localhost:50021".into(),
                speaker: 42,
            },
        );
        for (active, voice) in [
            ("system", "Microsoft Haruka Desktop"),
            ("openai", "ja-custom-voice"),
            ("voicevox", "42"),
        ] {
            profiles.write(active, &mut settings);
            assert_eq!(settings.tts.voice.as_deref(), Some(voice));
            profiles = SpeechProfiles::load(&settings);
        }
        assert_eq!(json(profiles.get("openai")), json(custom_openai()));
        assert_eq!(
            json(profiles.get("system"))["voice"],
            "Microsoft Haruka Desktop"
        );
        assert_eq!(json(profiles.get("voicevox"))["speaker"], 42);
    }

    #[test]
    fn imports_old_standalone_voice_only_for_the_active_provider() {
        let mut settings = Settings::default();
        settings.tts.provider_id = "system".into();
        settings.tts.voice = Some("legacy-system-voice".into());
        let profiles = SpeechProfiles::load(&settings);
        assert_eq!(json(profiles.get("system"))["voice"], "legacy-system-voice");
        assert_eq!(json(profiles.get("openai"))["voice"], "jf_alpha");
        profiles.write("system", &mut settings);
        settings.tts.voice = Some("stale-mirror".into());
        assert_eq!(
            json(SpeechProfiles::load(&settings).get("system"))["voice"],
            "legacy-system-voice"
        );
    }

    #[test]
    fn stable_json_does_not_depend_on_profile_insertion_order() {
        let mut first = SpeechProfiles::load(&Settings::default());
        let mut second = SpeechProfiles::load(&Settings::default());
        first.set("irodori", custom_irodori());
        first.set("openai", custom_openai());
        second.set("openai", custom_openai());
        second.set("irodori", custom_irodori());
        let mut left = Settings::default();
        let mut right = Settings::default();
        first.write("openai", &mut left);
        second.write("openai", &mut right);
        assert_eq!(
            serde_json::to_string(&left.tts.provider_config).unwrap(),
            serde_json::to_string(&right.tts.provider_config).unwrap()
        );
    }

    #[test]
    fn malformed_profile_does_not_discard_other_saved_providers() {
        let mut settings = Settings::default();
        settings.tts.provider_config = Some(serde_json::json!({
            "version": 1, "profiles": { "openai": json(custom_openai()), "voicevox": { "kind": "voicevox", "speaker": "invalid" } }
        }));
        let profiles = SpeechProfiles::load(&settings);
        assert_eq!(json(profiles.get("openai")), json(custom_openai()));
        assert_eq!(json(profiles.get("voicevox"))["speaker"], 0);
        assert_eq!(provider_id(&profiles.get("unknown")), "irodori");
    }

    #[test]
    fn data_operations_do_not_create_runtime_or_model_assets() {
        let path = std::env::temp_dir().join(format!(
            "yomi-profile-no-download-{}-{}",
            std::process::id(),
            reader_library::unix_millis()
        ));
        assert!(!path.exists());
        let mut settings = Settings::default();
        let config = ProviderConfig::Irodori {
            runtime_dir: path.join("runtime"),
            model_path: path.join("model.gguf"),
            backend: "cpu".into(),
            auto_download: true,
            aligner_path: Some(path.join("aligner.gguf")),
            reference_audio: None,
            voice_instruction: String::new(),
            seed: 42,
        };
        let mut profiles = SpeechProfiles::load(&settings);
        profiles.set("irodori", config.clone());
        profiles.write("irodori", &mut settings);
        assert_eq!(
            json(SpeechProfiles::load(&settings).get("irodori")),
            json(config)
        );
        assert!(!path.exists());
    }

    #[test]
    fn old_irodori_profile_without_voice_conditioning_gets_stable_defaults() {
        let mut settings = Settings::default();
        settings.tts.provider_config = Some(
            serde_json::json!({"version":1,"profiles":{"irodori":{"kind":"irodori","runtime_dir":"old/runtime","model_path":"old/model.gguf","backend":"vulkan","auto_download":false,"aligner_path":"old/aligner.gguf"}}}),
        );
        let profiles = SpeechProfiles::load(&settings);
        let value = json(profiles.get("irodori"));
        assert_eq!(value["runtime_dir"], "old/runtime");
        assert_eq!(value["seed"], 42);
        assert!(value["reference_audio"].is_null());
        assert_eq!(value["voice_instruction"], "");
        let mut saved = settings;
        profiles.write("irodori", &mut saved);
        assert_eq!(json(SpeechProfiles::load(&saved).get("irodori")), value);
    }

    #[test]
    fn builtin_kokoro_keeps_advanced_paths_and_voice_independent_of_external_kokoro() {
        let mut profiles = SpeechProfiles::load(&Settings::default());
        let kokoro = ProviderConfig::Kokoro {
            runtime_dir: "kokoro/runtime".into(),
            model_path: "kokoro/custom.onnx".into(),
            backend: "cpu".into(),
            auto_download: false,
            allow_nvidia_download: true,
            voice: "jm_kumo".into(),
            aligner_path: None,
        };
        profiles.set("kokoro", kokoro.clone());
        profiles.set("openai", custom_openai());
        let mut settings = Settings::default();
        profiles.write("kokoro", &mut settings);
        assert_eq!(settings.tts.provider_id, "kokoro");
        assert_eq!(settings.tts.voice.as_deref(), Some("jm_kumo"));
        settings.font_size = 30.;
        settings.theme = reader_library::Theme::Dark;
        let loaded = SpeechProfiles::load(&settings);
        assert_eq!(json(loaded.get("kokoro")), json(kokoro));
        assert_eq!(json(loaded.get("openai")), json(custom_openai()));
        assert_eq!(provider_id(&loaded.get("kokoro")), "kokoro");
    }

    #[test]
    fn aivis_keeps_signed_style_and_runtime_separate_from_voicevox() {
        let mut profiles = SpeechProfiles::load(&Settings::default());
        let aivis = ProviderConfig::AivisSpeech {
            base_url: "http://127.0.0.1:10111".into(),
            speaker: -123456,
            engine_path: Some("custom/AivisSpeech/run.exe".into()),
            auto_start: false,
        };
        profiles.set("aivis", aivis.clone());
        profiles.set(
            "voicevox",
            ProviderConfig::Voicevox {
                base_url: "http://localhost:50021".into(),
                speaker: 42,
            },
        );
        let mut settings = Settings::default();
        profiles.write("aivis", &mut settings);
        assert_eq!(settings.tts.voice.as_deref(), Some("-123456"));
        let mut restored = SpeechProfiles::load(&settings);
        restored.write("voicevox", &mut settings);
        restored = SpeechProfiles::load(&settings);
        assert_eq!(json(restored.get("aivis")), json(aivis));
        assert_eq!(json(restored.get("voicevox"))["speaker"], 42);
    }

    #[test]
    fn voiceroid_voice_and_tuning_survive_switches_without_changing_other_voices() {
        let mut profiles = SpeechProfiles::load(&Settings::default());
        let voiceroid = ProviderConfig::Voiceroid2 {
            install_dir: r"D:\Installed\VOICEROID2".into(),
            voice: "aoi_emo_44".into(),
            volume: 0.8,
            pitch: 1.2,
            range: 0.7,
        };
        profiles.set("voiceroid2", voiceroid.clone());
        let kokoro = profiles.get("kokoro");
        let mut settings = Settings::default();
        profiles.write("voiceroid2", &mut settings);
        assert_eq!(settings.tts.voice.as_deref(), Some("aoi_emo_44"));
        let restored = SpeechProfiles::load(&settings);
        restored.write("kokoro", &mut settings);
        let restored = SpeechProfiles::load(&settings);
        assert_eq!(json(restored.get("voiceroid2")), json(voiceroid));
        assert_eq!(json(restored.get("kokoro")), json(kokoro));
    }
}
