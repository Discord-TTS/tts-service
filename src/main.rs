#![warn(clippy::pedantic)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::similar_names
)]

use std::{
    fmt::Display,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};

use axum::{http::header::HeaderValue, response::Response, routing::get, Json};
use bytes::Bytes;
use serde_json::to_value;
use sha2::{
    digest::{consts::U32, generic_array::GenericArray},
    Digest,
};
use small_fixed_array::{FixedString, ValidLength};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod espeak;
mod gcloud;
mod gtts;
mod polly;
mod translation;

type Result<T, E = anyhow::Error> = std::result::Result<T, E>;
type ResponseResult<T> = std::result::Result<T, Error>;
type AudioCacheDigest = GenericArray<u8, U32>;

#[must_use]
pub fn check_mp3_length(audio: &[u8], max_length: u64) -> bool {
    use bytes::Buf;
    mp3_duration::from_read(&mut audio.reader()).map_or(true, |d| d.as_secs() < max_length)
}

pub struct DeadlineMonitor<F: FnOnce(Duration)> {
    start: Instant,
    expected: Duration,
    on_not_hit: Option<F>,
    hit_any_deadline: Arc<AtomicBool>,
}

impl<F: FnOnce(Duration)> DeadlineMonitor<F> {
    pub fn new(expected: Duration, hit_any_deadline: Arc<AtomicBool>, on_not_hit: F) -> Self {
        Self {
            expected,
            hit_any_deadline,
            start: Instant::now(),
            on_not_hit: Some(on_not_hit),
        }
    }
}

impl<F: FnOnce(Duration)> Drop for DeadlineMonitor<F> {
    fn drop(&mut self) {
        let Some(time_passed) = Instant::now().checked_duration_since(self.start) else {
            tracing::debug!("Time went backwards!");
            return;
        };

        if time_passed > self.expected && !self.hit_any_deadline.swap(true, Ordering::Relaxed) {
            (self.on_not_hit.take().unwrap())(time_passed);
        }
    }
}

#[derive(serde::Deserialize)]
struct GetVoices {
    mode: TTSMode,
    #[serde(default)]
    raw: bool,
}

async fn get_voices(
    axum::extract::Query(payload): axum::extract::Query<GetVoices>,
) -> ResponseResult<impl axum::response::IntoResponse> {
    let GetVoices { mode, raw } = payload;
    let state = STATE.get().unwrap();

    Ok(axum::Json(if raw {
        match mode {
            TTSMode::gTTS => to_value(gtts::get_raw_voices()),
            TTSMode::eSpeak => to_value(espeak::get_voices()),
            TTSMode::Polly => to_value(polly::get_raw_voices(&state.polly).await?),
            TTSMode::gCloud => to_value(gcloud::get_raw_voices(&state.gcloud).await?),
        }?
    } else {
        to_value(match mode {
            TTSMode::gTTS => gtts::get_voices(),
            TTSMode::eSpeak => espeak::get_voices().to_vec(),
            TTSMode::Polly => polly::get_voices(&state.polly).await?,
            TTSMode::gCloud => gcloud::get_voices(&state.gcloud).await?,
        })?
    }))
}

async fn get_translation_languages() -> ResponseResult<Json<Vec<(FixedString, FixedString)>>> {
    let state = STATE.get().unwrap();
    let Some(token) = &state.translation_key else {
        return Ok(Json(Vec::new()));
    };

    match translation::get_languages(&state.reqwest, token).await {
        Ok(languages) => Ok(Json(languages)),
        Err(err) => Err(Error::Unknown(err)),
    }
}

#[derive(serde::Deserialize, Debug)]
struct GetTTS {
    text: FixedString,
    mode: TTSMode,
    #[serde(rename = "lang")]
    voice: FixedString<u8>,
    #[serde(default)]
    speaking_rate: Option<f32>,
    max_length: Option<u64>,
    #[serde(default)]
    preferred_format: Option<FixedString<u8>>,
    #[serde(default)]
    translation_lang: Option<FixedString<u8>>,
}

#[expect(clippy::too_many_lines)]
async fn get_tts(
    axum::extract::Query(payload): axum::extract::Query<GetTTS>,
    headers: axum::http::HeaderMap,
) -> ResponseResult<Response<axum::body::Body>> {
    if payload.text.contains("SHOW TO DEVS") {
        tracing::debug!("Recieved request to TTS: {payload:?}");
    }

    let hit_any_deadline = Arc::new(AtomicBool::new(false));
    let _guard = DeadlineMonitor::new(
        Duration::from_millis(5000),
        hit_any_deadline.clone(),
        |took| {
            tracing::warn!("get_tts took {} millis!", took.as_millis());
        },
    );

    let state = STATE.get().unwrap();
    if let Some(auth_key) = state.auth_key.as_deref() {
        let auth_header = headers.get("Authorization");
        if auth_header.map(HeaderValue::to_str).transpose()? != Some(auth_key) {
            return Err(Error::Unauthorized);
        }
    }

    let translation_lang = payload.translation_lang;
    let preferred_format = payload.preferred_format;
    let speaking_rate = payload.speaking_rate;
    let mut text = payload.text;
    let voice = payload.voice;
    let mode = payload.mode;

    mode.check_speaking_rate(speaking_rate)?;
    mode.check_voice(state, &voice).await?;

    let mut cache_key = format!("{text} {voice} {mode} {}", speaking_rate.unwrap_or(0.0));

    if let Some(preferred_format) = &preferred_format {
        cache_key.push(' ');
        cache_key.push_str(preferred_format);
    }

    if let Some(translation_lang) = &translation_lang {
        cache_key.push(' ');
        cache_key.push_str(translation_lang);
    }

    tracing::debug!("Recieved request to TTS: {cache_key}");

    let cache_hash = {
        let _guard = DeadlineMonitor::new(
            Duration::from_millis(50),
            hit_any_deadline.clone(),
            |took| {
                tracing::warn!(
                    "Fetching from audio cache took {} millis!",
                    took.as_millis()
                );
            },
        );

        let cache_hash = sha2::Sha256::digest(&cache_key);
        if let Some(cached_audio) = state.cache.get(&cache_hash) {
            mode.check_length(&cached_audio, payload.max_length)?;

            tracing::debug!("Used cached TTS for {cache_key}");
            return Ok(mode.into_response(cached_audio, None));
        }

        cache_hash
    };

    if let Some(language) = translation_lang {
        let Some(token) = &state.translation_key else {
            return Err(Error::TranslationDisabled);
        };

        let _guard = DeadlineMonitor::new(
            Duration::from_millis(200),
            hit_any_deadline.clone(),
            |took| {
                tracing::warn!("Fetching translation took {} millis!", took.as_millis());
            },
        );

        if let Some(translated) = translation::run(&state.reqwest, token, &text, &language).await? {
            text = translated;
        }
    };

    let (audio, content_type) = match mode {
        TTSMode::gTTS => {
            gtts::get_tts(&state.gtts, &text, &voice, hit_any_deadline.clone()).await?
        }
        TTSMode::eSpeak => {
            espeak::get_tts(&text, &voice, speaking_rate.map_or(0, |r| r as u16)).await?
        }
        TTSMode::Polly => {
            polly::get_tts(
                &state.polly,
                text,
                &voice,
                speaking_rate.map(|r| r as u8),
                preferred_format.as_deref(),
            )
            .await?
        }
        TTSMode::gCloud => {
            gcloud::get_tts(
                &state.gcloud,
                &text,
                &voice,
                speaking_rate.unwrap_or(0.0),
                preferred_format.as_deref(),
            )
            .await?
        }
    };

    tracing::debug!("Generated TTS from {cache_key}");
    {
        let _guard = DeadlineMonitor::new(
            Duration::from_millis(50),
            hit_any_deadline.clone(),
            |took| {
                tracing::warn!("Caching audio result took {} millis!", took.as_millis());
            },
        );

        tracing::debug!("Cached {} kb of audio", (audio.len() as f64) / 1024.0);
        state.cache.insert(cache_hash, audio.clone());
    };

    mode.check_length(&audio, payload.max_length)?;
    Ok(mode.into_response(audio, content_type))
}

#[derive(serde::Deserialize, Clone, Copy, Debug)]
#[allow(non_camel_case_types)]
enum TTSMode {
    gTTS,
    Polly,
    eSpeak,
    gCloud,
}

impl TTSMode {
    fn into_response(
        self,
        data: Bytes,
        content_type: Option<reqwest::header::HeaderValue>,
    ) -> Response {
        Response::builder()
            .header(
                axum::http::header::CONTENT_TYPE,
                content_type.unwrap_or_else(|| {
                    HeaderValue::from_static(match self {
                        Self::gTTS => "audio/mpeg",
                        Self::eSpeak => "audio/wav",
                        Self::gCloud => "audio/opus",
                        Self::Polly => "audio/ogg",
                    })
                }),
            )
            .body(axum::body::Body::from(data))
            .unwrap()
    }

    async fn check_voice(self, state: &State, voice: &str) -> ResponseResult<()> {
        if match self {
            Self::gTTS => gtts::check_voice(voice),
            Self::eSpeak => espeak::check_voice(voice),
            Self::gCloud => gcloud::check_voice(&state.gcloud, voice).await?,
            Self::Polly => polly::check_voice(&state.polly, voice).await?,
        } {
            Ok(())
        } else {
            Err(Error::UnknownVoice(
                format!("Unknown voice: {voice}").into_boxed_str(),
            ))
        }
    }

    fn check_length(self, audio: &[u8], max_length: Option<u64>) -> ResponseResult<()> {
        if max_length.map_or(true, |max_length| match self {
            Self::gTTS => check_mp3_length(audio, max_length),
            Self::eSpeak => espeak::check_length(audio, max_length as u32),
            Self::gCloud | Self::Polly => true,
        }) {
            Ok(())
        } else {
            Err(Error::AudioTooLong)
        }
    }

    fn check_speaking_rate(self, speaking_rate: Option<f32>) -> ResponseResult<()> {
        if let Some(speaking_rate) = speaking_rate {
            if let Some(max) = self.max_speaking_rate() {
                if speaking_rate > max {
                    return Err(Error::InvalidSpeakingRate(speaking_rate));
                }
            }
        }

        Ok(())
    }

    const fn max_speaking_rate(self) -> Option<f32> {
        match self {
            Self::gTTS => None,
            Self::Polly => Some(500.0),
            Self::eSpeak => Some(400.0),
            Self::gCloud => Some(4.0),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::gTTS => "gTTS",
            Self::Polly => "Polly",
            Self::eSpeak => "eSpeak",
            Self::gCloud => "gCloud",
        }
    }
}

impl Display for TTSMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl serde::Serialize for TTSMode {
    fn serialize<S>(&self, serializer: S) -> std::prelude::v1::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct State {
    auth_key: Option<FixedString<u8>>,
    translation_key: Option<FixedString<u8>>,
    reqwest: reqwest::Client,

    cache: mini_moka::sync::Cache<AudioCacheDigest, Bytes>,

    polly: polly::State,
    gtts: tokio::sync::RwLock<gtts::State>,
    gcloud: tokio::sync::RwLock<gcloud::State>,
}

static STATE: OnceLock<State> = OnceLock::new();

fn str_to_fixedstring<LenT: ValidLength>(str: String) -> FixedString<LenT> {
    FixedString::try_from(str.into_boxed_str()).expect("string should be less than 256 chars long")
}

#[tokio::main]
async fn main() -> Result<()> {
    let log_env = std::env::var("LOG_LEVEL");

    let fmt_layer = tracing_subscriber::fmt::layer();
    let filter = LevelFilter::from_str(log_env.as_deref().unwrap_or("INFO"))?;

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(filter)
        .init();

    let ip_block = match std::env::var("IPV6_BLOCK") {
        Ok(ip_block) if &ip_block == "DISABLE" => None,
        Ok(ip_block) => Some(ip_block.parse().expect("Invalid IPV6 Block!")),
        _ => panic!("IPV6_BLOCK not set! Set to \"DISABLE\" to disable rate limit bypass"),
    };

    let client = reqwest::Client::new();
    let result = STATE.set(State {
        reqwest: client.clone(),
        gcloud: gcloud::State::new(client)?,
        polly: polly::State::new(&aws_config::load_from_env().await),
        gtts: tokio::sync::RwLock::new(gtts::get_random_ipv6(ip_block).await?),

        cache: {
            let max_cap = std::env::var("CACHE_MAX_CAPACITY")
                .ok()
                .and_then(|c| c.parse().ok())
                .unwrap_or(1000);

            let cache = mini_moka::sync::Cache::builder()
                .max_capacity(max_cap)
                .build();

            tracing::info!("Initialised audio cache with max capacity: {max_cap}");
            cache
        },

        auth_key: std::env::var("AUTH_KEY").ok().map(str_to_fixedstring),
        translation_key: std::env::var("DEEPL_KEY").ok().map(str_to_fixedstring),
    });

    if result.is_err() {
        unreachable!()
    }

    let app = axum::Router::new()
        .route("/tts", get(get_tts))
        .route("/voices", get(get_voices))
        .route("/translation_languages", get(get_translation_languages))
        .route(
            "/modes",
            get(|| async {
                axum::Json([
                    TTSMode::gTTS,
                    TTSMode::Polly,
                    TTSMode::eSpeak,
                    TTSMode::gCloud,
                ])
            }),
        );

    let env_addr = std::env::var("BIND_ADDR");
    let bind_to = env_addr.as_deref().unwrap_or("0.0.0.0:3000");

    tracing::info!("Binding to {bind_to}...");

    let listener = tokio::net::TcpListener::bind(bind_to).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

#[derive(Debug)]
enum Error {
    Unauthorized,
    TranslationDisabled,
    UnknownVoice(Box<str>),
    AudioTooLong,
    InvalidSpeakingRate(f32),

    Unknown(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for Error {
    fn from(e: E) -> Self {
        Self::Unknown(e.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSpeakingRate(rate) => write!(f, "Invalid speaking rate: {rate}"),
            Self::AudioTooLong => f.write_str("Max length exceeded!"),
            Self::UnknownVoice(msg) => f.write_str(msg),
            Self::Unauthorized => write!(f, "Unauthorized request"),
            Self::TranslationDisabled => {
                write!(f, "Translation requested but no key has been provided")
            }
            Self::Unknown(e) => write!(f, "Unknown error: {e}"),
        }
    }
}

impl axum::response::IntoResponse for Error {
    fn into_response(self) -> Response {
        if let Error::Unknown(inner) = &self {
            tracing::error!("{inner:?}");
        };

        let json_err = serde_json::json!({
            "display": self.to_string(),
            "code": match self {
                Self::TranslationDisabled => 5,
                Self::Unauthorized => 4,
                Self::InvalidSpeakingRate(_) => 3,
                Self::AudioTooLong => 2,
                Self::UnknownVoice(_) => 1,
                Self::Unknown(_) => 0_u8,
            },
        });

        let status = match self {
            Self::AudioTooLong | Self::InvalidSpeakingRate(_) | Self::TranslationDisabled => {
                axum::http::StatusCode::BAD_REQUEST
            }
            Self::Unknown(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Self::UnknownVoice(_) => axum::http::StatusCode::BAD_REQUEST,
            Self::Unauthorized => axum::http::StatusCode::FORBIDDEN,
        };

        (status, axum::Json(json_err)).into_response()
    }
}
