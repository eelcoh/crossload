//! Human-owned helpers for the acceptance tests in `tests/`: book builders, a simulated
//! CrossPoint reader, and a library on sim capabilities scoped as `run-crossload` scopes them.
//! The agent can't weaken these. Nothing here reaches a real device, a network or a browser.

pub mod books;
pub mod reader;
pub mod world;
