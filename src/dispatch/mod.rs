use std::collections::HashMap;

use songbird::id::GuildId;
use tokio_tungstenite::tungstenite::{Error as WSError, error::ProtocolError as WSProtocolError};

use crate::dispatch::models::{IncomingMessage, MessageFrame, WSConnectionInfo};
mod models;
mod source;

enum MessageResponse {
    Ok(MessageFrame),
    Disconnect,
    Ignore,
}

async fn recieve_message(ws: &mut axum::extract::ws::WebSocket) -> MessageResponse {
    let raw_msg = match ws.recv().await {
        Some(Ok(msg)) => msg,
        Some(Err(err)) => {
            let err = err.into_inner();
            if let Some(WSError::Protocol(WSProtocolError::ResetWithoutClosingHandshake)) =
                err.downcast_ref::<WSError>()
            {
                return MessageResponse::Disconnect;
            }

            tracing::error!("WS error: {err}");
            return MessageResponse::Disconnect;
        }
        None => return MessageResponse::Disconnect,
    };

    let raw_msg_bytes = match raw_msg {
        axum::extract::ws::Message::Text(utf8_bytes) => utf8_bytes.into(),
        axum::extract::ws::Message::Binary(bytes) => bytes,
        _ => return MessageResponse::Ignore,
    };

    match serde_json::from_slice(&raw_msg_bytes) {
        Ok(msg) => MessageResponse::Ok(msg),
        Err(err) => {
            tracing::error!("WS deserialization error: {err}");
            MessageResponse::Disconnect
        }
    }
}

pub type CallMap = HashMap<GuildId, songbird::Call>;

pub async fn ws_task(mut ws: axum::extract::ws::WebSocket) {
    let state = crate::STATE.get().expect("should be set before requests");
    let calls = state.calls.lock().unwrap().take();
    let Some(mut calls) = calls else {
        tracing::warn!("Attempted to start two streams at once");
        return;
    };

    loop {
        let msg = match recieve_message(&mut ws).await {
            MessageResponse::Ok(msg) => msg,
            MessageResponse::Disconnect => break,
            MessageResponse::Ignore => continue,
        };

        match msg.inner {
            IncomingMessage::QueueTTS(get_tts) => {
                let Some(call) = calls.get_mut(&msg.guild_id) else {
                    continue;
                };

                let compose = Box::new(source::TTSSource(Some(get_tts)));
                call.enqueue_input(songbird::input::Input::Lazy(compose))
                    .await;
            }
            IncomingMessage::MoveVC(info) => {
                let call = calls
                    .entry(msg.guild_id)
                    .or_insert_with(|| songbird::Call::standalone(msg.guild_id, info.bot_id));

                call.connect(ws_connect_info_to_songbird(info, msg.guild_id));
            }
            IncomingMessage::ClearQueue => {
                let Some(call) = calls.get_mut(&msg.guild_id) else {
                    continue;
                };

                call.stop();
            }
            IncomingMessage::Leave => {
                calls.remove(&msg.guild_id);
            }
        }
    }

    *state.calls.lock().unwrap() = Some(calls);
}

fn ws_connect_info_to_songbird(
    info: WSConnectionInfo,
    guild_id: GuildId,
) -> songbird::ConnectionInfo {
    songbird::ConnectionInfo {
        guild_id,
        channel_id: info.channel_id,
        endpoint: info.endpoint,
        session_id: info.session_id,
        token: info.token,
        user_id: info.bot_id,
    }
}
