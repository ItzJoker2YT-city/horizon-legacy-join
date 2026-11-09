// helpers/older_server.rs — OVERWRITE body of horizon_client::net::older_server.
// Pings the server, reads its protocol number, and decides who translates:
// upstream versions (>= 764) return upstream's own ServerVersion and clear any
// stale thread-local; a protocol the legacy lane registry covers records the
// number on this thread (run() calls older_server and adapt back to back on the
// same thread) and returns a marker variant, which the overwritten
// Connection::adapt reads. Other protocols stay None, as upstream had it.
fn older_server(address: &str) -> Option<horizon_protocol::adapter::ServerVersion> {
    let (json, _) = ping(address, std::time::Duration::from_secs(5)).ok()?;
    let protocol = i32::try_from(json["version"]["protocol"].as_i64()?).ok()?;
    if protocol == horizon_protocol::PROTOCOL_VERSION {
        return None;
    }
    if horizon_protocol::adapter::legacy_join::lane(protocol).is_some() {
        horizon_protocol::adapter::legacy_join::set_thread_protocol(protocol);
        // A marker value: adapt() is overwritten and reads the thread-local, not
        // this value; the "1.20.2" shows only in one log line adapt re-logs.
        return Some(horizon_protocol::adapter::ServerVersion::V1_20_2);
    }
    // Upstream's own protocol numbering (764..): resolve through the overwritten
    // from_protocol, which passes real variants through untouched.
    horizon_protocol::adapter::ServerVersion::from_protocol(protocol)
        .filter(|v| *v != horizon_protocol::adapter::ServerVersion::Current)
}
