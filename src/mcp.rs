//! MCP (Model Context Protocol) server over Streamable HTTP, mounted at /mcp.
//!
//! Exposes plocate file search as two MCP tools — `search_files` and `glob` —
//! so AI agents can query the indexed tree the same way REST clients do. Shares
//! the single AppState (engine, concurrency cap, timeouts) and the same input
//! validation as the REST layer.

use rmcp::{
    ServerHandler,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, ErrorData, Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;

use crate::dto::SearchResponse;
use crate::limits::{validate_offset, validate_query};
use crate::state::{AppState, SearchOrigin};

pub struct PlocateMcpHandler {
    state: AppState,
}

impl PlocateMcpHandler {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchFilesParams {
    /// Substring to match in file paths. If it contains glob metacharacters
    /// (`*` `?` `[`) plocate treats it as a glob.
    pub query: String,
    /// Maximum results (default 100, server-capped).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Pagination offset (0-based, max 10000).
    #[serde(default)]
    pub offset: Option<usize>,
    /// Case-insensitive match (default true).
    #[serde(default = "default_true")]
    pub case_insensitive: bool,
    /// `path` (default, match the full path) or `basename`.
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GlobParams {
    /// Glob pattern, e.g. `*.rs` or `**/2024/*.log`.
    pub pattern: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
    /// Case-insensitive match (default true).
    #[serde(default = "default_true")]
    pub case_insensitive: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FuzzySearchParams {
    /// Query string. Whitespace-separated tokens are matched with AND
    /// semantics (every token must appear in the path); results are ranked
    /// by fzf-style fuzzy relevance.
    pub query: String,
    /// Maximum results (default 100, server-capped).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Pagination offset (0-based, max 10000).
    #[serde(default)]
    pub offset: Option<usize>,
    /// Case-insensitive match (default true).
    #[serde(default = "default_true")]
    pub case_insensitive: bool,
}

#[tool_router]
impl PlocateMcpHandler {
    #[tool(
        name = "search_files",
        description = "Search indexed file paths by substring (or by glob when the query contains *, ? or []). Returns one match per line: a fully-qualified browseable URL when a file-server base is configured on the server, otherwise the relative path. The index is refreshed periodically; very recent files may not yet appear."
    )]
    async fn search_files(
        &self,
        Parameters(p): Parameters<SearchFilesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Err(e) = validate_query(&p.query) {
            return Ok(err_text(&e.to_string()));
        }
        let offset = match validate_offset(p.offset) {
            Ok(n) => n,
            Err(e) => return Ok(err_text(&e.to_string())),
        };
        let limit = p
            .limit
            .unwrap_or(self.state.max_results)
            .clamp(1, self.state.max_results.max(1));
        let basename_only = matches!(
            p.scope.as_deref(),
            Some("basename") | Some("b") | Some("name")
        );
        match self
            .state
            .search(
                &p.query,
                limit,
                offset,
                p.case_insensitive,
                basename_only,
                SearchOrigin::Mcp,
            )
            .await
        {
            Ok(resp) => Ok(CallToolResult::success(vec![Content::text(format_search(
                &resp,
                self.state.file_server_url.as_deref(),
            ))])),
            Err(e) => Ok(err_text(&format!("search failed: {e}"))),
        }
    }

    #[tool(
        name = "glob",
        description = "Search indexed file paths by a glob pattern (e.g. *.rs, **/2024/*.log). Returns one match per line: a fully-qualified browseable URL when a file-server base is configured on the server, otherwise the relative path."
    )]
    async fn glob(
        &self,
        Parameters(p): Parameters<GlobParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Err(e) = validate_query(&p.pattern) {
            return Ok(err_text(&e.to_string()));
        }
        let offset = match validate_offset(p.offset) {
            Ok(n) => n,
            Err(e) => return Ok(err_text(&e.to_string())),
        };
        let limit = p
            .limit
            .unwrap_or(self.state.max_results)
            .clamp(1, self.state.max_results.max(1));
        match self
            .state
            .search(
                &p.pattern,
                limit,
                offset,
                p.case_insensitive,
                false,
                SearchOrigin::Mcp,
            )
            .await
        {
            Ok(resp) => Ok(CallToolResult::success(vec![Content::text(format_search(
                &resp,
                self.state.file_server_url.as_deref(),
            ))])),
            Err(e) => Ok(err_text(&format!("glob failed: {e}"))),
        }
    }

    #[tool(
        name = "fuzzy_search",
        description = "Fuzzy multi-keyword search over indexed file paths. Whitespace-separated tokens are AND-ed (every token must appear), then ranked by fzf-style relevance (nucleo). Best for queries like 'zookeeper rpm oe1' where substring search would return nothing. Returns one match per line, ranked by relevance: a fully-qualified browseable URL when a file-server base is configured on the server, otherwise the relative path."
    )]
    async fn fuzzy_search(
        &self,
        Parameters(p): Parameters<FuzzySearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Err(e) = validate_query(&p.query) {
            return Ok(err_text(&e.to_string()));
        }
        let offset = match validate_offset(p.offset) {
            Ok(n) => n,
            Err(e) => return Ok(err_text(&e.to_string())),
        };
        let limit = p
            .limit
            .unwrap_or(self.state.max_results)
            .clamp(1, self.state.max_results.max(1));
        match self
            .state
            .search_fuzzy(
                &p.query,
                limit,
                offset,
                p.case_insensitive,
                SearchOrigin::Mcp,
            )
            .await
        {
            Ok(resp) => Ok(CallToolResult::success(vec![Content::text(format_search(
                &resp,
                self.state.file_server_url.as_deref(),
            ))])),
            Err(e) => Ok(err_text(&format!("fuzzy search failed: {e}"))),
        }
    }
}

#[tool_handler]
impl ServerHandler for PlocateMcpHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                String::from("plocate-server"),
                String::from(env!("CARGO_PKG_VERSION")),
            ))
            .with_instructions(String::from(
                "Filename/path search over a large indexed tree (RPMs, ISOs, build \
                 artifacts, source trees, etc.) via plocate. Sub-millisecond even at \
                 10M+ paths — prefer search_files/glob here over a scanning glob/grep \
                 tool, which would walk the filesystem. Matches paths only, NOT file \
                 contents. Covers the configured index root and below; nothing outside \
                 that root is reachable. Each match is returned on its own line as \
                 either a fully-qualified browseable URL (when the server has a \
                 file-server base configured) or a relative path; results may lag \
                 very recent filesystem changes until the next reindex. Use \
                 search_files for substring or glob queries, fuzzy_search for \
                 multi-keyword ranked matches (e.g. 'kernel x86 rpm'), glob for an \
                 explicit glob pattern.",
            ))
    }
}

/// Format search results as agent-friendly text. When `file_server_base` is
/// `Some`, each result is rendered as a fully-qualified, browseable URL by
/// appending its (percent-encoded) relative path to the configured external
/// file-server base; otherwise relative paths are emitted as-is.
fn format_search(resp: &SearchResponse, file_server_base: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} match(es){}\n",
        resp.total_matched,
        if resp.truncated {
            " (truncated, more exist)"
        } else {
            ""
        }
    ));
    for it in &resp.items {
        let is_dir = it.kind == "directory";
        match file_server_base {
            Some(base) => out.push_str(&build_file_url(base, &it.relative_path, is_dir)),
            None => {
                out.push_str(&it.relative_path);
                if is_dir {
                    out.push('/');
                }
            }
        }
        out.push('\n');
    }
    out.trim_end().to_owned()
}

/// Build a browseable URL for a result by appending its relative path to an
/// external file-server base. Mirrors the frontend `buildBrowseUrl` rule
/// (`web/src/api.ts`): strip trailing slashes from the base, percent-encode
/// each path segment (preserving `/` separators), and add a trailing slash
/// for directories so dufs/caddy/nginx render a listing.
fn build_file_url(base: &str, relative_path: &str, is_dir: bool) -> String {
    let clean_base = base.trim_end_matches('/');
    let encoded = relative_path
        .split('/')
        .map(percent_encode_segment)
        .collect::<Vec<_>>()
        .join("/");
    let mut url = format!("{clean_base}/{encoded}");
    if is_dir {
        url.push('/');
    }
    url
}

/// Percent-encode a single path segment, equivalent to JavaScript's
/// `encodeURIComponent`. Unreserved per RFC 3986 plus the RFC 3986 sub-delims
/// that `encodeURIComponent` leaves alone: `! ' ( ) *`. Everything else is
/// encoded as `%XX`.
fn percent_encode_segment(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'!'
            | b'\''
            | b'('
            | b')'
            | b'*' => out.push(b as char),
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

fn err_text(msg: &str) -> CallToolResult {
    CallToolResult::error(vec![Content::text(msg.to_owned())])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{FileItemDto, SearchResponse};

    fn item(kind: &str, rel: &str) -> FileItemDto {
        FileItemDto {
            kind: kind.to_owned(),
            name: String::new(),
            relative_path: rel.to_owned(),
            absolute_path: String::new(),
            score: None,
        }
    }

    fn resp(items: Vec<FileItemDto>) -> SearchResponse {
        SearchResponse {
            total_matched: items.len(),
            truncated: false,
            items,
            elapsed_ms: 0.0,
        }
    }

    #[test]
    fn format_without_file_server_emits_relative_paths() {
        let r = resp(vec![
            item("file", "src/main.rs"),
            item("directory", "web/dist"),
        ]);
        let out = format_search(&r, None);
        assert_eq!(out, "2 match(es)\nsrc/main.rs\nweb/dist/");
    }

    #[test]
    fn format_with_file_server_emits_urls() {
        let r = resp(vec![
            item("file", "src/main.rs"),
            item("directory", "web/dist"),
        ]);
        let out = format_search(&r, Some("https://files.example.com"));
        assert_eq!(
            out,
            "2 match(es)\nhttps://files.example.com/src/main.rs\nhttps://files.example.com/web/dist/"
        );
    }

    #[test]
    fn build_file_url_strips_trailing_slash_from_base() {
        assert_eq!(build_file_url("https://x/", "a/b", false), "https://x/a/b");
    }

    #[test]
    fn build_file_url_encodes_segments() {
        assert_eq!(
            build_file_url("https://x", "docs/read me.md", false),
            "https://x/docs/read%20me.md"
        );
        assert_eq!(
            build_file_url("https://x", "中文/目录", false),
            "https://x/%E4%B8%AD%E6%96%87/%E7%9B%AE%E5%BD%95"
        );
    }

    #[test]
    fn build_file_url_directory_trailing_slash() {
        assert_eq!(build_file_url("https://x", "a/b", true), "https://x/a/b/");
    }

    #[test]
    fn percent_encode_matches_encode_uri_component() {
        // Unreserved + encodeURIComponent-safe stay literal.
        assert_eq!(percent_encode_segment("Az09-_.~!*'()"), "Az09-_.~!*'()");
        // Space -> %20, not '+'.
        assert_eq!(percent_encode_segment("a b"), "a%20b");
        // Reserved characters get encoded.
        assert_eq!(
            percent_encode_segment("a/b?c=d#e&f"),
            "a%2Fb%3Fc%3Dd%23e%26f"
        );
    }
}
