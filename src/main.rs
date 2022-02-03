use std::net::IpAddr;

use rand::Rng;

fn get_random_ipv6() -> IpAddr {
    let ip_block = std::env::var("IPV6_BLOCK")
        .expect("IPV6_BLOCK not set!").parse()
        .expect("Invalid IPV6 Block!");

    let name: String = rand::thread_rng()
        .sample_iter::<char, rand::distributions::Standard>(rand::distributions::Standard)
        .take(16)
        .collect();

    ipgen::ip(&name, ip_block).unwrap()
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

    let mut url = reqwest::Url::parse("https://translate.google.com/translate_tts?ie=UTF-8&total=1&idx=0&client=tw-ob").unwrap();
    url.query_pairs_mut()
        .append_pair("tl", &lang)
        .append_pair("q", &text)
        .append_pair("textlen", &text.len().to_string())
        .finish();

    let response = reqwest::Client::builder()
        .local_address(Some(get_random_ipv6()))
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;    

    Ok((response.status(), response.bytes().await?))
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let app = axum::Router::new()
        .route("/tts", axum::routing::get(get_tts));

    // run it with hyper on localhost:3000
    axum::Server::bind(&"0.0.0.0:3000".parse().unwrap())
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
        todo!()
    }
}

impl axum::response::IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        dbg!(self);
        todo!()
    }
}
