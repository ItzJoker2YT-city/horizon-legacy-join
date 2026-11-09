// helpers/recv.rs — cancellable HEAD hook of `ConnReader::recv`.
// A legacy connection drives itself through its stamped adapter session (see
// engine/legacy_bridge.rs): its fabricated packets first, then read frames
// and translate each packet through the engine (a translation may queue
// fabricated packets behind it). Without a stamped session the original body
// runs (upstream's own adapter sessions, older clients, plain).
fn recv(reader: &mut ConnReader) -> ::core::ops::ControlFlow<Result<(i32, Vec<u8>)>> {
    let stamped = match reader.adapter.as_ref() {
        // The inlet is a stamped lane: the whole flow is this mod's.
        Some(session) if crate::adapter::legacy_join::join_lane(session) != 0 => {
            reader.adapter.clone()
        }
        Some(_) | None => return ::core::ops::ControlFlow::Continue(()),
    };
    let Some(stamped) = stamped else {
        return ::core::ops::ControlFlow::Continue(());
    };
    let lane = crate::adapter::legacy_join::join_lane(&stamped);
    let outcome = move |reader: &mut ConnReader| -> Result<(i32, Vec<u8>)> {
        let session = &*stamped;
        if let Some(packet) = crate::adapter::legacy_join::take_incoming(session) {
            return Ok(packet);
        }
        loop {
            match reader.next()? {
                Next::Packet((id, payload)) => {
                    if let Some(packet) =
                        crate::adapter::legacy_join::clientbound(lane, session, id, payload)?
                    {
                        return Ok(packet);
                    }
                    if let Some(packet) = crate::adapter::legacy_join::take_incoming(session) {
                        return Ok(packet);
                    }
                }
                Next::Empty => {}
                Next::Partial(wanted) => reader.fill(wanted)?,
            }
        }
    };
    ::core::ops::ControlFlow::Break(outcome(reader))
}
