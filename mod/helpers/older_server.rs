// helpers/older_server.rs — cancellable HEAD hook of `net::older_server`.
// The hook does the whole pre-join decision and returns from the function:
// it pings the server, reads its protocol, and — when the legacy lane
// registry covers it — records the number on this thread and breaks with the
// marker variant (run() calls older_server and adapt back to back on one
// thread, so what adapt attaches is decided here). Upstream-supported
// protocols resolve through the original table; anything else breaks with
// None, exactly as the original body would have. The original body (and its
// own ping and env gate) never runs; the gate is replicated here.
fn older_server(address: &str) -> ::core::ops::ControlFlow<Option<horizon_protocol::adapter::ServerVersion>> {
    let Some(value) = std::env::var_os("HORIZON_OLDER_SERVERS").filter(|v| v == "1") else {
        return ::core::ops::ControlFlow::Break(None);
    };
    drop(value);
    let (json, _) = match ping(address, std::time::Duration::from_secs(5)) {
        Ok(pair) => pair,
        Err(_) => return ::core::ops::ControlFlow::Break(None),
    };
    let Some(protocol) = json["version"]["protocol"].as_i64().and_then(|v| i32::try_from(v).ok()) else {
        return ::core::ops::ControlFlow::Break(None);
    };
    // The pinned version needs nothing.
    if protocol == horizon_protocol::PROTOCOL_VERSION {
        return ::core::ops::ControlFlow::Break(None);
    }
    // A covered legacy lane: remember it for adapt, join through the bridge.
    if horizon_protocol::adapter::legacy_join::covered(protocol) {
        horizon_protocol::adapter::legacy_join::set_thread_protocol(protocol);
        // The value only feeds `is_some()` and one log line; adapt reads the
        // thread-local instead of this argument.
        return ::core::ops::ControlFlow::Break(Some(horizon_protocol::adapter::ServerVersion::V1_20_2));
    }
    // Upstream's own releases (764..): through the original table.
    ::core::ops::ControlFlow::Break(
        horizon_protocol::adapter::ServerVersion::from_protocol(protocol)
            .filter(|v| *v != horizon_protocol::adapter::ServerVersion::Current),
    )
}
