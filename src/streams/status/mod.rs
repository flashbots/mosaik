//! Channel status information and statistics.
//!
//! This module provides types and utilities for monitoring the status
//! of stream channels, including connection states, active subscriptions,
//! and real-time statistics about data transmission.

mod info;
mod when;

pub use {
	info::*,
	when::{ChannelConditions, When},
};
