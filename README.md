# paperless-mcp

A read-only [MCP](https://modelcontextprotocol.io) server (Streamable HTTP) that lets agents
search and read documents in a [Paperless-ngx](https://docs.paperless-ngx.com) instance.

## Authentication

The server holds no credentials. Each client sends its own Paperless API token, which is
forwarded to Paperless on every request, so Paperless's own permissions decide what the agent
can see. `Authorization: Token <token>` and `Authorization: Bearer <token>` are both accepted.

## Installing on a machine (Claude Code)

```sh
claude mcp add --scope user --transport http paperless https://paperless-mcp.home.coolkev.com/mcp \
  --header "Authorization: Token <paperless api token>"
```

## Tools

| Tool | Purpose |
|---|---|
| `search_documents` | Full-text query plus tag / correspondent / type / date filters, paginated |
| `get_document` | Metadata, notes and OCR text (chunked via `offset` / `max_chars`) |
| `find_similar_documents` | Documents similar to a given one |
| `list_tags`, `list_correspondents`, `list_document_types` | Discover filter values |

Filters take names (case-insensitive) or ids. Tag/correspondent/type lookups are cached per
token for 5 minutes.

## Configuration

| Env var | Default |
|---|---|
| `PAPERLESS_URL` | `http://webserver.paperless.svc.cluster.local:8000` |
| `PAPERLESS_PUBLIC_URL` | `https://paperless.home.coolkev.com` (used for document links) |
| `PORT` | `8080` |

Also serves `GET /healthz` and Prometheus metrics at `GET /metrics`.
