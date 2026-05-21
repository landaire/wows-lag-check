//! Replay helpers that sit on top of the wows-replays parser: realm detection,
//! the arena-state method-id table, and arena_id extraction. Decryption,
//! decompression, and packet walking all come from wows-replays itself.

/// Parse the build number out of `clientVersionFromExe` (e.g. "15,4,0,12506899").
pub fn build_from_client_version(s: &str) -> Option<u32> {
    s.split(',').nth(3)?.trim().parse().ok()
}

/// Method_id of `onArenaStateReceived` on the Avatar entity, per WoWs build.
/// Derived from the game's entity definitions via
/// `replayshark spec <version> -g <game_dir>` (walk Avatar.def and its
/// `<Implements>` interfaces, concatenate client methods, sort by sort_size()).
/// Add a new entry whenever WoWs ships a patch that changes Avatar's method
/// layout.
///
/// Build 12506899 (15.4.0): " - 148: onArenaStateReceived", first arg Int64.
const ARENA_STATE_METHOD_ID: &[(u32, u32)] = &[
    (12506899, 148),
];

/// Look up the `onArenaStateReceived` method_id for a given game build.
/// Returns None for builds not in the table.
pub fn arena_state_method_id(build: u32) -> Option<u32> {
    ARENA_STATE_METHOD_ID
        .iter()
        .find_map(|(b, m)| if *b == build { Some(*m) } else { None })
}

/// Extract the canonical arenaUniqueId from an EntityMethod packet body, given
/// the expected `onArenaStateReceived` method_id. Returns None when the body's
/// method_id doesn't match.
///
/// EntityMethod body layout (BigWorld):
///   [0..4]   entity_id
///   [4..8]   method_id
///   [8..12]  payload_length
///   [12..20] arena_id (Int64, first arg of onArenaStateReceived)
pub fn arena_id_from_packet_body(raw: &[u8], method_id: u32) -> Option<i64> {
    if raw.len() < 20 {
        return None;
    }
    let m = u32::from_le_bytes(raw[4..8].try_into().unwrap());
    if m != method_id {
        return None;
    }
    Some(i64::from_le_bytes(raw[12..20].try_into().unwrap()))
}

/// Scan the decrypted packet bytes for pickle SHORT_BINSTRING tokens matching
/// known WoWs realm codes. The receivePlayerData blob stores each player's
/// realm as `U\x{len}{ascii}`, with the key name memoized once so it doesn't
/// recur. The most-common matching realm is the match's server.
pub fn detect_realm(packet_data: &[u8]) -> Option<&'static str> {
    let candidates: &[(&[u8], &str)] = &[
        (b"\x55\x02EU", "EU"),
        (b"\x55\x02NA", "NA"),
        (b"\x55\x04ASIA", "ASIA"),
        (b"\x55\x02RU", "RU"),
        (b"\x55\x02SG", "SG"),
        (b"\x55\x03CIS", "CIS"),
    ];
    let mut best: Option<(&'static str, usize)> = None;
    for (pat, name) in candidates {
        let mut count = 0usize;
        let mut i = 0;
        while let Some(off) = memmem(&packet_data[i..], pat) {
            count += 1;
            i += off + 1;
        }
        if count >= 3 && best.map(|(_, c)| count > c).unwrap_or(true) {
            best = Some((name, count));
        }
    }
    best.map(|(n, _)| n)
}

fn memmem(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
