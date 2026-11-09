// helpers/send.rs — cancellable HEAD hook of `ConnWriter::send`.
// A legacy connection drives itself through its stamped adapter session (see
// engine/legacy_bridge.rs): serverbound translation first (the session may
// also queue follow-ups to send right behind), then the same framing and
// threshold steps the original body applies. Without a stamped session the
// original body runs (upstream's own adapter sessions, older clients, plain).
fn send(writer: &mut ConnWriter, body: &[u8]) -> ::core::ops::ControlFlow<Result<()>> {
    let stamped = match writer.adapter.as_ref() {
        // The outlet is a stamped lane: the whole flow is this mod's.
        Some(session) if crate::adapter::legacy_join::join_lane(session) != 0 => {
            writer.adapter.clone()
        }
        Some(_) | None => return ::core::ops::ControlFlow::Continue(()),
    };
    let Some(stamped) = stamped else {
        return ::core::ops::ControlFlow::Continue(());
    };
    let lane = crate::adapter::legacy_join::join_lane(&stamped);
    let outcome = move |writer: &mut ConnWriter| -> Result<()> {
        let session = &*stamped;
        if let Some(answer) = crate::adapter::legacy_join::serverbound(lane, session, body)? {
            writer.send_contents(&frame::contents(&answer, writer.threshold)?)?;
        }
        for answer in crate::adapter::legacy_join::take_outgoing_bodies(session) {
            writer.send_contents(&frame::contents(&answer, writer.threshold)?)?;
        }
        Ok(())
    };
    ::core::ops::ControlFlow::Break(outcome(writer))
}
