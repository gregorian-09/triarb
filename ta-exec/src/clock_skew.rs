use std::net::UdpSocket;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Maximum acceptable clock offset before we refuse to send signed requests.
pub const MAX_CLOCK_SKEW_MS: i64 = 1000;

/// Result of an NTP clock check.
#[derive(Debug, Clone)]
pub struct ClockSkew {
    /// Estimated offset of local clock relative to NTP (positive = local is ahead).
    pub offset_ms: i64,
    /// Round-trip delay in ms.
    pub delay_ms: u64,
}

impl ClockSkew {
    /// Returns true if the local clock is within acceptable bounds.
    pub fn is_safe(&self) -> bool {
        self.offset_ms.abs() <= MAX_CLOCK_SKEW_MS
    }
}

/// Query an NTP server over UDP and compute the local clock offset.
///
/// Uses a simplified SNTP (RFC 4330) exchange. On any I/O failure
/// returns `None` so the caller can decide whether to proceed.
pub fn check_ntp(server: &str) -> Option<ClockSkew> {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return None;
    };
    sock.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let Ok(_) = sock.connect((server, 123u16)) else {
        return None;
    };

    // Build NTPv4 client packet (mode 3)
    let mut tx = [0u8; 48];
    tx[0] = 0x1B; // LI=0, VN=3 (v3 for max compat), Mode=3 (client)
    let t1 = ntp_now();
    tx[40..48].copy_from_slice(&t1.to_be_bytes());

    let tx_ns = nanos();

    if sock.send(&tx).is_err() {
        return None;
    }

    let mut rx = [0u8; 48];
    if sock.recv(&mut rx).is_err() {
        return None;
    }

    let t4_ns = nanos();

    // Parse server timestamps (NTP 64-bit: 32-bit seconds + 32-bit fraction)
    let t2_raw = u64::from_be_bytes([
        rx[32], rx[33], rx[34], rx[35], rx[36], rx[37], rx[38], rx[39],
    ]);
    let t3_raw = u64::from_be_bytes([
        rx[40], rx[41], rx[42], rx[43], rx[44], rx[45], rx[46], rx[47],
    ]);

    let t2 = ntp_to_nanos(t2_raw);
    let t3 = ntp_to_nanos(t3_raw);

    // Clock offset: ((T2 - T1) + (T3 - T4)) / 2
    let offset_ns = ((t2 as i128 - tx_ns as i128) + (t3 as i128 - t4_ns as i128)) / 2;
    let delay_ns = (t4_ns as i128 - tx_ns as i128) - (t3 as i128 - t2 as i128);

    Some(ClockSkew {
        offset_ms: (offset_ns / 1_000_000) as i64,
        delay_ms: (delay_ns.max(0) / 1_000_000) as u64,
    })
}

/// Current time in NTP 64-bit fixed-point format (seconds since 1900).
fn ntp_now() -> u64 {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs() + NTP_UNIX_OFFSET;
    let frac = ((d.subsec_nanos() as u64) << 32) / 1_000_000_000;
    (secs << 32) | frac
}

/// Convert an NTP 64-bit timestamp to nanoseconds since Unix epoch.
fn ntp_to_nanos(raw: u64) -> u64 {
    let secs = (raw >> 32).saturating_sub(NTP_UNIX_OFFSET);
    let frac_ns = ((((raw & 0xFFFF_FFFF) as u128) * 1_000_000_000) >> 32) as u64;
    secs * 1_000_000_000 + frac_ns
}

/// Seconds between NTP epoch (1900-01-01) and Unix epoch (1970-01-01).
const NTP_UNIX_OFFSET: u64 = 2_208_988_800;

/// Default NTP server pool.
pub const DEFAULT_NTP_SERVER: &str = "pool.ntp.org";

fn nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ntp_now_roundtrip() {
        let now = ntp_now();
        let back = ntp_to_nanos(now);
        let sys = nanos();
        let diff = back.abs_diff(sys);
        assert!(diff < 10_000_000, "NTP conversion off by {diff}ns");
    }

    #[test]
    fn test_ntp_query() {
        let result = check_ntp(DEFAULT_NTP_SERVER);
        if let Some(skew) = result {
            assert!(
                skew.offset_ms.abs() < 10_000,
                "NTP offset implausible: {}ms",
                skew.offset_ms
            );
            assert!(
                skew.delay_ms < 10_000,
                "NTP delay implausible: {}ms",
                skew.delay_ms
            );
        }
    }
}
