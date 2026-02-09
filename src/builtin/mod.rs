//! Built-in implementations of common components, such as in-memory storage and
//! a no-op state machine.

mod memory;
mod noop;

pub mod groups {
	pub use super::{memory::InMemory, noop::NoOp};
}
