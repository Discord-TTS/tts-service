use aformat::ToArrayString;
use small_fixed_array::FixedString;
use songbird::id::GuildId;
use tokio_tungstenite::tungstenite::{Error as WSError, error::ProtocolError as WSProtocolError};

use crate::{GetTTS, dispatch::models::WSConnectionInfo};
mod models;
mod source;

async fn recieve_message<M: serde::de::DeserializeOwned>(
    ws: &mut axum::extract::ws::WebSocket,
    guild_id: Option<GuildId>,
) -> Option<M> {
    let guild_id_arraystr;
    let guild_id = match guild_id {
        None => "<unknown>",
        Some(guild_id) => {
            guild_id_arraystr = guild_id.get().to_arraystring();
            &*guild_id_arraystr
        }
    };

    let raw_msg_res = ws.recv().await?;
    let raw_msg = match raw_msg_res {
        Ok(msg) => msg,
        Err(err) => {
            let err = err.into_inner();
            if let Some(WSError::Protocol(WSProtocolError::ResetWithoutClosingHandshake)) =
                err.downcast_ref::<WSError>()
            {
                return None;
            }

            tracing::error!("WS error from {}: {err}", guild_id);
            return None;
        }
    };

    match serde_json::from_slice(&raw_msg.into_data()) {
        Ok(msg) => msg,
        Err(err) => {
            tracing::error!("WS deserialization error from {guild_id}: {err}");
            None
        }
    }
}

pub async fn ws_task(mut ws: axum::extract::ws::WebSocket) {
    let Some(connect_info) = recieve_message::<WSConnectionInfo>(&mut ws, None).await else {
        return;
    };

    let guild_id = connect_info.guild_id;

    let mut call = songbird::Call::standalone(guild_id, connect_info.bot_id);
    if call.connect(connect_info.into()).await.is_err() {
        tracing::error!("songbird Driver hung up during connect");
        return;
    }

    handle_messages(ws, call, guild_id).await;
}

#[derive(serde::Deserialize)]
enum IncomingMessage {
    QueueTTS(GetTTS),
    MoveVC(models::WSConnectionInfo),
    ClearQueue,
}

async fn handle_messages(
    mut ws: axum::extract::ws::WebSocket,
    mut call: songbird::Call,
    guild_id: GuildId,
) {
    while let Some(msg) = recieve_message::<IncomingMessage>(&mut ws, Some(guild_id)).await {
        match msg {
            IncomingMessage::QueueTTS(mut get_tts) => {
                get_tts.preferred_format = Some(FixedString::from_static_trunc("opus"));

                let compose = Box::new(source::TTSSource(Some(get_tts)));
                call.enqueue_input(songbird::input::Input::Lazy(compose))
                    .await;
            }
            IncomingMessage::MoveVC(connection_info) => {
                call.connect(connection_info.into());
            }
            IncomingMessage::ClearQueue => {
                call.stop();
            }
        }
    }
}
