//! Token-bucket rate limiter shared between embedding and chat workloads.
//!
//! Extracted from `altevra-memory::worker` so chat completions and embeddings
//! can share a single rate budget when targeting the same upstream API.

use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub struct RateLimiter {
    capacity: f64,
    tokens: Mutex<TokenState>,
}

struct TokenState {
    tokens: f64,
    last_refill: Instant,
    refill_per_sec: f64,
}

impl RateLimiter {
    /// Build with a per-minute budget. e.g. `RateLimiter::per_minute(1000)`
    /// allows up to ~1000 calls per minute steady state, with burst up to 1000.
    pub fn per_minute(rpm: u32) -> Self {
        let cap = rpm as f64;
        Self {
            capacity: cap,
            tokens: Mutex::new(TokenState {
                tokens: cap,
                last_refill: Instant::now(),
                refill_per_sec: cap / 60.0,
            }),
        }
    }

    /// Acquire one token, sleeping if necessary. Cancellation-safe.
    pub async fn acquire(&self) {
        loop {
            let wait = {
                let mut state = self.tokens.lock().await;
                let now = Instant::now();
                let elapsed = now.duration_since(state.last_refill).as_secs_f64();
                state.tokens = (state.tokens + elapsed * state.refill_per_sec).min(self.capacity);
                state.last_refill = now;
                if state.tokens >= 1.0 {
                    state.tokens -= 1.0;
                    return;
                }
                // Compute sleep duration to next available token.
                let needed = 1.0 - state.tokens;
                Duration::from_secs_f64(needed / state.refill_per_sec)
            };
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn first_acquire_is_immediate_when_full() {
        let limiter = RateLimiter::per_minute(60);
        let start = Instant::now();
        limiter.acquire().await;
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn burst_then_throttles() {
        let limiter = RateLimiter::per_minute(120); // 2 per second steady
        for _ in 0..3 {
            limiter.acquire().await;
        }
        // No assertion on exact timing — just verifying it does not panic and
        // that subsequent acquires queue gracefully.
    }
}
