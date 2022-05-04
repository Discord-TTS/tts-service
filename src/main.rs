#![warn(clippy::pedantic)]
#![allow(clippy::unused_async, clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_lossless)]

#[cfg(not(any(feature="gtts", feature="espeak", feature="premium")))] 
compile_error!("Either feature `gtts`, `espeak`, or `premium` must be enabled!");

use std::{str::FromStr, fmt::Display, borrow::Cow};

use once_cell::sync::OnceCell;
use sha2::Digest;
use redis::AsyncCommands;
use serde_json::to_value;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[cfg(feature="gtts")] mod gtts;
#[cfg(feature="espeak")] mod espeak;
#[cfg(feature="premium")] mod premium;

type Result<T> = std::result::Result<T, anyhow::Error>;
type ResponseResult<T> = std::result::Result<T, Error>;

#[derive(serde::Deserialize)]
struct GetVoices {
    mode: TTSMode,
    #[serde(default)]
    #[cfg(feature="premium")]
    raw: bool,
}

async fn get_voices(
    axum::extract::Query(payload): axum::extract::Query<GetVoices>
) -> ResponseResult<impl axum::response::IntoResponse> {
    cfg_if::cfg_if!(
        if #[cfg(feature="premium")] {
            let GetVoices{mode, raw} = payload;
        } else {
            let GetVoices{mode} = payload;
        }
    );

    Ok(axum::Json(match mode {
        #[cfg(feature="espeak")] TTSMode::eSpeak => to_value(espeak::get_voices()),
        #[cfg(feature="gtts")] TTSMode::gTTS => if raw {
            to_value(gtts::get_raw_voices())
        } else {
            to_value(gtts::get_voices())
        },
        #[cfg(feature="premium")] TTSMode::Premium => if raw {
            to_value(premium::get_raw_voices())
        } else {
            to_value(premium::get_voices())
        },
    }?))
}

#[derive(serde::Deserialize)]
struct GetTTS {
    text: String,
    mode: TTSMode,
    #[serde(default)] speaking_rate: f32,
    #[serde(rename="lang")] voice: String,
    #[cfg(any(feature="gtts", feature="espeak"))] max_length: Option<u64>,
}

async fn get_tts(
    axum::extract::Query(payload): axum::extract::Query<GetTTS>
) -> ResponseResult<impl axum::response::IntoResponse> {
    cfg_if::cfg_if!(
        if #[cfg(any(feature="gtts", feature="espeak"))] {
            let GetTTS{text, voice, mode, speaking_rate, max_length} = payload;
        } else {
            let GetTTS{text, voice, mode, speaking_rate} = payload;
        }
    );

    #[cfg(any(feature="premium", feature="espeak"))]
    mode.check_speaking_rate(speaking_rate)?;
    mode.check_voice(&voice)?;

    let cache_key = format!("{text} | {voice} | {mode} | {speaking_rate}");
    tracing::debug!("Recieved request to TTS: {cache_key}");

    let state = STATE.get().unwrap();
    let redis_info = if let Some(redis_state) = &state.redis {
        let cache_hash = {
            let mut hasher = sha2::Sha256::new();
            hasher.update(&cache_key);
            hasher.finalize()
        };

        let mut conn = redis_state.client.get().await?;
        let cached_audio = conn.get::<'_, _, Option<String>>(&*cache_hash).await?
            .map(|enc| redis_state.key.decrypt(&enc))
            .transpose()?
            .map(bytes::Bytes::from);

        if let Some(cached_audio) = cached_audio {
            #[cfg(any(feature="gtts", feature="espeak"))]
            mode.check_length(&cached_audio, max_length)?;

            tracing::debug!("Used cached TTS for {cache_key}");
            return Ok(mode.into_response(cached_audio));
        }

        Some((conn, &redis_state.key, cache_hash))
    } else {
        None
    };

    let audio = match mode {
        #[cfg(feature="gtts")] TTSMode::gTTS => gtts::get_tts(&state.gtts, &text, &voice).await?,
        #[cfg(feature="espeak")] TTSMode::eSpeak => espeak::get_tts(&text, &voice, speaking_rate as u16).await?,
        #[cfg(feature="premium")] TTSMode::Premium => premium::get_tts(&state.premium, &text, &voice, speaking_rate).await?,
    };

    tracing::debug!("Generated TTS from {cache_key}");
    if let Some((mut redis_conn, key, cache_hash)) = redis_info {
        if let Err(err) = redis_conn.set::<'_, _, _, ()>(&*cache_hash, key.encrypt(&audio)).await {
            tracing::error!("Failed to cache: {err}");
        } else {
            tracing::debug!("Cached TTS from {cache_key}");
        };
    };

    #[cfg(any(feature="gtts", feature="espeak"))]
    mode.check_length(&audio, max_length)?;
    Ok(mode.into_response(audio))
}


#[derive(serde::Deserialize, Clone, Copy, Debug)]
#[allow(non_camel_case_types)]
enum TTSMode {
    #[cfg(feature="gtts")] gTTS,
    #[cfg(feature="espeak")] eSpeak,
    #[cfg(feature="premium")] Premium,
}

impl TTSMode {
    fn into_response(self, data: bytes::Bytes) -> impl axum::response::IntoResponse {
        axum::response::Response::builder()
            .header("Content-Type", match self {
                #[cfg(feature="gtts")]    Self::gTTS    => "audio/mpeg",
                #[cfg(feature="espeak")]  Self::eSpeak  => "audio/wav",
                #[cfg(feature="premium")] Self::Premium => "audio/opus"
            })
            .body(axum::body::Full::new(data))
            .unwrap()
    }

    fn check_voice(self, voice: &str) -> ResponseResult<()> {
        if match self {
            #[cfg(feature="gtts")] Self::gTTS => gtts::check_voice(voice),
            #[cfg(feature="espeak")] Self::eSpeak => espeak::check_voice(voice),
            #[cfg(feature="premium")] Self::Premium => premium::check_voice(voice),
        } {
            Ok(())
        } else {
            Err(Error::UnknownVoice(voice.to_owned()))
        }
    }

    #[cfg(any(feature="gtts", feature="espeak"))]
    #[allow(unused_variables)]
    fn check_length(self, audio: &[u8], max_length: Option<u64>) -> ResponseResult<()> {
        if max_length.map_or(true, |max_length| match self {
            #[cfg(feature="gtts")]    Self::gTTS    => gtts::check_length(audio, max_length),
            #[cfg(feature="espeak")]  Self::eSpeak  => espeak::check_length(audio, max_length as u32),
            #[cfg(feature="premium")] Self::Premium => true,
        }) {
            Ok(())
        } else {
            Err(Error::AudioTooLong)
        }
    }

    #[cfg(any(feature="premium", feature="espeak"))]
    fn check_speaking_rate(self, speaking_rate: f32) -> ResponseResult<()> {
        if let Some(max) = self.max_speaking_rate() {
            if speaking_rate > max {
                return Err(Error::InvalidSpeakingRate(speaking_rate))
            }
        }

        Ok(())
    }

    #[allow(clippy::unnecessary_wraps)]
    #[cfg(any(feature="premium", feature="espeak"))]
    const fn max_speaking_rate(self) -> Option<f32> {
        match self {
            #[cfg(feature="gtts")]    Self::gTTS    => None,
            #[cfg(feature="espeak")]  Self::eSpeak  => Some(400.0),
            #[cfg(feature="premium")] Self::Premium => Some(4.0),
        }
    }
}

impl Display for TTSMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            #[cfg(feature="gtts")] Self::gTTS => "gTTS",
            #[cfg(feature="espeak")] Self::eSpeak => "eSpeak",
            #[cfg(feature="premium")] Self::Premium => "Premium"
        })
    }
}


struct RedisCache {
    client: deadpool_redis::Pool,
    key: fernet::Fernet
}

struct State {
    redis: Option<RedisCache>,
    #[cfg(feature="gtts")] gtts: tokio::sync::RwLock<gtts::State>,
    #[cfg(feature="premium")] premium: tokio::sync::RwLock<premium::State>
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

    let redis_uri = std::env::var("REDIS_URI").ok();
    let result = STATE.set(State {
        #[cfg(feature="gtts")] gtts: gtts::State::new().await?,
        #[cfg(feature="premium")] premium: premium::State::new()?,
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
                #[cfg(feature="gtts")] "gTTS",
                #[cfg(feature="espeak")] "eSpeak",
                #[cfg(feature="premium")] "Premium",
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
    UnknownVoice(String),
    #[cfg(any(feature="gtts", feature="espeak"))] AudioTooLong,
    #[cfg(any(feature="premium", feature="espeak"))] InvalidSpeakingRate(f32),

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
            #[cfg(any(feature="premium", feature="espeak"))] Self::InvalidSpeakingRate(rate) => write!(f, "Invalid speaking rate: {rate}"),
            #[cfg(any(feature="gtts", feature="espeak"))] Self::AudioTooLong => f.write_str("Max length exceeded!"),
            Self::UnknownVoice(voice) => write!(f, "Unknown voice: {voice}"),
            Self::Unknown(e) => write!(f, "Unknown error: {e}"),
        }
    }
}

impl axum::response::IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        if let Error::Unknown(inner) = &self {
            tracing::error!("{inner:?}");
        };

        let json_err = serde_json::json!({
            "display": self.to_string(),
            "code": match self {
                #[cfg(any(feature="premium", feature="espeak"))] Self::InvalidSpeakingRate(_) => 3_u8,
                #[cfg(any(feature="gtts", feature="espeak"))] Self::AudioTooLong => 2,
                Self::UnknownVoice(_) => 1,
                Self::Unknown(_) => 0,
            },
        });

        let status = match self {
            #[cfg(any(feature="premium", feature="espeak"))] Self::InvalidSpeakingRate(_) => axum::http::StatusCode::BAD_REQUEST,
            #[cfg(any(feature="gtts", feature="espeak"))] Self::AudioTooLong => axum::http::StatusCode::BAD_REQUEST,
            Self::Unknown(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Self::UnknownVoice(_) => axum::http::StatusCode::BAD_REQUEST,
        };

        (status, axum::Json(json_err)).into_response()
    }
}
