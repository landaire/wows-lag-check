//! WoWs replay helpers: version parsing, realm detection, entity-def bundle.

use std::collections::HashMap;

/// Build number from "15,4,0,12506899" -> 12506899.
pub fn build_from_client_version(s: &str) -> Option<u32> {
    s.split(',').nth(3)?.trim().parse().ok()
}

/// "15,4,0,12506899" -> "15.4.0_12506899" (wows-replay-data dir name).
pub fn version_dir_name(s: &str) -> Option<String> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    if parts.len() != 4 {
        return None;
    }
    Some(format!(
        "{}.{}.{}_{}",
        parts[0], parts[1], parts[2], parts[3]
    ))
}

/// Format: [u32 count] ([u32 pathLen][path][u32 contentLen][content])*
pub fn unpack_def_bundle(bundle: &[u8]) -> Option<HashMap<String, Vec<u8>>> {
    let mut map = HashMap::new();
    let mut cur = 0usize;
    let read_u32 = |b: &[u8], at: usize| -> Option<usize> {
        b.get(at..at + 4)
            .map(|s| u32::from_le_bytes(s.try_into().unwrap()) as usize)
    };

    let count = read_u32(bundle, cur)?;
    cur += 4;
    for _ in 0..count {
        let path_len = read_u32(bundle, cur)?;
        cur += 4;
        let path = std::str::from_utf8(bundle.get(cur..cur + path_len)?)
            .ok()?
            .to_string();
        cur += path_len;
        let content_len = read_u32(bundle, cur)?;
        cur += 4;
        let content = bundle.get(cur..cur + content_len)?.to_vec();
        cur += content_len;
        map.insert(path, content);
    }
    Some(map)
}

/// Death-effect names indexed by Exterior param id (species ShipDestruction).
/// Regenerate with: jaq -r '.[]|select(.typeinfo.species=="ShipDestruction")|"\(.id) \(.name)"'
pub fn death_effect_name(param_id: u64) -> Option<&'static str> {
    Some(match param_id {
        4293816240 => "Black Friday 2024",
        4292767664 => "Firework (New Year 2024)",
        4291719088 => "Red (New Year 2024)",
        4290670512 => "Blue (New Year 2024)",
        4289621936 => "April Fools 2025",
        4288573360 => "Golden Month Red",
        4287524784 => "Golden Month Silver",
        4286476208 => "Golden Month Gold",
        4285427632 => "Cartoon Boom",
        4283330480 => "CC Program 2025",
        4282281904 => "Triple Detonation",
        4281233328 => "Big Water Explosion",
        4280184752 => "Phosphorus Bombs",
        4279136176 => "This Is Fine",
        4278087600 => "Blue Explosion",
        4277039024 => "Moray Eel (New Year)",
        4275990448 => "Ice (New Year)",
        4274941872 => "Northern Light (New Year)",
        4271796144 => "Good Team",
        4270747568 => "Bad Team",
        _ => return None,
    })
}

/// Scan for pickle SHORT_BINSTRING realm tokens (`U\x{len}{ascii}`); return
/// the most common match. Spec-free fallback when entity defs aren't loaded.
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
