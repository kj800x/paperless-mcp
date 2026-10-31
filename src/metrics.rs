use prometheus::{Encoder, IntCounterVec, Opts, Registry, TextEncoder};

pub struct Metrics {
    registry: Registry,
    tool_calls: IntCounterVec,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let tool_calls = IntCounterVec::new(
            Opts::new(
                "paperless_mcp_tool_calls_total",
                "MCP tool calls by tool and outcome",
            ),
            &["tool", "outcome"],
        )
        .expect("Failed to create tool_calls metric");
        registry
            .register(Box::new(tool_calls.clone()))
            .expect("Failed to register tool_calls metric");
        Self {
            registry,
            tool_calls,
        }
    }

    pub fn record_tool_call(&self, tool: &str, ok: bool) {
        // Unknown tool names come from clients; don't let them create unbounded series.
        let tool = if crate::tools::is_known(tool) {
            tool
        } else {
            "unknown"
        };
        let outcome = if ok { "ok" } else { "error" };
        self.tool_calls.with_label_values(&[tool, outcome]).inc();
    }

    pub fn render(&self) -> String {
        let mut buffer = Vec::new();
        TextEncoder::new()
            .encode(&self.registry.gather(), &mut buffer)
            .expect("Failed to encode metrics");
        String::from_utf8(buffer).unwrap_or_default()
    }
}
