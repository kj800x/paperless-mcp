use actix_web::{HttpRequest, HttpResponse, web};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::metrics::Metrics;
use crate::paperless::{Paperless, PaperlessError};
use crate::tools;

const PROTOCOL_VERSION: &str = "2025-03-26";

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

fn success(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Option<Value>, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

pub fn tool_text(text: String, is_error: bool) -> Value {
    let mut result = json!({ "content": [{ "type": "text", "text": text }] });
    if is_error {
        result["isError"] = json!(true);
    }
    result
}

fn unauthorized(message: &str) -> HttpResponse {
    HttpResponse::Unauthorized()
        .content_type("application/json")
        .json(error(None, -32001, message))
}

/// Pulls the caller's Paperless token from the request. `Bearer` is accepted for
/// clients that can't send other schemes and is rewritten to Paperless's `Token`.
fn paperless_auth(req: &HttpRequest) -> Option<String> {
    let header = req.headers().get("Authorization")?.to_str().ok()?.trim();
    let (scheme, credential) = header.split_once(' ')?;
    let credential = credential.trim();
    if credential.is_empty() {
        return None;
    }
    if scheme.eq_ignore_ascii_case("token") || scheme.eq_ignore_ascii_case("bearer") {
        Some(format!("Token {}", credential))
    } else {
        None
    }
}

pub async fn handle_mcp(
    req: HttpRequest,
    body: web::Json<JsonRpcRequest>,
    paperless: web::Data<Paperless>,
    metrics: web::Data<Metrics>,
) -> HttpResponse {
    let request = body.into_inner();

    let Some(auth) = paperless_auth(&req) else {
        return unauthorized(
            "Missing Paperless API token. Configure this MCP server with the header \
             `Authorization: Token <paperless api token>`.",
        );
    };

    // Notifications (no id) don't get responses
    if request.id.is_none() {
        return HttpResponse::Accepted().finish();
    }

    let response = match request.method.as_str() {
        "initialize" => {
            // Validate the token up front so misconfiguration surfaces at connect time.
            match paperless.lookups(&auth).await {
                Ok(lookups) => {
                    log::info!(
                        "MCP session initialized for Paperless user {}",
                        lookups.username
                    );
                    success(
                        request.id,
                        json!({
                            "protocolVersion": PROTOCOL_VERSION,
                            "capabilities": { "tools": {} },
                            "serverInfo": {
                                "name": "paperless-mcp",
                                "version": env!("CARGO_PKG_VERSION"),
                            },
                            "instructions": tools::SERVER_INSTRUCTIONS,
                        }),
                    )
                }
                Err(PaperlessError::Unauthorized) => {
                    return unauthorized("Paperless rejected the API token.");
                }
                Err(e) => error(request.id, -32603, &e.to_string()),
            }
        }
        "ping" => success(request.id, json!({})),
        "tools/list" => success(request.id, json!({ "tools": tools::tool_definitions() })),
        "tools/call" => {
            let params = request.params.unwrap_or(json!({}));
            match params.get("name").and_then(|v| v.as_str()) {
                Some(name) => {
                    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                    let result = tools::dispatch(name, &arguments, &auth, &paperless).await;
                    metrics.record_tool_call(name, result.is_ok());
                    let result = match result {
                        Ok(text) => tool_text(text, false),
                        Err(text) => {
                            log::warn!("Tool {} failed: {}", name, text);
                            tool_text(text, true)
                        }
                    };
                    success(request.id, result)
                }
                None => error(request.id, -32602, "Missing tool name"),
            }
        }
        _ => error(request.id, -32601, "Method not found"),
    };

    HttpResponse::Ok()
        .content_type("application/json")
        .json(response)
}
