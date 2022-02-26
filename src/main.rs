#![warn(clippy::pedantic)]
#![allow(clippy::unused_async)]

#[cfg(not(any(feature="gtts", feature="espeak", feature="premium")))] 
compile_error!("Either feature `gtts`, `espeak`, or `premium` must be enabled!");

use std::{str::FromStr, sync::Arc, borrow::Cow};

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[cfg(feature="gtts")] mod gtts;
#[cfg(feature="espeak")] mod espeak;
#[cfg(feature="premium")] mod premium;


#[derive(serde::Deserialize)]
struct GetVoices {
    mode: TTSMode,
}

async fn get_voices(
    axum::extract::Query(payload): axum::extract::Query<GetVoices>
) -> Result<impl axum::response::IntoResponse, Error> {
    let GetVoices{mode} = payload;

    let voices: Vec<String> = match mode {
        #[cfg(feature="gtts")] TTSMode::gTTS => gtts::get_voices(),
        #[cfg(feature="espeak")] TTSMode::eSpeak => espeak::get_voices(),
        #[cfg(feature="premium")] TTSMode::Premium => premium::get_voices(),
    };

    Ok(axum::Json(voices))
}


#[derive(serde::Deserialize)]
struct GetTTS {
    text: String,
    lang: String,
    mode: TTSMode,
    #[cfg(feature="premium")] #[serde(default)] speaking_rate: f32
}

async fn get_tts(
    #[allow(unused_variables)] state: Arc<State>,
    axum::extract::Query(payload): axum::extract::Query<GetTTS>
) -> Result<impl axum::response::IntoResponse, Error> {
    let text = &payload.text;
    let lang = &payload.lang;

    tracing::debug!("Recieved request to TTS: {text} {lang}");

    let content_type: Cow<str>;
    let data: bytes::Bytes;

    match payload.mode {
        #[cfg(feature="gtts")] TTSMode::gTTS => {
            let resp = gtts::get_tts(&state.gtts, text, lang).await?;

            content_type = Cow::Owned(resp.headers()[reqwest::header::CONTENT_TYPE].to_str()?.to_string());
            data = resp.bytes().await?;
        },
        #[cfg(feature="espeak")] TTSMode::eSpeak => {
            content_type = Cow::Borrowed("audio/wav");
            data = bytes::Bytes::from(espeak::get_tts(text, lang).await?);
        },
        #[cfg(feature="premium")] TTSMode::Premium => {
            content_type = Cow::Borrowed("audio/opus");
            data = bytes::Bytes::from(premium::get_tts(&state.premium, text, lang, payload.speaking_rate).await?);
        }
    };

    tracing::debug!("Generated TTS from {text}");
    Ok(
        axum::response::Response::builder()
            .header("Content-Type", &*content_type)
            .body(axum::body::Full::new(data))?
    )
}


#[derive(serde::Deserialize, Clone, Copy)]
#[allow(non_camel_case_types)]
enum TTSMode {
    #[cfg(feature="gtts")] gTTS,
    #[cfg(feature="espeak")] eSpeak,
    #[cfg(feature="premium")] Premium,
}

struct State {
    #[cfg(feature="gtts")] gtts: tokio::sync::RwLock<gtts::State>,
    #[cfg(feature="premium")] premium: tokio::sync::RwLock<premium::State>
}


#[tokio::main]
async fn main() -> Result<(), Error> {
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

    let state = Arc::new(State {
        #[cfg(feature="gtts")] gtts: gtts::State::new().await?,
        #[cfg(feature="premium")] premium: premium::State::new()?,
    });

    let app = axum::Router::new()
        .route("/tts", axum::routing::get({
            let shared_state = Arc::clone(&state);
            move |q| get_tts(Arc::clone(&shared_state), q)
        }))
        .route("/voices", axum::routing::get(get_voices))
        .route("/modes", axum::routing::get(|| async {
            axum::Json([
                #[cfg(feature="gtts")] "gTTS",
                #[cfg(feature="espeak")] "eSpeak",
                #[cfg(feature="premium")] "Premium",
            ])
        }));

    let bind_to = std::env::var("BIND_ADDR")
        .unwrap_or_else(|_| String::from("0.0.0.0:3000")).parse()?;

    tracing::info!("Binding to {bind_to}");
    axum::Server::bind(&bind_to)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}


#[derive(Debug)]
enum Error {
    #[cfg(feature="gtts")] Reqwest(reqwest::Error),
    Unknown(Box<dyn std::error::Error + Send + Sync>)
}

impl<E> From<E> for Error
where E: Into<Box<dyn std::error::Error + Send + Sync>> {
    fn from(e: E) -> Self {
        #[allow(unused_mut)]
        let mut err: Box<dyn std::error::Error + Send + Sync> = e.into();

        #[cfg(feature="gtts")] {
            err = match err.downcast::<reqwest::Error>() {
                Ok(err) => return Self::Reqwest(*err),
                Err(err) => err,
            };
        }

        Self::Unknown(err)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(feature="gtts")] Self::Reqwest(err) => write!(f, "Reqwest Error: {:?}", err),
            Self::Unknown(err) => write!(f, "{:?}", err)
        }
    }
}

impl axum::response::IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        tracing::error!("{self:?}");
        axum::response::Response::builder()
            .status(500)
            .body(axum::body::boxed(axum::body::Full::from(format!("{:?}", self))))
            .unwrap()
    }
}
