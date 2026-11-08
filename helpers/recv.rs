// helpers/recv.rs — OVERWRITE body of ConnReader::recv.
// A legacy connection's bridge keeps its own queue: its translations can make
// several packets of one arriving one (registry data, fabricated configuration
// traffic). Drain the queue first; then read frames as the original body does
// and translate each packet through the bridge.
fn recv(reader: &mut ConnReader) -> Result<(i32, Vec<u8>)> {
    if let Some(bridge) = reader
        .__horizon_mixins_horizon_legacy_join_reader_bridge_mut()
        .clone()
    {
        if let Some(packet) = bridge.lock().unwrap().take_incoming() {
            return Ok(packet);
        }
        loop {
            match reader.next()? {
                Next::Packet((id, payload)) => {
                    if let Some(packet) =
                        bridge.lock().unwrap().clientbound(id, payload)?
                    {
                        return Ok(packet);
                    }
                    if let Some(packet) = bridge.lock().unwrap().take_incoming() {
                        return Ok(packet);
                    }
                }
                Next::Empty => {}
                Next::Partial(wanted) => reader.fill(wanted)?,
            }
        }
    }
    if let Some(packet) = reader.adapter.as_ref().and_then(|a| a.next_incoming()) {
        return Ok(packet);
    }
    loop {
        match reader.next()? {
            Next::Packet((id, payload)) if reader.older_client.is_some() => {
                let client = reader.older_client.as_ref().expect("checked");
                if let Some(packet) = client.serverbound(id, payload)? {
                    return Ok(packet);
                }
            }
            Next::Packet(packet) => match &reader.adapter {
                None => return Ok(packet),
                Some(a) => {
                    if let Some(packet) = a.clientbound(packet.0, packet.1)? {
                        return Ok(packet);
                    }
                    if let Some(packet) = a.next_incoming() {
                        return Ok(packet);
                    }
                }
            },
            Next::Empty => {}
            Next::Partial(wanted) => reader.fill(wanted)?,
        }
    }
}
