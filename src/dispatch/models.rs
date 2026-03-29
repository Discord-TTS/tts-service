use songbird::{
    ConnectionInfo,
    id::{ChannelId, GuildId, UserId},
};

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
pub struct WSConnectionInfo {
    #[serde(deserialize_with = "deserialize_channel_id")]
    pub channel_id: ChannelId,
    pub endpoint: String,
    #[serde(deserialize_with = "deserialize_guild_id")]
    pub guild_id: GuildId,
    pub session_id: String,
    pub token: String,
    #[serde(deserialize_with = "deserialize_user_id")]
    pub bot_id: UserId,
}

impl From<WSConnectionInfo> for ConnectionInfo {
    fn from(info: WSConnectionInfo) -> Self {
        Self {
            channel_id: info.channel_id,
            endpoint: info.endpoint,
            guild_id: info.guild_id,
            session_id: info.session_id,
            token: info.token,
            user_id: info.bot_id,
        }
    }
}
