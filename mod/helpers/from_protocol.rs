// helpers/from_protocol.rs — OVERWRITE body of ServerVersion::from_protocol.
// Protocols the legacy lane registry covers resolve to a marker variant so the
// server list marks them joinable (multiplayer.rs gates on is_some()); the
// displayed name comes from the ping json itself. Upstream numbering passes
// through to the original table.
fn from_protocol(protocol: i32) -> Option<ServerVersion> {
    if let Some(marker) = crate::adapter::legacy_join::marker(protocol) {
        return Some(marker);
    }
    ServerVersion::ALL.into_iter().find(|v| v.protocol() == protocol)
}
