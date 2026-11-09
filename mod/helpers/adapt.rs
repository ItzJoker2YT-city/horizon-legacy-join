// helpers/adapt.rs — cancellable HEAD hook of `Connection::adapt`.
// A protocol recorded by the older_server hook attaches a stamped legacy
// session into the connection's own adapter fields (both halves) — the
// upstream session the engine drives, see engine/legacy_bridge.rs - and
// returns from the function. Otherwise the original body runs (upstream's
// own adapter sessions for 1.20.2..26.2 servers, nothing for the pinned).
fn adapt(
    conn: &mut Connection,
    version: crate::adapter::ServerVersion,
) -> ::core::ops::ControlFlow<()> {
    let _ = &version;
    if let Some(lane) = crate::adapter::legacy_join::take_thread_protocol() {
        if let Ok(stamped) = crate::adapter::legacy_join::stamped_session(lane) {
            conn.reader.adapter = Some(stamped.clone());
            conn.writer.adapter = Some(stamped);
            return ::core::ops::ControlFlow::Break(());
        }
    }
    ::core::ops::ControlFlow::Continue(())
}
