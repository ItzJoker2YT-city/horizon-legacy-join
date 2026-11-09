// helpers/older_servers_enabled.rs — OVERWRITE body of horizon_client::net::older_servers_enabled.
// The gate is off by default upstream while the adapter is incomplete; this mod
// turns it on: upstream's 1.20.2+ support becomes always available, and the
// mod's own legacy lanes ride the same switch.
fn older_servers_enabled() -> bool {
    true
}
