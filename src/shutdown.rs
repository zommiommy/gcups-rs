use core::fmt;
use core::time::Duration;

use crate::device::UpsTransport;
use crate::wire::MEGATEC_MAX_COMMAND_LEN;

/// Select the table entry with the greatest delay not exceeding `requested`,
/// falling back to the smallest entry. Tables are ascending.
fn greatest_not_exceeding<T: Copy>(
    table: &[T],
    requested: Duration,
    delay_of: impl Fn(T) -> Duration,
) -> T {
    let mut best = table[0];
    for &entry in table {
        if delay_of(entry) <= requested {
            best = entry;
        }
    }
    best
}

/// Supported shutdown delays.
///
/// The exact delay grid depends on the USB transport. [`Ups::shutdown`](crate::Ups::shutdown)
/// and [`Ups::shutdown_and_restore`](crate::Ups::shutdown_and_restore) return the delay
/// selected for the currently opened device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShutdownDelay {
    delay: Duration,
}

impl ShutdownDelay {
    pub(crate) const fn new(secs: u64) -> Self {
        Self {
            delay: Duration::from_secs(secs),
        }
    }

    /// Select the greatest delay supported by `transport` that is ≤ `requested`.
    /// Falls back to the smallest step when `requested` is shorter than any step.
    pub fn from_duration(requested: Duration, transport: UpsTransport) -> Self {
        match transport {
            UpsTransport::Descriptor => DescriptorShutdownDelay::from_duration(requested).delay,
            UpsTransport::CypressHid => MegatecShutdownDelay::from_duration(requested).delay,
        }
    }

    /// The actual delay the UPS will use.
    pub fn actual_delay(&self) -> Duration {
        self.delay
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DescriptorShutdownDelay {
    pub(crate) delay: ShutdownDelay,
    pub(crate) shutdown_report: u8,
    pub(crate) restore_report: u8,
}

impl DescriptorShutdownDelay {
    /// All supported MEC0003 delay steps, ascending.
    const TABLE: &[DescriptorShutdownDelay] = &[
        DescriptorShutdownDelay::new(30, 0x18, 0x10),
        DescriptorShutdownDelay::new(35, 0x28, 0x20),
        DescriptorShutdownDelay::new(40, 0x38, 0x30),
        DescriptorShutdownDelay::new(47, 0x48, 0x40),
        DescriptorShutdownDelay::new(53, 0x58, 0x50),
        DescriptorShutdownDelay::new(60, 0x68, 0x60),
        DescriptorShutdownDelay::new(120, 0x78, 0x70),
        DescriptorShutdownDelay::new(180, 0x88, 0x80),
        DescriptorShutdownDelay::new(240, 0x98, 0x90),
        DescriptorShutdownDelay::new(300, 0xa8, 0xa0),
        DescriptorShutdownDelay::new(360, 0xb8, 0xb0),
        DescriptorShutdownDelay::new(420, 0xc8, 0xc0),
        DescriptorShutdownDelay::new(480, 0xd8, 0xd0),
        DescriptorShutdownDelay::new(540, 0xe8, 0xe0),
    ];

    const fn new(secs: u64, shutdown: u8, restore: u8) -> Self {
        Self {
            delay: ShutdownDelay::new(secs),
            shutdown_report: shutdown,
            restore_report: restore,
        }
    }

    pub(crate) fn from_duration(requested: Duration) -> Self {
        greatest_not_exceeding(Self::TABLE, requested, |e| e.delay.actual_delay())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MegatecShutdownDelay {
    pub(crate) delay: ShutdownDelay,
    code: MegatecDelayCode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MegatecDelayCode {
    Tenths(u8),
    Minutes(u8),
}

impl MegatecShutdownDelay {
    /// All supported Megatec delay steps, ascending.
    const TABLE: &[MegatecShutdownDelay] = &[
        MegatecShutdownDelay::new_tenths(12, 2),
        MegatecShutdownDelay::new_tenths(18, 3),
        MegatecShutdownDelay::new_tenths(24, 4),
        MegatecShutdownDelay::new_tenths(30, 5),
        MegatecShutdownDelay::new_tenths(36, 6),
        MegatecShutdownDelay::new_tenths(42, 7),
        MegatecShutdownDelay::new_tenths(48, 8),
        MegatecShutdownDelay::new_tenths(54, 9),
        MegatecShutdownDelay::new_minutes(60, 1),
        MegatecShutdownDelay::new_minutes(120, 2),
        MegatecShutdownDelay::new_minutes(180, 3),
        MegatecShutdownDelay::new_minutes(240, 4),
        MegatecShutdownDelay::new_minutes(300, 5),
        MegatecShutdownDelay::new_minutes(360, 6),
        MegatecShutdownDelay::new_minutes(420, 7),
        MegatecShutdownDelay::new_minutes(480, 8),
        MegatecShutdownDelay::new_minutes(540, 9),
        MegatecShutdownDelay::new_minutes(600, 10),
    ];

    const fn new_tenths(secs: u64, tenths: u8) -> Self {
        Self {
            delay: ShutdownDelay::new(secs),
            code: MegatecDelayCode::Tenths(tenths),
        }
    }

    const fn new_minutes(secs: u64, minutes: u8) -> Self {
        Self {
            delay: ShutdownDelay::new(secs),
            code: MegatecDelayCode::Minutes(minutes),
        }
    }

    pub(crate) fn from_duration(requested: Duration) -> Self {
        greatest_not_exceeding(Self::TABLE, requested, |e| e.delay.actual_delay())
    }

    pub(crate) fn write_shutdown_command(
        &self,
        buf: &mut [u8; MEGATEC_MAX_COMMAND_LEN],
        stay_off: bool,
    ) -> usize {
        let mut len = 0;
        buf[len] = b'S';
        len += 1;

        match self.code {
            MegatecDelayCode::Tenths(tenths) => {
                buf[len] = b'.';
                len += 1;
                buf[len] = b'0' + tenths;
                len += 1;
            }
            MegatecDelayCode::Minutes(minutes) => {
                buf[len] = b'0' + minutes / 10;
                len += 1;
                buf[len] = b'0' + minutes % 10;
                len += 1;
            }
        }

        // The bare `S<n>` form reconnects the output ~10 s after mains is
        // recovered (Megatec spec), so there is no standard "stay off" command.
        // Stay-off therefore uses the `S<n>R0000` convention from NUT's
        // blazer/nutdrv_qx drivers, where `R0000` ("never restore") holds the
        // output off.
        if stay_off {
            buf[len..len + 5].copy_from_slice(b"R0000");
            len += 5;
        }

        buf[len] = b'\r';
        len += 1;
        len
    }
}

impl fmt::Display for ShutdownDelay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}s", self.delay.as_secs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn megatec_shutdown_delay_lookup_and_encoding() {
        let sd = MegatecShutdownDelay::from_duration(Duration::from_secs(45));
        assert_eq!(sd.delay.actual_delay(), Duration::from_secs(42));

        let mut command = [0; MEGATEC_MAX_COMMAND_LEN];
        // Return-on-mains: bare `S<n>` (reconnects ~10 s after mains returns).
        let len = sd.write_shutdown_command(&mut command, false);
        assert_eq!(&command[..len], b"S.7\r");
        // Stay-off: `S<n>R0000` (R0000 = never restore).
        let len = sd.write_shutdown_command(&mut command, true);
        assert_eq!(&command[..len], b"S.7R0000\r");

        let sd = MegatecShutdownDelay::from_duration(Duration::from_secs(5));
        assert_eq!(sd.delay.actual_delay(), Duration::from_secs(12));

        let sd = MegatecShutdownDelay::from_duration(Duration::from_secs(60));
        let len = sd.write_shutdown_command(&mut command, false);
        assert_eq!(&command[..len], b"S01\r");
        let len = sd.write_shutdown_command(&mut command, true);
        assert_eq!(&command[..len], b"S01R0000\r");
    }
}
