//! Prints LiteGen's MCP tool reference, the content of `docs/mcp/tools.md`.
//!
//! From `litegen-core/`: `cargo run --example mcp_tools_doc > ../docs/mcp/tools.md`.
//! A unit test (`mcp::tests::tools_reference_is_current`) fails when the
//! committed file differs from what this prints.

fn main() {
    let registry = litegen::mcp::tool_registry();
    print!("{}", litegen::mcp::registry::render_reference(&registry));
}
