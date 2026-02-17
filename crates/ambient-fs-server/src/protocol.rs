// JSON-RPC 2.0 protocol for ambient-fs server
//
// Spec: https://www.jsonrpc.org/specification
//
// Request format:
//   {"jsonrpc":"2.0","method":"subscribe","params":{"project_id":"abc"},"id":1}
//
// Response format:
//   {"jsonrpc":"2.0","result":{...},"id":1}
//   {"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid Request"},"id":1}

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 request
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// JSON-RPC version, must be "2.0"
    pub jsonrpc: String,
    /// Method name to invoke
    pub method: String,
    /// Parameters (object or array, optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Params>,
    /// Request identifier (number, string, or null)
    /// Optional for notifications (requests without id)
    #[serde(default)]
    pub id: Id,
}

impl Request {
    /// Create a new request with the given method, params, and id
    pub fn new(method: impl Into<String>, params: Option<Params>, id: Id) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
            id,
        }
    }

    /// Create a notification (request without id)
    pub fn notification(method: impl Into<String>, params: Option<Params>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
            id: Id::Null,
        }
    }

    /// Returns true if this is a notification (no id)
    pub fn is_notification(&self) -> bool {
        matches!(&self.id, Id::Null)
    }
}

/// Request identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Null,
    Number(i64),
    String(String),
}

impl Default for Id {
    fn default() -> Self {
        Self::Null
    }
}

/// Parameters can be an object (named) or array (positional)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Params {
    Array(Vec<Value>),
    Object(serde_json::Map<String, Value>),
}

/// JSON-RPC 2.0 response
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    /// JSON-RPC version, must be "2.0"
    pub jsonrpc: String,
    /// Result if successful (absent on error)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error if failed (absent on success)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Error>,
    /// Request identifier (must match request)
    pub id: Id,
}

impl Response {
    /// Create a successful response
    pub fn result(id: Id, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }

    /// Create an error response
    pub fn error(id: Id, error: Error) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(error),
            id,
        }
    }

    /// Returns true if this response is successful
    pub fn is_success(&self) -> bool {
        self.error.is_none() && self.result.is_some()
    }
}

/// JSON-RPC 2.0 error object
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Error {
    /// Error code
    pub code: i32,
    /// Short description
    pub message: String,
    /// Additional data (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl Error {
    /// Create a new error
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Create an error with additional data
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

// Standard JSON-RPC error codes
impl Error {
    pub fn parse_error() -> Self {
        Self::new(-32700, "Parse error")
    }

    pub fn invalid_request() -> Self {
        Self::new(-32600, "Invalid Request")
    }

    pub fn method_not_found(method: String) -> Self {
        Self::new(-32601, format!("Method not found: {}", method))
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::new(-32602, msg.into())
    }

    pub fn internal_error() -> Self {
        Self::new(-32603, "Internal error")
    }
}

// Server-specific error codes (positive range for app-specific)
impl Error {
    pub fn project_not_found(id: String) -> Self {
        Self::new(-1001, format!("Project not found: {}", id))
    }

    pub fn already_watching(path: String) -> Self {
        Self::new(-1002, format!("Already watching: {}", path))
    }

    pub fn watch_failed(path: String) -> Self {
        Self::new(-1003, format!("Failed to watch: {}", path))
    }

    pub fn invalid_path(path: String) -> Self {
        Self::new(-1004, format!("Invalid path: {}", path))
    }
}

/// Method names
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Subscribe,
    Unsubscribe,
    QueryEvents,
    QueryAwareness,
    WatchProject,
    UnwatchProject,
    WatchAgents,
    UnwatchAgents,
    QueryAgents,
    QueryTree,
    Attribute,
}

impl Method {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Subscribe => "subscribe",
            Self::Unsubscribe => "unsubscribe",
            Self::QueryEvents => "query_events",
            Self::QueryAwareness => "query_awareness",
            Self::WatchProject => "watch_project",
            Self::UnwatchProject => "unwatch_project",
            Self::WatchAgents => "watch_agents",
            Self::UnwatchAgents => "unwatch_agents",
            Self::QueryAgents => "query_agents",
            Self::QueryTree => "query_tree",
            Self::Attribute => "attribute",
        }
    }
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Method {
    type Err = MethodParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "subscribe" => Ok(Self::Subscribe),
            "unsubscribe" => Ok(Self::Unsubscribe),
            "query_events" => Ok(Self::QueryEvents),
            "query_awareness" => Ok(Self::QueryAwareness),
            "watch_project" => Ok(Self::WatchProject),
            "unwatch_project" => Ok(Self::UnwatchProject),
            "watch_agents" => Ok(Self::WatchAgents),
            "unwatch_agents" => Ok(Self::UnwatchAgents),
            "query_agents" => Ok(Self::QueryAgents),
            "query_tree" => Ok(Self::QueryTree),
            "attribute" => Ok(Self::Attribute),
            _ => Err(MethodParseError(s.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodParseError(pub String);

impl std::fmt::Display for MethodParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown method: {}", self.0)
    }
}

impl std::error::Error for MethodParseError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use pretty_assertions::assert_eq;

    // Id tests

    #[test]
    fn id_null_roundtrip() {
        let id = Id::Null;
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "null");
        let parsed: Id = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn id_number_roundtrip() {
        let id = Id::Number(42);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "42");
        let parsed: Id = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn id_string_roundtrip() {
        let id = Id::String("abc123".to_string());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"abc123\"");
        let parsed: Id = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn id_default_is_null() {
        assert_eq!(Id::default(), Id::Null);
    }

    // Params tests

    #[test]
    fn params_array_roundtrip() {
        let params = Params::Array(vec![json!("hello"), json!(42)]);
        let json = serde_json::to_string(&params).unwrap();
        assert_eq!(json, "[\"hello\",42]");
        let parsed: Params = serde_json::from_str(&json).unwrap();
        assert_eq!(params, parsed);
    }

    #[test]
    fn params_object_roundtrip() {
        let mut map = serde_json::Map::new();
        map.insert("project_id".to_string(), json!("abc"));
        map.insert("since".to_string(), json!(3600));
        let params = Params::Object(map);
        let json = serde_json::to_string(&params).unwrap();
        let parsed: Params = serde_json::from_str(&json).unwrap();
        assert_eq!(params, parsed);
    }

    // Request tests

    #[test]
    fn request_with_object_params() {
        let req = Request::new(
            "subscribe".to_string(),
            Some(Params::Object({
                let mut m = serde_json::Map::new();
                m.insert("project_id".to_string(), json!("my-project"));
                m
            })),
            Id::Number(1),
        );

        let json = serde_json::to_string(&req).unwrap();
        let expected = r#"{"jsonrpc":"2.0","method":"subscribe","params":{"project_id":"my-project"},"id":1}"#;
        assert_eq!(json, expected);

        let parsed: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.method, "subscribe");
        assert_eq!(parsed.id, Id::Number(1));
    }

    #[test]
    fn request_with_array_params() {
        let req = Request::new(
            "query_events".to_string(),
            Some(Params::Array(vec![json!("my-project"), json!(3600)])),
            Id::Number(2),
        );

        let json = serde_json::to_string(&req).unwrap();
        let parsed: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.method, "query_events");
    }

    #[test]
    fn request_without_params() {
        let req = Request::new("status".to_string(), None, Id::String("req1".to_string()));

        let json = serde_json::to_string(&req).unwrap();
        let expected = r#"{"jsonrpc":"2.0","method":"status","id":"req1"}"#;
        assert_eq!(json, expected);

        let parsed: Request = serde_json::from_str(&json).unwrap();
        assert!(parsed.params.is_none());
    }

    #[test]
    fn notification_has_no_id() {
        let req = Request::notification(
            "ping".to_string(),
            Some(Params::Array(vec![json!("hello")])),
        );

        assert!(req.is_notification());
        assert_eq!(req.id, Id::Null);
    }

    #[test]
    fn request_with_id_is_not_notification() {
        let req = Request::new("ping".to_string(), None, Id::Number(1));
        assert!(!req.is_notification());
    }

    // Response tests

    #[test]
    fn success_response() {
        let resp = Response::result(Id::Number(1), json!({"status":"ok"}));

        let json = serde_json::to_string(&resp).unwrap();
        let expected = r#"{"jsonrpc":"2.0","result":{"status":"ok"},"id":1}"#;
        assert_eq!(json, expected);

        let parsed: Response = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_success());
        assert!(parsed.error.is_none());
        assert_eq!(parsed.result, Some(json!({"status":"ok"})));
    }

    #[test]
    fn error_response() {
        let err = Error::method_not_found("bogus_method".to_string());
        let resp = Response::error(Id::Number(1), err);

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: Response = serde_json::from_str(&json).unwrap();
        assert!(!parsed.is_success());
        assert!(parsed.result.is_none());
        assert!(parsed.error.is_some());
        assert_eq!(parsed.error.unwrap().code, -32601);
    }

    #[test]
    fn response_with_null_id() {
        let resp = Response::result(Id::Null, json!(true));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"id\":null"));
    }

    // Error tests

    #[test]
    fn error_standard_codes() {
        assert_eq!(Error::parse_error().code, -32700);
        assert_eq!(Error::invalid_request().code, -32600);
        assert_eq!(Error::method_not_found("x".to_string()).code, -32601);
        assert_eq!(Error::invalid_params("x").code, -32602);
        assert_eq!(Error::internal_error().code, -32603);
    }

    #[test]
    fn error_with_data() {
        let err = Error::invalid_params("missing project_id".to_string())
            .with_data(json!({"field":"project_id","required":true}));

        let json = serde_json::to_string(&err).unwrap();
        let parsed: Error = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.code, -32602);
        assert!(parsed.data.is_some());
    }

    #[test]
    fn error_server_specific_codes() {
        assert_eq!(Error::project_not_found("x".to_string()).code, -1001);
        assert_eq!(Error::already_watching("x".to_string()).code, -1002);
        assert_eq!(Error::watch_failed("x".to_string()).code, -1003);
        assert_eq!(Error::invalid_path("x".to_string()).code, -1004);
    }

    // Method enum tests

    #[test]
    fn method_to_string() {
        assert_eq!(Method::Subscribe.as_str(), "subscribe");
        assert_eq!(Method::Unsubscribe.as_str(), "unsubscribe");
        assert_eq!(Method::QueryEvents.as_str(), "query_events");
        assert_eq!(Method::QueryAwareness.as_str(), "query_awareness");
        assert_eq!(Method::WatchProject.as_str(), "watch_project");
        assert_eq!(Method::UnwatchProject.as_str(), "unwatch_project");
        assert_eq!(Method::WatchAgents.as_str(), "watch_agents");
        assert_eq!(Method::UnwatchAgents.as_str(), "unwatch_agents");
        assert_eq!(Method::QueryAgents.as_str(), "query_agents");
        assert_eq!(Method::QueryTree.as_str(), "query_tree");
        assert_eq!(Method::Attribute.as_str(), "attribute");
    }

    #[test]
    fn method_from_str_valid() {
        assert_eq!("subscribe".parse::<Method>().unwrap(), Method::Subscribe);
        assert_eq!("unsubscribe".parse::<Method>().unwrap(), Method::Unsubscribe);
        assert_eq!("query_events".parse::<Method>().unwrap(), Method::QueryEvents);
        assert_eq!("query_awareness".parse::<Method>().unwrap(), Method::QueryAwareness);
        assert_eq!("watch_project".parse::<Method>().unwrap(), Method::WatchProject);
        assert_eq!("unwatch_project".parse::<Method>().unwrap(), Method::UnwatchProject);
        assert_eq!("watch_agents".parse::<Method>().unwrap(), Method::WatchAgents);
        assert_eq!("unwatch_agents".parse::<Method>().unwrap(), Method::UnwatchAgents);
        assert_eq!("query_agents".parse::<Method>().unwrap(), Method::QueryAgents);
        assert_eq!("query_tree".parse::<Method>().unwrap(), Method::QueryTree);
        assert_eq!("attribute".parse::<Method>().unwrap(), Method::Attribute);
    }

    #[test]
    fn method_from_str_invalid() {
        let result: Result<Method, _> = "bogus".parse();
        assert!(result.is_err());
    }

    // Integration tests

    #[test]
    fn full_roundtrip_subscribe_request() {
        let json_input = r#"{"jsonrpc":"2.0","method":"subscribe","params":{"project_id":"my-project"},"id":1}"#;
        let req: Request = serde_json::from_str(json_input).unwrap();

        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "subscribe");
        assert_eq!(req.id, Id::Number(1));

        // Send success response
        let resp = Response::result(req.id.clone(), json!({"subscribed":true}));
        let json_output = serde_json::to_string(&resp).unwrap();
        assert!(json_output.contains("\"result\""));
        assert!(json_output.contains("\"subscribed\""));
    }

    #[test]
    fn full_roundtrip_error_response() {
        let json_input = r#"{"jsonrpc":"2.0","method":"watch_project","params":{"path":"/nonexistent"},"id":2}"#;
        let req: Request = serde_json::from_str(json_input).unwrap();

        // Send error response
        let err = Error::invalid_path("/nonexistent".to_string());
        let resp = Response::error(req.id, err);

        let json_output = serde_json::to_string(&resp).unwrap();
        let parsed_resp: Response = serde_json::from_str(&json_output).unwrap();
        assert!(!parsed_resp.is_success());
        assert_eq!(parsed_resp.error.unwrap().code, -1004);
    }

    #[test]
    fn notification_parse() {
        let json_input = r#"{"jsonrpc":"2.0","method":"ping","params":[1,2,3]}"#;
        let req: Request = serde_json::from_str(json_input).unwrap();

        assert!(req.is_notification());
        assert_eq!(req.method, "ping");
    }

    #[test]
    fn params_optional_serialize_none() {
        let req = Request::new("method".to_string(), None, Id::Number(1));
        let json = serde_json::to_string(&req).unwrap();
        // params should not be in the JSON when None
        assert!(!json.contains("\"params\""));
    }

    #[test]
    fn response_success_serialize_result_only() {
        let resp = Response::result(Id::Number(1), json!(42));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"result\""));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn response_error_serialize_error_only() {
        let err = Error::internal_error();
        let resp = Response::error(Id::Number(1), err);
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"error\""));
        assert!(!json.contains("\"result\""));
    }
}
