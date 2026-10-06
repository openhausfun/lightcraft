//! LightCraft's MCP server.
//!
//! [Model Context Protocol](https://modelcontextprotocol.io) over stdio: newline-delimited
//! JSON-RPC 2.0, hand-written (no async runtime). The server exposes LightCraft as MCP tools and
//! resources and forwards everything to a [`Backend`]:
//!
//! - [`Remote`] talks to a running desktop app through its loopback JSON-lines control channel
//!   (`lightcraft --control 7980`): one `{"id","method","params"}` line in, one
//!   `{"id","ok","result"|"error"}` line out (see `docs/control-protocol.md`).
//! - [`Headless`] hosts an in-process [`lightcraft_engine::Session`] and answers the same
//!   control-channel method names itself (rendering with the engine's pipeline and encoding with
//!   `lightcraft-codecs`), so agents can develop photos and look at the result without a window.
//!
//! Entry points: [`Server::serve`] (stdio loop) and [`Server::handle_line`] (one message).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod backend;
mod base64;
mod headless;
mod server;
mod tools;

pub use backend::{Backend, Remote};
pub use base64::{base64_decode, base64_encode};
pub use headless::{Headless, PHOTO_EXTENSIONS, encode_image, expand_paths, write_image};
pub use server::{PROTOCOL_VERSION, Server};
pub use tools::{COMMAND_TOOL_PREFIX, ToolResult, call_tool, command_tool_name, helper_tools, tool_definitions};

/// Default control-channel address of the desktop app (`lightcraft --control 7980`).
pub const DEFAULT_ADDR: &str = "127.0.0.1:7980";

#[cfg(test)]
mod tests;
