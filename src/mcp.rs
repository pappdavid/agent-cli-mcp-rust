use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::errors::{AgentCliError, ErrorCode};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

pub type ToolHandler = Arc<
    dyn Fn(
            serde_json::Value,
        )
            -> Pin<Box<dyn Future<Output = Result<serde_json::Value, AgentCliError>> + Send>>
        + Send
        + Sync,
>;

pub type ResourceHandler = Arc<
    dyn Fn(
            String, // URI
            HashMap<String, String>, // Params
        )
            -> Pin<Box<dyn Future<Output = Result<serde_json::Value, AgentCliError>> + Send>>
        + Send
        + Sync,
>;

#[derive(Clone)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub handler: ToolHandler,
}

#[derive(Clone)]
pub struct McpResource {
    pub name: String,
    pub uri_pattern: String,
    pub title: String,
    pub description: String,
    pub mime_type: String,
    pub handler: ResourceHandler,
}

pub struct McpServer {
    name: String,
    version: String,
    instructions: String,
    tools: Arc<Mutex<HashMap<String, McpTool>>>,
    resources: Arc<Mutex<HashMap<String, McpResource>>>,
    stdout_tx: mpsc::UnboundedSender<String>,
    stdout_rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<String>>>,
}

impl McpServer {
    pub fn new(name: &str, version: &str, instructions: &str) -> Self {
        let (stdout_tx, stdout_rx) = mpsc::unbounded_channel();
        McpServer {
            name: name.to_string(),
            version: version.to_string(),
            instructions: instructions.to_string(),
            tools: Arc::new(Mutex::new(HashMap::new())),
            resources: Arc::new(Mutex::new(HashMap::new())),
            stdout_tx,
            stdout_rx: Arc::new(tokio::sync::Mutex::new(stdout_rx)),
        }
    }

    pub fn register_tool<F, Fut>(&self, name: &str, description: &str, schema: serde_json::Value, handler: F)
    where
        F: Fn(serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value, AgentCliError>> + Send + 'static,
    {
        let wrapped_handler = Arc::new(move |args| {
            let fut = handler(args);
            let boxed: Pin<Box<dyn Future<Output = Result<serde_json::Value, AgentCliError>> + Send>> =
                Box::pin(fut);
            boxed
        });

        self.tools.lock().unwrap().insert(
            name.to_string(),
            McpTool {
                name: name.to_string(),
                description: description.to_string(),
                input_schema: schema,
                handler: wrapped_handler,
            },
        );
    }

    pub fn register_resource<F, Fut>(&self, name: &str, uri_pattern: &str, title: &str, description: &str, mime_type: &str, handler: F)
    where
        F: Fn(String, HashMap<String, String>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value, AgentCliError>> + Send + 'static,
    {
        let wrapped_handler = Arc::new(move |uri, params| {
            let fut = handler(uri, params);
            let boxed: Pin<Box<dyn Future<Output = Result<serde_json::Value, AgentCliError>> + Send>> =
                Box::pin(fut);
            boxed
        });

        self.resources.lock().unwrap().insert(
            uri_pattern.to_string(),
            McpResource {
                name: name.to_string(),
                uri_pattern: uri_pattern.to_string(),
                title: title.to_string(),
                description: description.to_string(),
                mime_type: mime_type.to_string(),
                handler: wrapped_handler,
            },
        );
    }

    pub fn get_tools_list(&self) -> serde_json::Value {
        let tools_map = self.tools.lock().unwrap();
        let tools_list: Vec<serde_json::Value> = tools_map
            .values()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.input_schema,
                })
            })
            .collect();

        serde_json::json!({ "tools": tools_list })
    }

    pub fn get_resources_list(&self) -> serde_json::Value {
        let res_map = self.resources.lock().unwrap();
        let res_list: Vec<serde_json::Value> = res_map
            .values()
            .map(|r| {
                serde_json::json!({
                    "name": r.name,
                    "uri": r.uri_pattern,
                    "title": r.title,
                    "description": r.description,
                    "mimeType": r.mime_type,
                })
            })
            .collect();

        serde_json::json!({ "resources": res_list })
    }

    pub fn send_notification(&self, method: &str, params: serde_json::Value) {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: Some(params),
        };
        if let Ok(serialized) = serde_json::to_string(&notif) {
            let _ = self.stdout_tx.send(serialized);
        }
    }

    pub async fn run(&self) -> io::Result<()> {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin).lines();
        let mut stdout = io::stdout();

        let stdout_rx = self.stdout_rx.clone();
        let tx = self.stdout_tx.clone();

        // Spawn a background task to write outgoing queue to stdout
        tokio::spawn(async move {
            let mut rx = stdout_rx.lock().await;
            while let Some(msg) = rx.recv().await {
                let mut out = io::stdout();
                let _ = out.write_all(msg.as_bytes()).await;
                let _ = out.write_all(b"\n").await;
                let _ = out.flush().await;
            }
        });

        loop {
            tokio::select! {
                line_opt = reader.next_line() => {
                    match line_opt {
                        Ok(Some(line)) => {
                            if line.trim().is_empty() {
                                continue;
                            }
                            let tx_clone = tx.clone();
                            let tools = self.tools.clone();
                            let resources = self.resources.clone();
                            let name = self.name.clone();
                            let version = self.version.clone();
                            let instructions = self.instructions.clone();

                            tokio::spawn(async move {
                                if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(&line) {
                                    let resp = handle_request(req, tools, resources, &name, &version, &instructions).await;
                                    if let Ok(serialized) = serde_json::to_string(&resp) {
                                        let _ = tx_clone.send(serialized);
                                    }
                                } else if let Ok(notif) = serde_json::from_str::<JsonRpcNotification>(&line) {
                                    // Process notifications (e.g. initialized)
                                    handle_notification(notif).await;
                                } else {
                                    // Invalid JSON-RPC message
                                    let err_resp = JsonRpcResponse {
                                        jsonrpc: "2.0".to_string(),
                                        id: serde_json::Value::Null,
                                        result: None,
                                        error: Some(JsonRpcError {
                                            code: -32600, // Invalid Request
                                            message: "Invalid JSON-RPC request".to_string(),
                                            data: None,
                                        }),
                                    };
                                    if let Ok(serialized) = serde_json::to_string(&err_resp) {
                                        let _ = tx_clone.send(serialized);
                                    }
                                }
                            });
                        }
                        Ok(None) => {
                            // EOF on stdin - shut down
                            break;
                        }
                        Err(_) => {
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

async fn handle_request(
    req: JsonRpcRequest,
    tools: Arc<Mutex<HashMap<String, McpTool>>>,
    resources: Arc<Mutex<HashMap<String, McpResource>>>,
    server_name: &str,
    server_version: &str,
    server_instructions: &str,
) -> JsonRpcResponse {
    match req.method.as_str() {
        "initialize" => {
            let result = serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {},
                    "resources": {},
                },
                "serverInfo": {
                    "name": server_name,
                    "version": server_version,
                },
                "instructions": server_instructions,
            });
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(result),
                error: None,
            }
        }
        "tools/list" => {
            let tools_map = tools.lock().unwrap();
            let tools_list: Vec<serde_json::Value> = tools_map
                .values()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.input_schema,
                    })
                })
                .collect();

            let result = serde_json::json!({ "tools": tools_list });
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(result),
                error: None,
            }
        }
        "tools/call" => {
            let tool_name = req.params.as_ref()
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("");

            let tool_args = req.params.as_ref()
                .and_then(|p| p.get("arguments"))
                .cloned()
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

            let handler_opt = tools.lock().unwrap().get(tool_name).cloned();
            if let Some(tool) = handler_opt {
                match (tool.handler)(tool_args).await {
                    Ok(res) => JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: req.id,
                        result: Some(res),
                        error: None,
                    },
                    Err(e) => JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: req.id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32603, // Internal Error
                            message: format!("{}", e),
                            data: Some(serde_json::to_value(e.to_response()).unwrap_or_default()),
                        }),
                    },
                }
            } else {
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32601, // Method not found
                        message: format!("Tool not found: {}", tool_name),
                        data: None,
                    }),
                }
            }
        }
        "resources/list" => {
            let res_map = resources.lock().unwrap();
            let res_list: Vec<serde_json::Value> = res_map
                .values()
                .map(|r| {
                    serde_json::json!({
                        "name": r.name,
                        "uri": r.uri_pattern,
                        "title": r.title,
                        "description": r.description,
                        "mimeType": r.mime_type,
                    })
                })
                .collect();

            let result = serde_json::json!({ "resources": res_list });
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(result),
                error: None,
            }
        }
        "resources/read" => {
            let uri = req.params.as_ref()
                .and_then(|p| p.get("uri"))
                .and_then(|u| u.as_str())
                .unwrap_or("");

            // Find matching resource pattern
            let mut match_pattern = None;
            let mut params = HashMap::new();

            {
                let res_map = resources.lock().unwrap();
                for r in res_map.values() {
                    if let Some(matched_params) = match_uri_pattern(&r.uri_pattern, uri) {
                        match_pattern = Some(r.clone());
                        params = matched_params;
                        break;
                    }
                }
            }

            if let Some(res) = match_pattern {
                match (res.handler)(uri.to_string(), params).await {
                    Ok(res_val) => JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: req.id,
                        result: Some(res_val),
                        error: None,
                    },
                    Err(e) => JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: req.id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32603,
                            message: format!("{}", e),
                            data: Some(serde_json::to_value(e.to_response()).unwrap_or_default()),
                        }),
                    },
                }
            } else {
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32601,
                        message: format!("Resource not found: {}", uri),
                        data: None,
                    }),
                }
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("Method not found: {}", req.method),
                data: None,
            }),
        },
    }
}

async fn handle_notification(_notif: JsonRpcNotification) {
    // Standard notifications like initialized do not require response.
}

fn match_uri_pattern(pattern: &str, uri: &str) -> Option<HashMap<String, String>> {
    // Simplistic URI template matcher for templates like:
    // - jules://sessions
    // - jules://session/{sessionId}
    // - jules://session/{sessionId}/activities
    let p_parts: Vec<&str> = pattern.split('/').collect();
    let u_parts: Vec<&str> = uri.split('/').collect();

    if p_parts.len() != u_parts.len() {
        return None;
    }

    let mut params = HashMap::new();
    for i in 0..p_parts.len() {
        let p = p_parts[i];
        let u = u_parts[i];

        if p.starts_with('{') && p.ends_with('}') {
            let key = &p[1..p.len() - 1];
            params.insert(key.to_string(), u.to_string());
        } else if p != u {
            return None;
        }
    }

    Some(params)
}
