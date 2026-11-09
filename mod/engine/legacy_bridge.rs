// The legacy lane engine of horizon-legacy-join: joining servers older than
// upstream's own adapter floor (1.20.2). Injected as `pub mod legacy_join`
// into `adapter.rs`, so it shares the adapter's module tree, and the driver
// itself is just an upstream `Session`: the adapt hook attaches a session
// pinned to [`ServerVersion::V1_20_3`] into the connection's own `adapter`
// fields (both halves), stamped through the injected per-Session state
// `__horizon_mixins_horizon_legacy_join_lane_protocol` (0 = upstream's own
// session, a lane protocol otherwise). The queues the session already owns
// (incoming, outgoing) carry everything fabricated on top.
//
// Alpha: id-level lanes. Every advertised release 1.8.x (47) through 1.20.1
// (763) carries its own id tables, composed by name against the 765 era, and
// packets the pinned era's reader cannot parse drop instead of cutting the
// connection; translating the era payload layouts themselves is the next
// milestone (join a pre-flattening server and expect degradation).
use std::sync::Arc;

use super::{ServerVersion, Session};
use crate::{
    Result, State,
    buf::{Codec, Reader, Writer},
    ids,
    packets::configuration,
};

///// The lane gate ///////////////////////////////////////////////////////////

/// Whether this mod joins `protocol`: the only protocols `older_server`
/// accepts. Upstream's own releases come first: a protocol the adapter
/// already resolves (1.20.2 (764) through 26.3 (777), incl. 1.21.11 (774))
/// never becomes a lane of this mod — its joins are upstream's own session,
/// and this gate keeps the mod from shadowing them however wide its tables
/// grow. Below that floor, a protocol counts when the build's generated
/// tables cover it; the data crate's lane list is the single source of
/// truth, and this gate holds none of its own to fall out of sync with.
pub fn covered(protocol: i32) -> bool {
    if ServerVersion::from_protocol(protocol).is_some() {
        return false;
    }
    horizon_legacy_join_data::covers(protocol)
}

///// Thread-local handoff (older_server and adapt run on the join thread) ////

std::thread_local! {
    static THREAD_PROTOCOL: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
}

/// Record a legacy protocol on the joining thread: `older_server` reports it,
/// the very next `adapt` (the same join) consumes it.
pub fn set_thread_protocol(protocol: i32) {
    THREAD_PROTOCOL.with(|cell| cell.set(Some(protocol)));
}

/// Take the recorded protocol out. Later `adapt`s on the thread run upstream.
pub fn take_thread_protocol() -> Option<i32> {
    THREAD_PROTOCOL.with(|cell| cell.take())
}

///// The stamped session /////////////////////////////////////////////////////

/// The lane protocol a session drives (0 = upstream's own adapter session,
/// which this mod never intercepts).
pub fn join_lane(session: &Session) -> i32 {
    session
        .__horizon_mixins_horizon_legacy_join_lane_protocol()
        .load(::std::sync::atomic::Ordering::Acquire)
}

/// Stamp a session as the driver of `protocol`'s lane.
pub fn set_lane(session: &mut Session, protocol: i32) {
    session
        .__horizon_mixins_horizon_legacy_join_lane_protocol()
        .store(protocol, ::std::sync::atomic::Ordering::Release);
}

/// A stamped legacy session (the adapt hook's shorthand): the inner upstream
/// session of the pinned era, and the lane it carries in injected state.
pub fn stamped_session(lane: i32) -> Result<Arc<Session>> {
    let mut session = Session::new(ServerVersion::V1_20_3);
    set_lane(&mut session, lane);
    Ok(Arc::new(session))
}

/// The next fabricated packet Horizon reads, ahead of anything from the wire.
pub fn take_incoming(session: &Session) -> Option<(i32, Vec<u8>)> {
    session.incoming.lock().unwrap().pop_front()
}

/// The inner session's own follow-up bodies, already renumbered into the
/// lane's own numbering, for Horizon to send right behind what it sends.
pub fn take_outgoing_bodies(session: &Session) -> Vec<Vec<u8>> {
    session.take_outgoing()
}

///// The translation steps (the helpers call these) ///////////////////////////

/// A packet Horizon sends, as its full pinned-numbered body: the lane wire's
/// answer re-identified into the lane's own ids, or `None` when this lane
/// keeps the packet to itself (the fake phases) or has no such packet.
pub fn serverbound(lane: i32, session: &Session, body: &[u8]) -> Result<Option<Vec<u8>>> {
    let entry = session.writing();
    let packet = Reader::new(body).varint().ok().unwrap_or(-1);
    let forwarded = match entry {
        // The handshake passes as it is: the inner session reads its own
        // protocol field and answers with its own protocol number, which is
        // rebuilt with the lane's.
        State::Handshake => session
            .serverbound(body)
            .map(|body| body.map(|body| swap_intention_protocol(lane, body)))?,
        // A server before 1.20.2 answers neither the login acknowledgment nor
        // any configuration packet. The inner session keeps its own state
        // machine through them; the wire stays quiet.
        State::Login if packet == ids::login::serverbound::LOGIN_ACKNOWLEDGED => {
            let _ = session.serverbound(body)?;
            None
        }
        State::Configuration => {
            let _ = session.serverbound(body)?;
            None
        }
        // The real traffic: the inner session renumbers it, then it is rebuilt
        // with the lane's own packet id. A body the pinned reader cannot parse
        // drops instead of cutting the connection.
        _ => match session.serverbound(body) {
            Ok(translated) => translated.and_then(|body| renumber(lane, entry, body)),
            Err(_) => None,
        },
    };
    requeue_outgoing(lane, session, entry);
    Ok(forwarded)
}

/// A packet the lane server sent, in its own ids and its own layouts. One
/// non-passthrough stop is the login's finish: the moment the fake
/// configuration phase is queued (it is read before any further frame, so it
/// lands entirely ahead of the play feed the server starts right after).
pub fn clientbound(
    lane: i32,
    session: &Session,
    old_id: i32,
    payload: Vec<u8>,
) -> Result<Option<(i32, Vec<u8>)>> {
    let reading = session.reading();
    // The tables name no packet under this lane id for the pinned era: drop
    // it instead of handing the inner session a different packet's frame.
    let Some(inner_id) = up_id(lane, reading, old_id) else {
        return Ok(None);
    };
    match reading {
        State::Login => {
            let translated = session.clientbound(inner_id, payload)?;
            requeue_outgoing(lane, session, reading);
            if session.reading() == State::Configuration {
                fabricate_configuration(session);
            }
            Ok(translated)
        }
        // The connection hears nothing on the wire during this phase: its
        // client side was fabricated. Strays (a second finish somehow) drop.
        State::Configuration => Ok(None),
        _ => {
            // Era payload layouts the pinned reader cannot parse (pre-
            // flattening servers, mostly) drop instead of cutting the
            // connection: a degraded feed outlasts a dead one.
            let translated = session.clientbound(inner_id, payload).ok().flatten();
            requeue_outgoing(lane, session, reading);
            Ok(translated)
        }
    }
}

/// The fake configuration phase: every static registry of the era as its own
/// RegistryData (entries named, no worldgen data), no tags, no known packs,
/// then the finish - which flips the inner session to its play state so the
/// real play feed right behind the queue decodes.
fn fabricate_configuration(session: &Session) {
    use crate::ids::configuration::clientbound as cb;
    let mut tables: Vec<(String, Vec<String>)> = super::legacy::registries(ServerVersion::V1_20_2)
        .into_iter()
        .map(|(registry, ids)| (registry, ids.names))
        .collect();
    tables.sort_by(|a, b| a.0.cmp(&b.0));
    for (registry, names) in &tables {
        session.incoming.lock().unwrap().push_back((
            cb::REGISTRY_DATA,
            registry_body(registry, names),
        ));
    }
    let mut w = Writer::new();
    configuration::UpdateTags::write(&configuration::UpdateTags { registries: Vec::new() }, &mut w);
    session.incoming.lock().unwrap().push_back((cb::UPDATE_TAGS, w.data));
    let mut w = Writer::new();
    configuration::SelectKnownPacks::write(
        &configuration::SelectKnownPacks { packs: Vec::new() },
        &mut w,
    );
    session.incoming.lock().unwrap().push_back((cb::SELECT_KNOWN_PACKS, w.data));
    let _ = session.clientbound(cb::FINISH_CONFIGURATION, Vec::new());
}

/// The inner session's own follow-ups (it queues them in its `outgoing`)
/// pushed back, renumbered into the lane's numbering for `state`'s wire;
/// packets the lane never had drop here.
fn requeue_outgoing(lane: i32, session: &Session, state: State) {
    for body in session.take_outgoing() {
        if let Some(renumbered) = renumber(lane, state, body) {
            session.outgoing.lock().unwrap().push(renumbered);
        }
    }
}

/// A renumbered outgoing body: the inner session's own packet id looked up in
/// the lane's send table for `state`, or None when the lane never had that
/// packet. The tables are the only source of truth — sending the inner id raw
/// would mean a different packet to a pre-flattening server.
fn renumber(lane: i32, state: State, body: Vec<u8>) -> Option<Vec<u8>> {
    let mut r = Reader::new(&body);
    let Ok(id) = r.varint() else {
        return None; // a body without its own packet id is broken
    };
    let table = horizon_legacy_join_data::table_down(lane, number_of(state));
    let lane_id = table.get(usize::try_from(id).unwrap_or(999)).copied().flatten()?;
    let mut w = Writer::new();
    w.varint(i32::from(lane_id));
    w.bytes(r.rest());
    Some(w.data)
}

///// auxiliaries /////////////////////////////////////////////////////////////

/// The inner session's handshake answer comes with its own protocol number
/// in place; the lane server wants its own. [id][protocol][rest].
fn swap_intention_protocol(lane: i32, body: Vec<u8>) -> Vec<u8> {
    let mut r = Reader::new(&body);
    if r.varint().is_err() || r.varint().is_err() {
        return body;
    }
    let rest = r.rest();
    let mut w = Writer::new();
    w.varint(ids::handshake::serverbound::INTENTION);
    w.varint(lane);
    w.bytes(rest);
    w.data
}

/// A lane packet's id in the inner session's terms, or None when the pinned
/// era names no such packet: the engine drops the packet instead of sending
/// a different packet's frame.
fn up_id(lane: i32, state: State, old_id: i32) -> Option<i32> {
    let table = horizon_legacy_join_data::table_up(lane, number_of(state));
    usize::try_from(old_id)
        .ok()
        .and_then(|i| table.get(i).copied().flatten())
        .map(i32::from)
}

fn number_of(state: State) -> u8 {
    match state {
        State::Handshake => 0,
        State::Status => 1,
        State::Login => 2,
        State::Configuration => 3,
        State::Play => 4,
    }
}

/// One registry's RegistryData payload, in the pinned numbering Horizon's
/// configuration reader takes (and resolves back by name).
fn registry_body(registry: &str, names: &[String]) -> Vec<u8> {
    let mut w = Writer::new();
    configuration::RegistryData::write(
        &configuration::RegistryData {
            registry: registry.to_owned(),
            entries: names
                .iter()
                .map(|name| configuration::RegistryEntry {
                    id: name.clone(),
                    data: None,
                })
                .collect(),
        },
        &mut w,
    );
    w.data
}
