// helpers/send.rs — OVERWRITE body of ConnWriter::send.
// A legacy connection's bridge: serverbound translate first (it may also queue
// extra packets to send right after), then framing with this writer's threshold.
fn send(writer: &mut ConnWriter, body: &[u8]) -> Result<()> {
    if writer.older_client.is_some() {
        return match writer.for_client(body)? {
            Some(body) => writer.send_contents(&frame::contents(&body, writer.threshold)?),
            None => Ok(()),
        };
    }
    if let Some(bridge) = writer
        .__horizon_mixins_horizon_legacy_join_writer_bridge_mut()
        .clone()
    {
        let translated = bridge.lock().unwrap().serverbound(body)?;
        if let Some(body) = translated {
            writer.send_contents(&frame::contents(&body, writer.threshold)?)?;
        }
        for extra in bridge.lock().unwrap().take_outgoing() {
            writer.send_contents(&frame::contents(&extra, writer.threshold)?)?;
        }
        return Ok(());
    }
    if let Some(a) = writer.adapter.clone() {
        if let Some(body) = a.serverbound(body)? {
            writer.send_contents(&frame::contents(&body, writer.threshold)?)?;
        }
        for body in a.take_outgoing() {
            writer.send_contents(&frame::contents(&body, writer.threshold)?)?;
        }
        return Ok(());
    }
    writer.send_contents(&frame::contents(body, writer.threshold)?)
}
