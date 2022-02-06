use std::{str::FromStr, sync::Arc, net::IpAddr};

use rand::Rng;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};


fn parse_url(text: &str, lang: &str) -> reqwest::Url {
    let mut url = reqwest::Url::parse("https://translate.google.com/translate_tts?ie=UTF-8&total=1&idx=0&client=tw-ob").unwrap();
    url.query_pairs_mut()
        .append_pair("tl", lang)
        .append_pair("q", text)
        .append_pair("textlen", &text.len().to_string())
        .finish();
    url
}

async fn get_random_ipv6() -> Result<(IpAddr, reqwest::Client), Error> {
    let ip_block = std::env::var("IPV6_BLOCK")
        .expect("IPV6_BLOCK not set!").parse()
        .expect("Invalid IPV6 Block!");

    loop {
        let name: String = rand::thread_rng()
            .sample_iter::<char, rand::distributions::Standard>(rand::distributions::Standard)
            .take(16)
            .collect();
    
        tracing::debug!("Generated random name: {:?}", name.as_bytes());
        let ip = ipgen::ip(&name, ip_block).unwrap();

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_millis(500))
            .local_address(Some(ip))
            .build()?;

        match client.get(parse_url("Hello", "en")).send().await {
            Ok(_) => {
                tracing::warn!("Generated random IP: {}", ip);
                break Ok((ip, client))
            },
            Err(err) if err.is_timeout() => {
                tracing::warn!("Generated IP {} timed out!", ip);
                continue
            },
            Err(err) => break Err(Error::Reqwest(err))
        }
    }
}


#[derive(serde::Deserialize)]
struct GetTTS {
    text: String,
    lang: String,
}

async fn get_tts(
    state: State,
    axum::extract::Query(payload): axum::extract::Query<GetTTS>
) -> Result<impl axum::response::IntoResponse, Error> {
    let GetTTS{text, lang} = payload;

    tracing::debug!("Recieved request to TTS: {text} {lang}");

    let resp = loop {
        let (ip, resp) = {
            let State_{ip, http} = state.read().await.clone();
            (ip, http.get(parse_url(&text, &lang)).send().await?)
        };

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            // Generate a new client, with an new IP, and try again
            tracing::warn!("IP {} has been blocked!", ip);

            let (new_ip, new_http) = get_random_ipv6().await?;
            let mut state = state.write().await;
            state.http = new_http;
            state.ip = new_ip;
        } else {
            break resp
        }
    };

    tracing::debug!("Generated TTS from {text}");
    return Ok((resp.status(), resp.bytes().await?));
}


#[derive(Clone)]
struct State_ {
    ip: IpAddr,
    http: reqwest::Client
}

type State = Arc<tokio::sync::RwLock<State_>>;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let fmt_layer = tracing_subscriber::fmt::layer();
    let filter = tracing_subscriber::filter::LevelFilter::from_str(
        &std::env::var("LOG_LEVEL")
        .unwrap_or_else(|_| String::from("INFO"))
    )?;

    tracing_subscriber::registry().with(fmt_layer).with(filter).init();

    let (ip, http) = get_random_ipv6().await?;
    let state: State = Arc::new(tokio::sync::RwLock::new(State_ {ip, http}));

    let app = axum::Router::new()
        .route("/tts", axum::routing::get({
            let shared_state = Arc::clone(&state);
            move |q| get_tts(Arc::clone(&shared_state), q)
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
    Reqwest(reqwest::Error),
    Unknown(Box<dyn std::error::Error>)
}

impl<E> From<E> for Error
where E: Into<Box<dyn std::error::Error>> {
    fn from(e: E) -> Self {
        let mut err: Box<dyn std::error::Error> = e.into();
        err = match err.downcast::<reqwest::Error>() {
            Ok(err) => return Self::Reqwest(*err),
            Err(err) => err,
        };

        Self::Unknown(err)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reqwest(err) => write!(f, "Reqwest Error: {:?}", err),
            Self::Unknown(err) => write!(f, "{:?}", err)
        }
    }
}

impl axum::response::IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        axum::response::Response::builder()
            .status(500)
            .body(axum::body::boxed(axum::body::Full::from(format!("{:?}", self))))
            .unwrap()
    }
}
