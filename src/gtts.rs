use itertools::Itertools;
use once_cell::sync::Lazy;
use rand::Rng;
use tokio::sync::RwLock;

use crate::Result;


#[derive(Clone)]
pub struct State {
    ip: std::net::IpAddr,
    pub http: reqwest::Client
}

static BASE_URL: Lazy<reqwest::Url> = Lazy::new(|| {
    reqwest::Url::parse("https://translate.google.com/translate_tts?ie=UTF-8&total=1&idx=0&client=tw-ob").unwrap()
});

fn parse_url(text: &str, lang: &str) -> reqwest::Url {
    let mut url = BASE_URL.clone();
    url.query_pairs_mut()
        .append_pair("tl", lang)
        .append_pair("q", text)
        .append_pair("textlen", &text.len().to_string())
        .finish();
    url
}

pub async fn get_random_ipv6() -> Result<State> {
    let ip_block = match std::env::var("IPV6_BLOCK") {
        Ok(ip_block) if &ip_block == "DISABLE" => return Ok(State {ip: "0.0.0.0".parse()?, http: reqwest::Client::new()}),
        Ok(ip_block) => ip_block.parse().expect("Invalid IPV6 Block!"),
        _ => panic!("IPV6_BLOCK not set! Set to \"DISABLE\" to disable rate limit bypass"),
    };

    let mut attempts = 1;
    loop {
        let name: String = rand::thread_rng()
            .sample_iter::<char, _>(rand::distributions::Standard)
            .take(16)
            .collect();
    
        tracing::debug!("Generated random name: {:?}", name.as_bytes());
        let ip = ipgen::ip(&name, ip_block).unwrap();

        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .local_address(Some(ip))
            .build()?;

        let check_request = http.get(parse_url("Hello", "en")).send().await;
        let fail_reason = match is_block(check_request).await? {
            CheckResult::Ok(..) => {
                tracing::warn!("Generated random IP: {ip}");
                break Ok(State{ip, http})
            },
            CheckResult::NormalBlock => "429 block",
            CheckResult::TimeoutBlock => "timeout block",
            CheckResult::HostUnreachable => "unreachable error",
        };

        tracing::warn!("Failed to generate a new IP on attempt {attempts} with a {fail_reason}");
        attempts += 1;
    }
}

enum CheckResult {
    Ok(Option<reqwest::header::HeaderValue>, bytes::Bytes),
    NormalBlock,
    TimeoutBlock,
    HostUnreachable,
}

fn is_host_unreachable(err: &reqwest::Error) -> bool {
    let debug_message = format!("{err:?}");
    ["No route to host", "HostUnreachable"].into_iter().all(|s| debug_message.contains(s))
}

async fn is_block(resp: reqwest::Result<reqwest::Response>) -> Result<CheckResult> {
    match resp {
        Ok(resp) => {
            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                Ok(CheckResult::NormalBlock)
            } else {
                Ok(CheckResult::Ok(
                    resp.headers().get(reqwest::header::CONTENT_TYPE).cloned(),
                    resp.bytes().await?
                ))
            }
        },
        Err(err) => {
            if err.is_timeout() {
                Ok(CheckResult::TimeoutBlock)
            } else if is_host_unreachable(&err) {
                Ok(CheckResult::HostUnreachable)
            } else {
                Err(err.into())
            }
        },
    }
}

pub async fn get_tts(state: &RwLock<State>, text: &str, voice: &str) -> Result<(bytes::Bytes, Option<reqwest::header::HeaderValue>)> {
    let mut content_type = None;
    let mut audio = Vec::new();

    let chunks: Vec<String> = text.chars().chunks(200).into_iter().map(Iterator::collect).collect();
    for chunk in chunks {
        loop {
            let (ip, result) = {
                let State{ip, http} = state.read().await.clone();
                (ip, http.get(parse_url(&chunk, voice)).send().await)
            };

            if let CheckResult::Ok(content_type_, audio_chunk) = is_block(result).await? {
                if let Some(content_type_) = content_type_ {
                    content_type = Some(content_type_);
                }

                break audio.extend(audio_chunk)
            }

            // Generate a new client, with an new IP, and try again
            let mut state = state.write().await;
            if state.ip == ip {
                tracing::warn!("IP {ip} has been blocked!");
                *state = get_random_ipv6().await?;
            }
        }
    }

    Ok((bytes::Bytes::from(audio), content_type))
}

pub fn check_voice(voice: &str) -> bool {
    get_voices().iter().any(|s| s.as_str() == voice)
}


pub fn get_voices() -> Vec<String> {
    get_raw_voices().into_keys().collect()
}

pub fn get_raw_voices() -> std::collections::BTreeMap<String, String> {
    serde_json::from_str(include_str!("data/voices-gtts.json")).unwrap()
}
