//! Recognizing which terminal sessions are running a coding agent.
//!
//! The detection itself lives in `okena_core::agents`, where the daemon can
//! reach it: the daemon decides what each agent is doing, and it has to agree
//! with every view about which terminals are agents.

pub use okena_core::agents::{AGENT_COMMANDS, detect_agent};
