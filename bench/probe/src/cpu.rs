//! CPU time of this process, for the cost of a transfer beside its speed.

/// User and system CPU seconds.
#[derive(Clone, Copy, Debug)]
pub struct Cpu {
    /// Seconds in user mode.
    pub user_s: f64,
    /// Seconds in the kernel.
    pub sys_s: f64,
}

impl Cpu {
    /// `self` minus an earlier reading.
    pub fn since(self, earlier: Cpu) -> Cpu {
        Cpu {
            user_s: self.user_s - earlier.user_s,
            sys_s: self.sys_s - earlier.sys_s,
        }
    }
}

/// Cumulative CPU time of this process. `None` on Windows, where reading it needs unsafe code.
#[cfg(unix)]
pub fn now() -> Option<Cpu> {
    use nix::sys::{
        resource::{UsageWho, getrusage},
        time::{TimeVal, TimeValLike},
    };

    let usage = getrusage(UsageWho::RUSAGE_SELF).ok()?;
    let seconds = |t: TimeVal| t.num_microseconds() as f64 / 1e6;
    Some(Cpu {
        user_s: seconds(usage.user_time()),
        sys_s: seconds(usage.system_time()),
    })
}

/// Cumulative CPU time of this process. `None` on Windows, where reading it needs unsafe code.
#[cfg(not(unix))]
pub fn now() -> Option<Cpu> {
    None
}
