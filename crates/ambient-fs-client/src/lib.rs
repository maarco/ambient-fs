// Client library for connecting to ambient-fsd

mod builder;
mod client;

pub use builder::AmbientFsClientBuilder;
pub use client::{
    AmbientFsClient, AnalysisCompleteParams, AwarenessChangedParams, ClientError,
    ClientNotification, EventFilter, Notification, TreePatchParams, DEFAULT_SOCKET_PATH, Result,
};
