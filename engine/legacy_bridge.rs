//! The legacy lane engine of horizon-legacy-join: joining servers older than
//! upstream's own adapter floor (1.20.2). Injected as `legacy_join` into
//! `adapter.rs`, so it shares the adapter's module tree: one inner `Session`
//! pinned to [`ServerVersion::V1_20_3`] does the renumbering, payload steps
//! and state machine upstream built for 1.20.3-era servers, while this bridge
//! carries a lane wire (its own protocol number and its own packet ids) into
//! that session and fabricates the phases the era did not speak: whole
//! configuration state (registries, tags, known packs, the finish) a server
//! before 1.20.2 never sends, and the login acknowledgment it never answers.
//!
//! Alpha-1 wires lane 763 (1.20.1): its login and status packets match the
//! 765 era, and its play packets go through the tables of
//! `horizon_legacy_join_data` (identity fallback in this alpha, until the
//! generator's per-release tables land in CI and replace them).
use std::collections::VecDeque;

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
    LANES
        .iter()
        .find(|lane| lane.protocol == protocol)
        .filter(|lane| horizon_legacy_join_data::covers(lane.protocol))
}

/// The variant a covered lane resolves to through the overwritten
/// `from_protocol`: only its `is_some()` matters (the server list), the join
/// itself reads the thread-local below.
pub fn marker(protocol: i32) -> Option<ServerVersion> {
    lane(protocol).map(|_| ServerVersion::V1_20_2)
}

///// Thread-local handoff (older_server and adapt are both overwritten) //////

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

///// The bridge //////////////////////////////////////////////////////////////

/// Per-connection translator, shared by both halves of the connection (the
/// injected per-instance state holds it as `Arc<Mutex<Bridge>>`), with one
/// inner upstream session as the state authority for everything it does.
pub struct Bridge {
    lane: &'static Lane,
    /// The upstream session this wire is made to look like 1.20.3 to.
    inner: Session,
    /// Translations waiting for Horizon to read, in pinned numbering.
    incoming: VecDeque<(i32, Vec<u8>)>,
    /// Bodies Horizon sends next (follow-ups the inner session's translation
    /// produced), renumbered into the lane's own ids.
    outgoing: Vec<Vec<u8>>,
}

impl Bridge {
    pub fn new(protocol: i32) -> Self {
        Self {
            lane: lane(protocol).unwrap_or(&LANES[0]),
            inner: Session::new(ServerVersion::V1_20_3),
            incoming: VecDeque::new(),
            outgoing: Vec::new(),
        }
    }

    /// The next translation Horizon reads.
    pub fn take_incoming(&mut self) -> Option<(i32, Vec<u8>)> {
        self.incoming.pop_front()
    }

    /// Bodies Horizon sends right behind the packet it just sent.
    pub fn take_outgoing(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.outgoing)
    }

    /// The inner session's own follow-ups (it queues split or extra packets
    /// there), renumbered into the lane's numbers for this state's wire.
    fn pull_inner_outgoing(&mut self, state: State) {
        for body in self.inner.take_outgoing() {
            self.outgoing.push(self.swap_outgoing_id(state, body));
        }
    }

    /// A packet Horizon sends, as its full pinned-numbered body. The lane
    /// wire's answer (re-identified into the lane's own ids), or `None` when
    /// this lane keeps the packet to itself (the fake phases) or has no
    /// such packet at all.
    pub fn serverbound(&mut self, body: &[u8]) -> Result<Option<Vec<u8>>> {
        let entry = self.inner.writing();
        let packet = Reader::new(body).varint().ok().unwrap_or(-1);
        let forwarded = match entry {
            // The handshake passes as it is: the inner session reads its own
            // protocol field, states its next states by the intent and answers
            // with its own protocol number, which is swapped to the lane's.
            State::Handshake => self
                .inner
                .serverbound(body)?
                .map(|body| self.swap_intention_protocol(body)),
            // A server before 1.20.2 answered neither the login acknowledgment
            // nor any configuration packet. The inner session keeps its own
            // state machine through them; the wire stays quiet.
            State::Login if packet == ids::login::serverbound::LOGIN_ACKNOWLEDGED => {
                let _ = self.inner.serverbound(body)?;
                None
            }
            State::Configuration => {
                let _ = self.inner.serverbound(body)?;
                None
            }
            // The real traffic: the inner session renumbers it, then it is
            // rebuilt with the lane's own packet id.
            _ => self
                .inner
                .serverbound(body)?
                .map(|body| self.swap_outgoing_id(entry, body)),
        };
        self.pull_inner_outgoing(entry);
        Ok(forwarded)
    }

    /// A packet the lane server sent, in its own ids and its own layouts.
    /// Horizon's pinned-numbered packet, or `None` when the inner session
    /// knows no such packet. The one non-passthrough stop is the login's
    /// finish: the moment the fake configuration phase is queued (it is read
    /// before any further frame, so it lands entirely ahead of the play feed
    /// the server starts right after the login).
    pub fn clientbound(
        &mut self,
        old_id: i32,
        payload: Vec<u8>,
    ) -> Result<Option<(i32, Vec<u8>)>> {
        let reading = self.inner.reading();
        let inner_id = up_id(self.lane.protocol, reading, old_id);
        match reading {
            State::Login => {
                let translated = self.inner.clientbound(inner_id, payload)?;
                self.pull_inner_outgoing(reading);
                // The login went through: queue the phase the era lacks, and
                // steer the inner session into its play state behind it.
                if self.inner.reading() == State::Configuration {
                    self.fabricate_configuration();
                }
                Ok(translated)
            }
            // The connection hears nothing on the wire during this phase: its
            // client side was fabricated. Strays (a second finish somehow) drop.
            State::Configuration => Ok(None),
            _ => {
                let translated = self.inner.clientbound(inner_id, payload)?;
                self.pull_inner_outgoing(reading);
                Ok(translated)
            }
        }
    }

    /// The fake configuration phase: every static registry of the era as its
    /// own RegistryData (entries named, no worldgen data), no tags, no known
    /// packs, then the finish — which flips the inner session to its play
    /// state so the real play feed right behind the queue decodes.
    fn fabricate_configuration(&mut self) {
        let cb = ids::configuration::clientbound;
        let mut tables: Vec<(String, Vec<String>)> =
            super::legacy::registries(ServerVersion::V1_20_2)
                .into_iter()
                .map(|(registry, ids)| (registry, ids.names))
                .collect();
        tables.sort_by(|a, b| a.0.cmp(&b.0));
        for (registry, names) in tables {
            self.incoming
                .push_back((cb::REGISTRY_DATA, registry_body(&registry, &names)));
        }
        let mut w = Writer::new();
        configuration::UpdateTags::write(&configuration::UpdateTags { registries: Vec::new() }, &mut w);
        self.incoming.push_back((cb::UPDATE_TAGS, w.data));
        let mut w = Writer::new();
        configuration::SelectKnownPacks::write(
            &configuration::SelectKnownPacks { packs: Vec::new() },
            &mut w,
        );
        self.incoming.push_back((cb::SELECT_KNOWN_PACKS, w.data));
        self.incoming.push_back((cb::FINISH_CONFIGURATION, Vec::new()));
        let _ = self.inner.clientbound(cb::FINISH_CONFIGURATION, Vec::new());
    }

    /// The inner session's handshake answer comes with its own protocol
    /// number in place; the lane server wants its own. [id][protocol][rest].
    fn swap_intention_protocol(&self, body: Vec<u8>) -> Vec<u8> {
        let mut r = Reader::new(&body);
        if r.varint().is_err() || r.varint().is_err() {
            return body;
        }
        let rest = r.rest();
        let mut w = Writer::new();
        w.varint(ids::handshake::serverbound::INTENTION);
        w.varint(self.lane.protocol);
        w.bytes(rest);
        w.data
    }

    /// A serverbound answer (the inner session's own ids) rebuilt with the
    /// lane's packet id.
    fn swap_outgoing_id(&self, state: State, body: Vec<u8>) -> Vec<u8> {
        let mut r = Reader::new(&body);
        let Ok(id) = r.varint() else {
            return body;
        };
        let lane = horizon_legacy_join_data::table_down(self.lane.protocol, number_of(state));
        let lane_id = lane
            .get(usize::try_from(id).unwrap_or(999))
            .copied()
            .flatten()
            .map_or(id, |i| i32::from(i));
        let mut w = Writer::new();
        w.varint(lane_id);
        w.bytes(r.rest());
        w.data
    }
}

///// auxiliaries /////////////////////////////////////////////////////////////

/// A lane packet's id in the inner session's terms. Unmapped ids pass as they
/// are: alpha-1's identity tables make that right wherever the ids coincide,
/// and the generated tables keep every mapped id correct.
fn up_id(protocol: i32, state: State, old_id: i32) -> i32 {
    let table = horizon_legacy_join_data::table_up(protocol, number_of(state));
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
