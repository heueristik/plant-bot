use std::time::Duration;

pub fn seconds(n: u32) -> Duration {
    Duration::from_secs(n as u64)
}

pub fn minutes(n: u32) -> Duration {
    n * seconds(60)
}

#[allow(dead_code)]
pub fn hours(n: u32) -> Duration {
    n * minutes(60)
}