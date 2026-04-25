use chrono::Local;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClockState {
    /// ISO 8601 local datetime — frontend formats it.
    pub now_local: String,
}

pub fn current() -> ClockState {
    ClockState { now_local: Local::now().to_rfc3339() }
}
