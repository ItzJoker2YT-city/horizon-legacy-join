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
// Alpha-1 wires lane 763 (1.20.1): its login and status packets match the
// 765 era, and its play packets go through the tables of
// `horizon_legacy_join_data` (identity fallback where its tables end, until
// the generator's per-release tables land in CI and replace them).
use std::sync::Arc;

use super::{ServerVersion, Session};
use crate::{
    Result, State,
    buf::{Codec, Reader, Writer},
    ids,
    packets::configuration,
};

///// Lane registry ///////////////////////////////////////////////////////////

/// One legacy lane: a server release this mod translates for.
pub struct Lane {
    /// The protocol number the server's status reports and its handshake wants.
    pub protocol: i32,
}

/// The whole 1.8.x..1.20.1 span (the roadmap). A lane only counts as joinable
/// when the data crate carries its tables (`covers`), so alpha-1 wires 1.20.1
/// and the span grows release by release.
static LANES: &[Lane] = &[
    Lane { protocol: 763 },
    Lane { protocol: 762 },
    Lane { protocol: 761 },
    Lane { protocol: 760 },
    Lane { protocol: 759 },
    Lane { protocol: 758 },
    Lane { protocol: 757 },
    Lane { protocol: 756 },
    Lane { protocol: 755 },
    Lane { protocol: 754 },
    Lane { protocol: 753 },
    Lane { protocol: 751 },
    Lane { protocol: 578 },
    Lane { protocol: 575 },
    Lane { protocol: 498 },
    Lane { protocol: 480 },
    Lane { protocol: 477 },
    Lane { protocol: 473 },
    Lane { protocol: 441 },
    Lane { protocol: 404 },
    Lane { protocol: 401 },
    Lane { protocol: 389 },
    Lane { protocol: 340 },
    Lane { protocol: 335 },
    Lane { protocol: 316 },
    Lane { protocol: 315 },
    Lane { protocol: 210 },
    Lane { protocol: 110 },
    Lane { protocol: 109 },
    Lane { protocol: 108 },
    Lane { protocol: 107 },
    Lane { protocol: 47 },
];

/// A lane this mod's data covers: the only protocols `older_server` accepts.
pub fn lane(protocol: i32) -> Option<&'static Lane> {
    LANES.iter().find(|lane| lane.protocol == protocol).filter(
        |lane| horizon_legacy_join_data::covers(lane.protocol),
    )
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
        // with the lane's own packet id.
        _ => session
            .serverbound(body)?
            .map(|body| renumber(lane, entry, body)),
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
    let inner_id = up_id(lane, reading, old_id);
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
            let translated = session.clientbound(inner_id, payload)?;
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
/// pushed back, renumbered into the lane's numbering for `state`'s wire.
fn requeue_outgoing(lane: i32, session: &Session, state: State) {
    for body in session.take_outgoing() {
        let renumbered = renumber(lane, state, body);
        session.outgoing.lock().unwrap().push(renumbered);
    }
}

/// A renumbered outgoing body: the inner session's own packet id looked up in
/// the lane's send table for `state`, or the body kept when unmapped.
fn renumber(lane: i32, state: State, body: Vec<u8>) -> Vec<u8> {
    let mut r = Reader::new(&body);
    let Ok(id) = r.varint() else {
        return body;
    };
    let table = horizon_legacy_join_data::table_down(lane, number_of(state));
    let lane_id = table
        .get(usize::try_from(id).unwrap_or(999))
        .copied()
        .flatten()
        .map_or(id, |i| i32::from(i));
    let mut w = Writer::new();
    w.varint(lane_id);
    w.bytes(r.rest());
    w.data
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

/// A lane packet's id in the inner session's terms. Unmapped ids pass as they
/// are: alpha-1's identity tables make that right wherever the ids coincide,
/// and the generated tables keep every mapped id correct.
fn up_id(lane: i32, state: State, old_id: i32) -> i32 {
    let table = horizon_legacy_join_data::table_up(lane, number_of(state));
    usize::try_from(old_id)
        .ok()
        .and_then(|i| table.get(i).copied().flatten())
        .map_or(old_id, |i| i32::from(i))
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
