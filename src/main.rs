#![warn(clippy::pedantic)]
#![allow(clippy::unused_async, clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_lossless)]

#[cfg(not(any(feature="gtts", feature="espeak", feature="gcloud", feature="polly")))]
compile_error!("Either feature `gtts`, `espeak`, `gcloud`, `polly` must be enabled!");

use std::{str::FromStr, fmt::Display, borrow::Cow};

use axum::{http::header::HeaderValue, response::Response};
use once_cell::sync::OnceCell;
use sha2::Digest;
use redis::AsyncCommands;
use serde_json::to_value;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[cfg(any(feature="polly", feature="gcloud"))] use std::fmt::Write as _;

#[cfg(feature="gtts")] mod gtts;
#[cfg(feature="polly")] mod polly;
#[cfg(feature="espeak")] mod espeak;
#[cfg(feature="gcloud")] mod gcloud;

type Result<T, E = anyhow::Error> = std::result::Result<T, E>;
type ResponseResult<T> = std::result::Result<T, Error>;

#[must_use]
#[cfg(feature = "gtts")]
pub fn check_mp3_length(audio: &[u8], max_length: u64) -> bool {
    use bytes::Buf;
    mp3_duration::from_read(&mut audio.reader()).map_or(true, |d| d.as_secs() < max_length)
}


#[derive(serde::Deserialize)]
struct GetVoices {
    mode: TTSMode,
    #[serde(default)]
    #[cfg(any(feature="gtts", feature="polly", feature="gcloud"))]
    raw: bool,
}

async fn get_voices(
    axum::extract::Query(payload): axum::extract::Query<GetVoices>
) -> ResponseResult<impl axum::response::IntoResponse> {
    cfg_if::cfg_if!(
        if #[cfg(any(feature="gtts", feature="polly", feature="gcloud"))]{
            let GetVoices{mode, raw} = payload;
        } else {
            let GetVoices{mode} = payload;
            let raw = true;
        }
    );

    #[cfg(any(feature="gcloud", feature="polly"))]
    let state = STATE.get().unwrap();

    Ok(axum::Json(
        if raw {match mode {
            #[cfg(feature="gtts")]   TTSMode::gTTS   => to_value(gtts::get_raw_voices()),
            #[cfg(feature="polly")]  TTSMode::Polly  => to_value(polly::get_raw_voices(&state.polly).await?),
            #[cfg(feature="gcloud")] TTSMode::gCloud => to_value(gcloud::get_raw_voices(&state.gcloud).await?),

            #[cfg(feature="espeak")] TTSMode::eSpeak => to_value(espeak::get_voices()),
        }?} else {to_value(match mode {
            #[cfg(feature="gtts")]   TTSMode::gTTS   => gtts::get_voices(),
            #[cfg(feature="espeak")] TTSMode::eSpeak => espeak::get_voices(),
            #[cfg(feature="polly")]  TTSMode::Polly  => polly::get_voices(&state.polly).await?,
            #[cfg(feature="gcloud")] TTSMode::gCloud => gcloud::get_voices(&state.gcloud).await?,
        })?},
    ))
}

#[derive(serde::Deserialize)]
struct GetTTS {
    text: String,
    mode: TTSMode,
    #[serde(rename="lang")] voice: String,
    #[serde(default)] speaking_rate: Option<f32>,
    #[cfg(any(feature="gtts", feature="espeak"))] max_length: Option<u64>,
    #[cfg(any(feature="polly", feature="gcloud"))] #[serde(default)] preferred_format: Option<String>,
}

async fn get_tts(
    axum::extract::Query(payload): axum::extract::Query<GetTTS>,
    headers: axum::http::HeaderMap,
) -> ResponseResult<Response<axum::body::Full<bytes::Bytes>>> {
    let state = STATE.get().unwrap();
    if let Some(auth_key) = state.auth_key.as_deref() {
        if headers.get("Authorization").map(HeaderValue::to_str).transpose()? != Some(auth_key) {
            return Err(Error::Unauthorized);
        }
    }

    #[cfg(any(feature="polly", feature="gcloud"))]
    let preferred_format = payload.preferred_format;
    let speaking_rate = payload.speaking_rate;
    let mut voice = payload.voice;
    let mode = payload.mode;
    let text = payload.text;

    #[cfg(any(feature="gcloud", feature="espeak"))]
    mode.check_speaking_rate(speaking_rate)?;
    voice = mode.check_voice(state, voice).await?;

    #[cfg_attr(not(any(feature="polly", feature="gcloud")), allow(unused_mut))]
    let mut cache_key = format!("{text} | {voice} | {mode} | {}", speaking_rate.unwrap_or(0.0));

    #[cfg(any(feature="polly", feature="gcloud"))]
    if let Some(preferred_format) = preferred_format.as_ref() {
        write!(cache_key, "| {preferred_format}").unwrap();
    }

    tracing::debug!("Recieved request to TTS: {cache_key}");

    let redis_info = if let Some(redis_state) = &state.redis {
        let cache_hash = {
            let mut hasher = sha2::Sha256::new();
            hasher.update(&cache_key);
            hasher.finalize()
        };

        let mut conn = redis_state.client.get().await?;
        let cached_audio = conn.get::<_, Option<String>>(&*cache_hash).await?
            .map(|enc| redis_state.key.decrypt(&enc))
            .transpose()?
            .map(bytes::Bytes::from);

        if let Some(cached_audio) = cached_audio {
            #[cfg(any(feature="gtts", feature="espeak"))]
            mode.check_length(&cached_audio, payload.max_length)?;

            tracing::debug!("Used cached TTS for {cache_key}");
            return mode.into_response(cached_audio, None);
        }

        Some((conn, &redis_state.key, cache_hash))
    } else {
        None
    };

    let (audio, content_type) = match mode {
        #[cfg(feature="gtts")] TTSMode::gTTS => gtts::get_tts(&state.gtts, &text, &voice).await?,
        #[cfg(feature="espeak")] TTSMode::eSpeak => espeak::get_tts(&text, &voice, speaking_rate.map_or(0, |r| r as u16)).await?,
        #[cfg(feature="polly")] TTSMode::Polly => polly::get_tts(&state.polly, text, &voice, speaking_rate.map(|r| r as u8), preferred_format).await?,
        #[cfg(feature="gcloud")] TTSMode::gCloud => gcloud::get_tts(&state.gcloud, &text, &voice, speaking_rate.unwrap_or(0.0), preferred_format).await?,
    };

    tracing::debug!("Generated TTS from {cache_key}");
    if let Some((mut redis_conn, key, cache_hash)) = redis_info {
        if let Err(err) = redis_conn.set::<_, _, ()>(&*cache_hash, key.encrypt(&audio)).await {
            tracing::error!("Failed to cache: {err}");
        } else {
            tracing::debug!("Cached TTS from {cache_key}");
        };
    };

    #[cfg(any(feature="gtts", feature="espeak"))]
    mode.check_length(&audio, payload.max_length)?;
    mode.into_response(audio, content_type)
}


#[derive(serde::Deserialize, Clone, Copy, Debug)]
#[allow(non_camel_case_types)]
enum TTSMode {
    #[cfg(feature="gtts")] gTTS,
    #[cfg(feature="polly")] Polly,
    #[cfg(feature="espeak")] eSpeak,
    #[cfg(feature="gcloud")] gCloud,
}

impl TTSMode {
    fn into_response<T: bytes::Buf>(self, data: T, content_type: Option<HeaderValue>) -> ResponseResult<Response<axum::body::Full<T>>> {
        Response::builder()
            .header(axum::http::header::CONTENT_TYPE, content_type.unwrap_or_else(|| HeaderValue::from_static(match self {
                #[cfg(feature="gtts")]    Self::gTTS    => "audio/mpeg",
                #[cfg(feature="espeak")]  Self::eSpeak  => "audio/wav",
                #[cfg(feature="gcloud")]  Self::gCloud  => "audio/opus",
                #[cfg(feature="polly")]   Self::Polly   => "audio/ogg",
            })))
            .body(axum::body::Full::new(data))
            .map_err(Into::into)
    }

    #[cfg_attr(not(feature="polly"), allow(unused_variables, clippy::unnecessary_wraps))]
    async fn check_voice(self, state: &State, voice: String) -> ResponseResult<String> {
        if match self {
            #[cfg(feature="gtts")]   Self::gTTS   => gtts::check_voice(&voice),
            #[cfg(feature="espeak")] Self::eSpeak => espeak::check_voice(&voice),
            #[cfg(feature="gcloud")] Self::gCloud => gcloud::check_voice(&state.gcloud, &voice).await?,
            #[cfg(feature="polly")]  Self::Polly  => polly::check_voice(&state.polly, &voice).await?,
        } {
            Ok(voice)
        } else {
            Err(Error::UnknownVoice(voice))
        }
    }

    #[cfg(any(feature="gtts", feature="espeak"))]
    #[allow(unused_variables)]
    fn check_length(self, audio: &[u8], max_length: Option<u64>) -> ResponseResult<()> {
        if max_length.map_or(true, |max_length| match self {
            #[cfg(feature="gtts")]    Self::gTTS    => check_mp3_length(audio, max_length),
            #[cfg(feature="espeak")]  Self::eSpeak  => espeak::check_length(audio, max_length as u32),
            #[cfg(feature="gcloud")]  Self::gCloud  => true,
            #[cfg(feature="polly")]   Self::Polly   => true,
        }) {
            Ok(())
        } else {
            Err(Error::AudioTooLong)
        }
    }

    #[cfg(any(feature="gcloud", feature="espeak"))]
    fn check_speaking_rate(self, speaking_rate: Option<f32>) -> ResponseResult<()> {
        if let Some(speaking_rate) = speaking_rate {
            if let Some(max) = self.max_speaking_rate() {
                if speaking_rate > max {
                    return Err(Error::InvalidSpeakingRate(speaking_rate))
                }
            }
        }

        Ok(())
    }

    #[allow(clippy::unnecessary_wraps)]
    #[cfg(any(feature="gcloud", feature="espeak"))]
    const fn max_speaking_rate(self) -> Option<f32> {
        match self {
            #[cfg(feature="gtts")]    Self::gTTS    => None,
            #[cfg(feature="polly")]   Self::Polly   => Some(500.0),
            #[cfg(feature="espeak")]  Self::eSpeak  => Some(400.0),
            #[cfg(feature="gcloud")]  Self::gCloud  => Some(4.0),
        }
    }
}

impl Display for TTSMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            #[cfg(feature="gtts")] Self::gTTS => "gTTS",
            #[cfg(feature="polly")]  Self::Polly => "Polly",
            #[cfg(feature="espeak")] Self::eSpeak => "eSpeak",
            #[cfg(feature="gcloud")] Self::gCloud => "gCloud",
        })
    }
}


struct RedisCache {
    client: deadpool_redis::Pool,
    key: fernet::Fernet
}

struct State {
    auth_key: Option<String>,
    redis: Option<RedisCache>,
    #[cfg(feature="polly")] polly: polly::State,
    #[cfg(feature="gtts")] gtts: tokio::sync::RwLock<gtts::State>,
    #[cfg(feature="gcloud")] gcloud: tokio::sync::RwLock<gcloud::State>
}

static STATE: OnceCell<State> = OnceCell::new();

#[tokio::main]
async fn main() -> Result<()> {
    let fmt_layer = tracing_subscriber::fmt::layer();
    let filter = tracing_subscriber::filter::LevelFilter::from_str(
        &std::env::var("LOG_LEVEL")
        .unwrap_or_else(|_| String::from("INFO"))
    )?;

    tracing_subscriber::registry().with(fmt_layer).with(filter).init();

    #[cfg(feature="espeak")] {
        // Init espeakng internally so we can fetch the voice path
        espeakng::initialise(None)?;
    }

    #[cfg(feature = "gcloud")]
    let reqwest_client = reqwest::Client::new();

    let redis_uri = std::env::var("REDIS_URI").ok();
    let result = STATE.set(State {
        #[cfg(feature="gcloud")] gcloud: gcloud::State::new(reqwest_client)?,
        #[cfg(feature="gtts")] gtts: tokio::sync::RwLock::new(gtts::get_random_ipv6().await?),
        #[cfg(feature="polly")] polly: polly::State::new(&aws_config::load_from_env().await),

        auth_key: std::env::var("AUTH_KEY").ok(),
        redis: redis_uri.as_ref().map(|uri| {
            let key = std::env::var("CACHE_KEY").expect("CACHE_KEY not set!");
            RedisCache {
                client: deadpool_redis::Config::from_url(uri).create_pool(Some(deadpool_redis::Runtime::Tokio1)).unwrap(),
                key: fernet::Fernet::new(&key).unwrap()
            }
        }),
    });
    if result.is_err() {unreachable!()}

    let app = axum::Router::new()
        .route("/tts", axum::routing::get(get_tts))
        .route("/voices", axum::routing::get(get_voices))
        .route("/modes", axum::routing::get(|| async {
            axum::Json([
                #[cfg(feature="gtts")] TTSMode::gTTS.to_string(),
                #[cfg(feature="polly")] TTSMode::Polly.to_string(),
                #[cfg(feature="espeak")] TTSMode::eSpeak.to_string(),
                #[cfg(feature="gcloud")] TTSMode::gCloud.to_string(),
            ])
        }));

    let bind_to = std::env::var("BIND_ADDR").ok().map_or_else(
        || Cow::Borrowed("0.0.0.0:3000"),
        Cow::Owned
    ).parse()?;

    tracing::info!("Binding to {bind_to} {} redis enabled!", if redis_uri.is_some() {"with"} else {"without"});
    axum::Server::bind(&bind_to)
        .serve(app.into_make_service())
        .with_graceful_shutdown(async {drop(tokio::signal::ctrl_c().await)})
        .await?;

    Ok(())
}


#[derive(Debug)]
enum Error {
    Unauthorized,
    UnknownVoice(String),
    #[cfg(any(feature="gtts", feature="espeak"))] AudioTooLong,
    #[cfg(any(feature="gcloud", feature="espeak"))] InvalidSpeakingRate(f32),

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
            #[cfg(any(feature="gcloud", feature="espeak"))] Self::InvalidSpeakingRate(rate) => write!(f, "Invalid speaking rate: {rate}"),
            #[cfg(any(feature="gtts", feature="espeak"))] Self::AudioTooLong => f.write_str("Max length exceeded!"),
            Self::UnknownVoice(voice) => write!(f, "Unknown voice: {voice}"),
            Self::Unauthorized => write!(f, "Unauthorized request"),
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
                Self::Unauthorized => 4,
                #[cfg(any(feature="gcloud", feature="espeak"))] Self::InvalidSpeakingRate(_) => 3_u8,
                #[cfg(any(feature="gtts", feature="espeak"))] Self::AudioTooLong => 2,
                Self::UnknownVoice(_) => 1,
                Self::Unknown(_) => 0,
            },
        });

        let status = match self {
            #[cfg(any(feature="gcloud", feature="espeak"))] Self::InvalidSpeakingRate(_) => axum::http::StatusCode::BAD_REQUEST,
            #[cfg(any(feature="gtts", feature="espeak"))] Self::AudioTooLong => axum::http::StatusCode::BAD_REQUEST,
            Self::Unknown(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Self::UnknownVoice(_) => axum::http::StatusCode::BAD_REQUEST,
            Self::Unauthorized => axum::http::StatusCode::FORBIDDEN,
        };

        (status, axum::Json(json_err)).into_response()
    }
}
