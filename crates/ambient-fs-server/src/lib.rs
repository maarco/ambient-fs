// Unix socket and gRPC server

pub mod agents;
// pub mod llm;
pub mod protocol;
pub mod socket;
pub mod state;
pub mod subscriptions;

pub use agents::{AgentActivity, AgentState, AgentTracker};
pub use socket::{SocketServer, SocketError};
pub use state::ServerState;
pub use subscriptions::{SubscriptionManager, SubscriptionId};

