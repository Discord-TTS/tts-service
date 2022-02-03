use std::{net::IpAddr, str::FromStr};

use rand::Rng;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn get_random_ipv6() -> IpAddr {
    let ip_block = std::env::var("IPV6_BLOCK")
        .expect("IPV6_BLOCK not set!").parse()
        .expect("Invalid IPV6 Block!");

    let name: String = rand::thread_rng()
        .sample_iter::<char, rand::distributions::Standard>(rand::distributions::Standard)
        .take(16)
        .collect();

    tracing::debug!("Generated random name: {name}");
    let ip = ipgen::ip(&name, ip_block).unwrap();
    tracing::debug!("Generated random IP: {ip}");
    ip
}

#[derive(serde::Deserialize)]
struct GetTTS {
    text: String,
    lang: String,
}

async fn get_tts(
    axum::extract::Query(payload): axum::extract::Query<GetTTS>
) -> Result<impl axum::response::IntoResponse, Error> {
    let GetTTS{text, lang} = payload;

    tracing::debug!("Recieved request to TTS: {text} {lang}");
    let mut url = reqwest::Url::parse("https://translate.google.com/translate_tts?ie=UTF-8&total=1&idx=0&client=tw-ob").unwrap();
    url.query_pairs_mut()
        .append_pair("tl", &lang)
        .append_pair("q", &text)
        .append_pair("textlen", &text.len().to_string())
        .finish();

    let client = reqwest::Client::builder()
        .local_address(Some(get_random_ipv6()))
        .timeout(std::time::Duration::from_secs(2))
        .http2_prior_knowledge()
        .build()?;

    for _ in 0..5 {
        match client.get(url.clone()).send().await {
            Ok(mut resp) => {
                resp = resp.error_for_status()?;

                tracing::debug!("Generated TTS from {text}");
                return Ok((resp.status(), resp.bytes().await?))
            },
            Err(err) => {
                if !err.is_timeout() {
                    return Err(Error::from(err))
                }
            }
        };
    };

    Ok((reqwest::StatusCode::GATEWAY_TIMEOUT, axum::body::Bytes::new()))
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let fmt_layer = tracing_subscriber::fmt::layer();
    let filter = tracing_subscriber::filter::LevelFilter::from_str(
        &std::env::var("LOG_LEVEL")
        .unwrap_or_else(|_| String::from("INFO"))
    )?;

    tracing_subscriber::registry().with(fmt_layer).with(filter).init();

    let app = axum::Router::new()
        .route("/tts", axum::routing::get(get_tts));

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
    Unknown(Box<dyn std::error::Error>)
}

impl<E> From<E> for Error
where E: Into<Box<dyn std::error::Error + Send + Sync>> {
    fn from(e: E) -> Self {
        Self::Unknown(e.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(err) => write!(f, "{:?}", err)
        }
    }
}

impl axum::response::IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        let (status_code, body) = match self {
            Self::Unknown(err) => (500, format!("{:?}", err))
        };

        axum::response::Response::builder()
            .status(status_code)
            .body(axum::body::boxed(axum::body::Full::from(body)))
            .unwrap()
    }
}
