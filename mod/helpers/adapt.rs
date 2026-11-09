// helpers/adapt.rs — OVERWRITE body of Connection::adapt.
// A legacy protocol recorded on this thread attaches the mod's bridge to both
// halves and clears the thread-local; upstream versions run upstream's own
// session exactly as before.
fn adapt(conn: &mut Connection, version: crate::adapter::ServerVersion) {
    if let Some(protocol) = crate::adapter::legacy_join::take_thread_protocol() {
        let bridge = Arc::new(std::sync::Mutex::new(
            crate::adapter::legacy_join::Bridge::new(protocol),
        ));
        log::info!(
            "Legacy join: speaking protocol {} through the horizon-legacy-join bridge",
            protocol
        );
        conn.reader
            .__horizon_mixins_horizon_legacy_join_reader_bridge_mut()
            .replace(bridge.clone());
        conn.writer
            .__horizon_mixins_horizon_legacy_join_writer_bridge_mut()
            .replace(bridge);
        return;
    }
    let session = (version != crate::adapter::ServerVersion::Current)
        .then(|| Arc::new(crate::adapter::Session::new(version)));
    conn.reader.adapter = session.clone();
    conn.writer.adapter = session;
}
