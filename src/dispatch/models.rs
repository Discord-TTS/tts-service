use songbird::id::{ChannelId, GuildId, UserId};

macro_rules! make_deserializers {
    ($(fn $fn_name:ident($id:ty);)*) => {$(
        fn $fn_name<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<$id, D::Error> {
            Ok(<$id>::new(serde::de::Deserialize::deserialize(deserializer)?))
        }
    )*};
}

make_deserializers! {
    fn deserialize_channel_id(ChannelId);
    fn deserialize_guild_id(GuildId);
    fn deserialize_user_id(UserId);
}

#[derive(serde::Deserialize)]
pub struct MessageFrame {
    #[serde(deserialize_with = "deserialize_guild_id")]
    pub guild_id: GuildId,
    pub inner: IncomingMessage,
}

#[derive(serde::Deserialize)]
pub enum IncomingMessage {
    QueueTTS(crate::GetTTS),
    MoveVC(WSConnectionInfo),
    ClearQueue,
    Leave,
}

#[derive(serde::Deserialize)]
pub struct WSConnectionInfo {
    #[serde(deserialize_with = "deserialize_channel_id")]
    pub channel_id: ChannelId,
    pub endpoint: String,
    pub session_id: String,
    pub token: String,
    #[serde(deserialize_with = "deserialize_user_id")]
    pub bot_id: UserId,
}
