// Unix socket and gRPC server

pub mod agents;
pub mod awareness;
pub mod gateway;
pub mod grpc;
pub mod llm;
pub mod pipeline;
pub mod protocol;
pub mod socket;
pub mod state;
pub mod subscriptions;
pub mod sync;
pub mod tree_state;

pub use agents::{AgentActivity, AgentState, AgentTracker};
pub use awareness::{build_awareness, AwarenessError};
pub use gateway::GatewayServer;
pub use grpc::GrpcServer;
pub use llm::{LlmClient, LlmConfig, LlmError};
pub use pipeline::{AnalysisPipeline, PipelineConfig};
pub use socket::{SocketServer, SocketError};
pub use state::ServerState;
pub use subscriptions::{SubscriptionManager, SubscriptionId};
pub use sync::{PeerConfig, SyncManager, SyncError};
pub use tree_state::{ProjectTree, TreePatch};
