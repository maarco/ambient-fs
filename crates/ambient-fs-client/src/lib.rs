// Client library for connecting to ambient-fsd

mod builder;
mod client;

pub use builder::AmbientFsClientBuilder;
pub use client::{AmbientFsClient, ClientError, DEFAULT_SOCKET_PATH, EventFilter, Result};
