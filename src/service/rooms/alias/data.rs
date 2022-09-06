use ruma::{RoomId, RoomAliasId};

pub trait Data {
    /// Creates or updates the alias to the given room id.
    fn set_alias(
        alias: &RoomAliasId,
        room_id: &RoomId
    ) -> Result<()>;

    /// Forgets about an alias. Returns an error if the alias did not exist.
    fn remove_alias(
        alias: &RoomAliasId,
    ) -> Result<()>;

    /// Looks up the roomid for the given alias.
    fn resolve_local_alias(
        alias: &RoomAliasId,
    ) -> Result<()>;

    /// Returns all local aliases that point to the given room
    fn local_aliases_for_room(
        alias: &RoomAliasId,
    ) -> Result<()>;
}
