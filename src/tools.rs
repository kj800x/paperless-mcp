use serde_json::{Value, json};

use crate::paperless::{NamedObject, Page, Paperless, PaperlessError, truncate};

pub const SERVER_INSTRUCTIONS: &str = "Access to a Paperless-ngx document archive \
(scanned mail, receipts, statements, tax forms, manuals, etc.). Start with search_documents; \
use list_tags / list_correspondents / list_document_types to discover filter values. \
get_document returns OCR text, which may be imperfect. update_document changes a document's \
title, date, correspondent, type and tags (existing values only); create_tag / \
create_correspondent add new ones first if needed; add_note attaches a note. \
Nothing can be deleted or uploaded.";

const DEFAULT_PAGE_SIZE: i64 = 10;
const MAX_PAGE_SIZE: i64 = 50;
const DEFAULT_MAX_CHARS: usize = 20_000;
const MAX_MAX_CHARS: usize = 100_000;

const TOOL_NAMES: &[&str] = &[
    "search_documents",
    "get_document",
    "find_similar_documents",
    "list_tags",
    "list_correspondents",
    "list_document_types",
    "update_document",
    "add_note",
    "create_tag",
    "create_correspondent",
];

/// Paperless's "Auto" matching: the classifier learns from documents the object
/// is assigned to. Same default the Paperless UI uses for new objects.
const MATCHING_AUTO: i64 = 6;

pub fn is_known(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

pub fn tool_definitions() -> Value {
    let mut tools = read_tool_definitions();
    let read_only = json!({ "readOnlyHint": true, "openWorldHint": false });
    for tool in tools.as_array_mut().into_iter().flatten() {
        tool["annotations"] = read_only.clone();
    }
    let write = json!({
        "readOnlyHint": false,
        "destructiveHint": false,
        "idempotentHint": false,
        "openWorldHint": false
    });
    for mut tool in write_tool_definitions() {
        tool["annotations"] = write.clone();
        tools.as_array_mut().unwrap().push(tool);
    }
    tools
}

fn write_tool_definitions() -> Vec<Value> {
    let name_or_id = "Name (case-insensitive) or numeric id; must already exist";
    vec![
        json!({
            "name": "update_document",
            "description": "Change a document's metadata. Only the fields you pass are changed. \
                Tags, correspondents and document types must already exist (see list_tags etc.); \
                nothing is created. Returns the updated document summary.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer" },
                    "title": { "type": "string" },
                    "created": { "type": "string", "description": "Document date, YYYY-MM-DD" },
                    "correspondent": { "type": "string", "description": name_or_id },
                    "document_type": { "type": "string", "description": name_or_id },
                    "add_tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": format!("Tags to add. {}", name_or_id)
                    },
                    "remove_tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": format!("Tags to remove. {}", name_or_id)
                    }
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "add_note",
            "description": "Add a note to a document. Notes are visible in Paperless and \
                included in full-text search.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer" },
                    "note": { "type": "string" }
                },
                "required": ["id", "note"]
            }
        }),
        json!({
            "name": "create_tag",
            "description": "Create a new tag. Check list_tags first; if a tag with the same name \
                (case-insensitive) already exists, it is returned instead of creating a duplicate. \
                Uses Paperless's Auto matching. Returns the tag's id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "color": { "type": "string", "description": "Hex color, e.g. #a6cee3. Default: Paperless picks one" },
                    "is_inbox_tag": { "type": "boolean", "description": "Auto-apply to newly consumed documents. Default false" }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "create_correspondent",
            "description": "Create a new correspondent (sender/organization). Check \
                list_correspondents first; if one with the same name (case-insensitive) already \
                exists, it is returned instead of creating a duplicate. Uses Paperless's Auto \
                matching. Returns the correspondent's id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                },
                "required": ["name"]
            }
        }),
    ]
}

fn read_tool_definitions() -> Value {
    let name_or_id = "Name (case-insensitive) or numeric id";
    json!([
        {
            "name": "search_documents",
            "description": "Search documents. `query` is full-text search over OCR content, titles, \
                tags, correspondents and notes (Whoosh syntax: AND/OR/NOT, \"exact phrase\", \
                prefix*). Combine with structured filters, or omit `query` to browse by filter. \
                Returns id, date, title, metadata and a snippet per hit.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Full-text query" },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": format!("Documents must have ALL of these tags. {}", name_or_id)
                    },
                    "correspondent": { "type": "string", "description": name_or_id },
                    "document_type": { "type": "string", "description": name_or_id },
                    "created_after": { "type": "string", "description": "YYYY-MM-DD, inclusive" },
                    "created_before": { "type": "string", "description": "YYYY-MM-DD, inclusive" },
                    "sort": {
                        "type": "string",
                        "enum": ["relevance", "newest", "oldest"],
                        "description": "Default: relevance when `query` is set, otherwise newest"
                    },
                    "page": { "type": "integer", "minimum": 1, "description": "Default 1" },
                    "page_size": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_PAGE_SIZE,
                        "description": format!("Default {}", DEFAULT_PAGE_SIZE)
                    }
                }
            }
        },
        {
            "name": "get_document",
            "description": "Get a document's metadata, notes and OCR text content. Long content is \
                returned in chunks; use `offset` to continue.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer" },
                    "offset": { "type": "integer", "minimum": 0, "description": "Character offset into the content. Default 0" },
                    "max_chars": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": MAX_MAX_CHARS,
                        "description": format!("Max content characters to return. Default {}; 0 for metadata only", DEFAULT_MAX_CHARS)
                    }
                },
                "required": ["id"]
            }
        },
        {
            "name": "find_similar_documents",
            "description": "Find documents with content similar to the given document.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer" },
                    "page_size": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_PAGE_SIZE,
                        "description": format!("Default {}", DEFAULT_PAGE_SIZE)
                    }
                },
                "required": ["id"]
            }
        },
        {
            "name": "list_tags",
            "description": "List all tags with document counts.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "list_correspondents",
            "description": "List all correspondents (senders/organizations) with document counts.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "list_document_types",
            "description": "List all document types with document counts.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

pub async fn dispatch(
    name: &str,
    args: &Value,
    auth: &str,
    paperless: &Paperless,
) -> Result<String, String> {
    let result = match name {
        "search_documents" => search_documents(args, auth, paperless).await,
        "get_document" => get_document(args, auth, paperless).await,
        "find_similar_documents" => find_similar(args, auth, paperless).await,
        "list_tags" => list_named(auth, paperless, "tags", |l| &l.tags).await,
        "list_correspondents" => {
            list_named(auth, paperless, "correspondents", |l| &l.correspondents).await
        }
        "list_document_types" => {
            list_named(auth, paperless, "document types", |l| &l.document_types).await
        }
        "update_document" => update_document(args, auth, paperless).await,
        "add_note" => add_note(args, auth, paperless).await,
        "create_tag" => create_tag(args, auth, paperless).await,
        "create_correspondent" => create_correspondent(args, auth, paperless).await,
        _ => return Err(format!("Unknown tool: {}", name)),
    };
    result.map_err(|e| match e {
        ToolError::Paperless(PaperlessError::NotFound) => {
            "Not found (or not visible to this Paperless user)".to_string()
        }
        ToolError::Paperless(e) => e.to_string(),
        ToolError::Invalid(msg) => msg,
    })
}

#[derive(Debug)]
enum ToolError {
    Paperless(PaperlessError),
    Invalid(String),
}

impl From<PaperlessError> for ToolError {
    fn from(e: PaperlessError) -> Self {
        ToolError::Paperless(e)
    }
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn arg_int(args: &Value, key: &str) -> Result<Option<i64>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_i64()
            .map(Some)
            .ok_or_else(|| ToolError::Invalid(format!("`{}` must be an integer", key))),
        // Some clients send numbers as strings.
        Some(Value::String(s)) => s
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| ToolError::Invalid(format!("`{}` must be an integer", key))),
        Some(_) => Err(ToolError::Invalid(format!("`{}` must be an integer", key))),
    }
}

fn arg_date(args: &Value, key: &str) -> Result<Option<String>, ToolError> {
    let Some(value) = arg_str(args, key) else {
        return Ok(None);
    };
    let bytes = value.as_bytes();
    let valid = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
    if valid {
        Ok(Some(value.to_string()))
    } else {
        Err(ToolError::Invalid(format!("`{}` must be YYYY-MM-DD", key)))
    }
}

/// Resolves a user-supplied name or id against a lookup table.
fn resolve(objects: &[NamedObject], kind: &str, value: &str) -> Result<i64, ToolError> {
    if let Ok(id) = value.parse::<i64>()
        && objects.iter().any(|o| o.id == id)
    {
        return Ok(id);
    }
    if let Some(o) = objects.iter().find(|o| o.name.eq_ignore_ascii_case(value)) {
        return Ok(o.id);
    }
    let needle = value.to_lowercase();
    let mut close: Vec<&str> = objects
        .iter()
        .filter(|o| {
            let name = o.name.to_lowercase();
            name.contains(&needle) || needle.contains(&name)
        })
        .map(|o| o.name.as_str())
        .collect();
    close.truncate(10);
    let hint = if close.is_empty() {
        format!("Use list_{} to see valid values.", kind.replace(' ', "_"))
    } else {
        format!("Did you mean: {}?", close.join(", "))
    };
    Err(ToolError::Invalid(format!(
        "No {} named \"{}\". {}",
        kind.trim_end_matches('s'),
        value,
        hint
    )))
}

fn arg_tag_ids(
    args: &Value,
    key: &str,
    lookups: &crate::paperless::Lookups,
) -> Result<Vec<i64>, ToolError> {
    let names: Vec<String> = match args.get(key) {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                Value::String(s) => s.trim().to_string(),
                other => other.to_string(),
            })
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => vec![s.trim().to_string()],
        Some(_) => return Err(ToolError::Invalid(format!("`{}` must be an array", key))),
    };
    names
        .iter()
        .map(|t| resolve(&lookups.tags, "tags", t))
        .collect()
}

fn page_size(args: &Value) -> Result<i64, ToolError> {
    Ok(arg_int(args, "page_size")?
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE))
}

async fn search_documents(
    args: &Value,
    auth: &str,
    paperless: &Paperless,
) -> Result<String, ToolError> {
    let lookups = paperless.lookups(auth).await?;
    let query_text = arg_str(args, "query");
    let page = arg_int(args, "page")?.unwrap_or(1).max(1);
    let page_size = page_size(args)?;

    let mut query: Vec<(&str, String)> = vec![
        ("page", page.to_string()),
        ("page_size", page_size.to_string()),
        ("truncate_content", "true".to_string()),
    ];

    if let Some(q) = query_text {
        query.push(("query", q.to_string()));
    }

    let tags = arg_tag_ids(args, "tags", &lookups)?;
    if !tags.is_empty() {
        let ids: Vec<String> = tags.iter().map(i64::to_string).collect();
        query.push(("tags__id__all", ids.join(",")));
    }
    if let Some(c) = arg_str(args, "correspondent") {
        let id = resolve(&lookups.correspondents, "correspondents", c)?;
        query.push(("correspondent__id", id.to_string()));
    }
    if let Some(t) = arg_str(args, "document_type") {
        let id = resolve(&lookups.document_types, "document types", t)?;
        query.push(("document_type__id", id.to_string()));
    }
    if let Some(d) = arg_date(args, "created_after")? {
        query.push(("created__gte", d));
    }
    if let Some(d) = arg_date(args, "created_before")? {
        query.push(("created__lte", d));
    }

    let sort = arg_str(args, "sort").unwrap_or(if query_text.is_some() {
        "relevance"
    } else {
        "newest"
    });
    match sort {
        "relevance" if query_text.is_some() => {}
        "relevance" | "newest" => query.push(("ordering", "-created".to_string())),
        "oldest" => query.push(("ordering", "created".to_string())),
        other => {
            return Err(ToolError::Invalid(format!(
                "Unknown sort \"{}\"; use relevance, newest or oldest",
                other
            )));
        }
    }

    let results: Page<Value> = paperless.get(auth, "/api/documents/", &query).await?;
    Ok(format_results(&results, page, page_size, &lookups))
}

async fn find_similar(
    args: &Value,
    auth: &str,
    paperless: &Paperless,
) -> Result<String, ToolError> {
    let id = arg_int(args, "id")?.ok_or_else(|| ToolError::Invalid("`id` is required".into()))?;
    let page_size = page_size(args)?;
    let lookups = paperless.lookups(auth).await?;
    let query = [
        ("more_like_id", id.to_string()),
        ("page_size", page_size.to_string()),
        ("truncate_content", "true".to_string()),
    ];
    let results: Page<Value> = paperless.get(auth, "/api/documents/", &query).await?;
    Ok(format_results(&results, 1, page_size, &lookups))
}

fn format_results(
    results: &Page<Value>,
    page: i64,
    page_size: i64,
    lookups: &crate::paperless::Lookups,
) -> String {
    if results.results.is_empty() {
        return if results.count > 0 {
            format!("No results on page {} ({} total).", page, results.count)
        } else {
            "No matching documents.".to_string()
        };
    }

    let first = (page - 1) * page_size + 1;
    let last = first + results.results.len() as i64 - 1;
    let mut out = format!(
        "{} matching documents (showing {}–{}):\n",
        results.count, first, last
    );

    for doc in &results.results {
        out.push('\n');
        out.push_str(&summary_line(doc, lookups));
        out.push('\n');

        let hit = doc.get("__search_hit__");
        let highlight = hit
            .and_then(|h| h.get("highlights"))
            .and_then(|h| h.as_str())
            .map(clean_highlight)
            .filter(|s| !s.is_empty());
        let note_highlight = hit
            .and_then(|h| h.get("note_highlights"))
            .and_then(|h| h.as_str())
            .map(clean_highlight)
            .filter(|s| !s.is_empty());

        match highlight {
            Some(h) => out.push_str(&format!("  > {}\n", truncate(&h, 400))),
            None => {
                let content = doc.get("content").and_then(|c| c.as_str()).unwrap_or("");
                let content = collapse_whitespace(content);
                if !content.is_empty() {
                    out.push_str(&format!("  > {}\n", truncate(&content, 200)));
                }
            }
        }
        if let Some(n) = note_highlight {
            out.push_str(&format!("  > note: {}\n", truncate(&n, 400)));
        }
    }

    if last < results.count {
        out.push_str(&format!(
            "\n{} more. Use page={} for the next page, or narrow the search.\n",
            results.count - last,
            page + 1
        ));
    }
    out
}

fn summary_line(doc: &Value, lookups: &crate::paperless::Lookups) -> String {
    let id = doc.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
    let title = doc
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("(untitled)");
    let created = date_only(doc.get("created"));

    let mut meta = vec![];
    if let Some(c) = doc.get("correspondent").and_then(|v| v.as_i64()) {
        meta.push(format!("from: {}", lookups.correspondent_name(c)));
    }
    if let Some(t) = doc.get("document_type").and_then(|v| v.as_i64()) {
        meta.push(format!("type: {}", lookups.document_type_name(t)));
    }
    let tags = tag_names(doc, lookups);
    if !tags.is_empty() {
        meta.push(format!("tags: {}", tags.join(", ")));
    }

    let mut line = format!("#{} · {} · {}", id, created, title);
    if !meta.is_empty() {
        line.push_str(&format!("\n  {}", meta.join(" · ")));
    }
    line
}

fn tag_names(doc: &Value, lookups: &crate::paperless::Lookups) -> Vec<String> {
    doc.get("tags")
        .and_then(|v| v.as_array())
        .map(|tags| {
            tags.iter()
                .filter_map(|t| t.as_i64())
                .map(|t| lookups.tag_name(t))
                .collect()
        })
        .unwrap_or_default()
}

fn date_only(value: Option<&Value>) -> String {
    value
        .and_then(|v| v.as_str())
        .map(|s| s.chars().take(10).collect())
        .unwrap_or_else(|| "????-??-??".to_string())
}

async fn get_document(
    args: &Value,
    auth: &str,
    paperless: &Paperless,
) -> Result<String, ToolError> {
    let id = arg_int(args, "id")?.ok_or_else(|| ToolError::Invalid("`id` is required".into()))?;
    let offset = arg_int(args, "offset")?.unwrap_or(0).max(0) as usize;
    let max_chars = arg_int(args, "max_chars")?
        .map(|m| m.max(0) as usize)
        .unwrap_or(DEFAULT_MAX_CHARS)
        .min(MAX_MAX_CHARS);

    let (lookups, doc) = tokio::try_join!(paperless.lookups(auth), paperless.document(auth, id))?;

    let mut out = summary_line(&doc, &lookups);
    out.push('\n');

    let mut fields = vec![];
    if let Some(added) = doc.get("added").and_then(|v| v.as_str()) {
        fields.push(format!(
            "added: {}",
            added.chars().take(10).collect::<String>()
        ));
    }
    if let Some(asn) = doc.get("archive_serial_number").and_then(|v| v.as_i64()) {
        fields.push(format!("ASN: {}", asn));
    }
    if let Some(name) = doc.get("original_file_name").and_then(|v| v.as_str()) {
        fields.push(format!("file: {}", name));
    }
    if let Some(pages) = doc.get("page_count").and_then(|v| v.as_i64()) {
        fields.push(format!("pages: {}", pages));
    }
    if !fields.is_empty() {
        out.push_str(&format!("  {}\n", fields.join(" · ")));
    }

    if let Some(custom) = doc.get("custom_fields").and_then(|v| v.as_array()) {
        for field in custom {
            let Some(field_id) = field.get("field").and_then(|v| v.as_i64()) else {
                continue;
            };
            let value = match field.get("value") {
                None | Some(Value::Null) => continue,
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
            };
            out.push_str(&format!(
                "  {}: {}\n",
                lookups.custom_field_name(field_id),
                value
            ));
        }
    }
    out.push_str(&format!("  url: {}\n", paperless.document_url(id)));

    if let Some(notes) = doc.get("notes").and_then(|v| v.as_array())
        && !notes.is_empty()
    {
        out.push_str("\nNotes:\n");
        for note in notes {
            let text = note.get("note").and_then(|v| v.as_str()).unwrap_or("");
            let created = date_only(note.get("created"));
            let user = note
                .get("user")
                .and_then(|u| u.get("username").or(Some(u)))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            if user.is_empty() {
                out.push_str(&format!("- ({}) {}\n", created, text));
            } else {
                out.push_str(&format!("- ({}, {}) {}\n", created, user, text));
            }
        }
    }

    if max_chars > 0 {
        let content = doc.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let total = content.chars().count();
        if total == 0 {
            out.push_str("\n(No text content.)\n");
        } else if offset >= total {
            out.push_str(&format!(
                "\n(offset {} is past the end of the content, which is {} characters.)\n",
                offset, total
            ));
        } else {
            let chunk: String = content.chars().skip(offset).take(max_chars).collect();
            let end = offset + chunk.chars().count();
            if offset == 0 && end == total {
                out.push_str(&format!("\nContent ({} chars):\n", total));
            } else {
                out.push_str(&format!(
                    "\nContent (chars {}–{} of {}):\n",
                    offset, end, total
                ));
            }
            out.push_str(&chunk);
            if end < total {
                out.push_str(&format!(
                    "\n\n[Truncated. Call get_document with offset={} to continue.]\n",
                    end
                ));
            }
        }
    }

    Ok(out)
}

async fn update_document(
    args: &Value,
    auth: &str,
    paperless: &Paperless,
) -> Result<String, ToolError> {
    let id = arg_int(args, "id")?.ok_or_else(|| ToolError::Invalid("`id` is required".into()))?;
    let lookups = paperless.lookups(auth).await?;

    // Validate everything before touching Paperless.
    let mut patch = serde_json::Map::new();
    let mut changes = vec![];
    if let Some(title) = arg_str(args, "title") {
        patch.insert("title".into(), json!(title));
        changes.push(format!("title → {}", title));
    }
    if let Some(created) = arg_date(args, "created")? {
        patch.insert("created".into(), json!(created));
        changes.push(format!("date → {}", created));
    }
    if let Some(c) = arg_str(args, "correspondent") {
        let cid = resolve(&lookups.correspondents, "correspondents", c)?;
        patch.insert("correspondent".into(), json!(cid));
        changes.push(format!(
            "correspondent → {}",
            lookups.correspondent_name(cid)
        ));
    }
    if let Some(t) = arg_str(args, "document_type") {
        let tid = resolve(&lookups.document_types, "document types", t)?;
        patch.insert("document_type".into(), json!(tid));
        changes.push(format!("type → {}", lookups.document_type_name(tid)));
    }
    let add = arg_tag_ids(args, "add_tags", &lookups)?;
    let remove = arg_tag_ids(args, "remove_tags", &lookups)?;
    if let Some(both) = add.iter().find(|t| remove.contains(t)) {
        return Err(ToolError::Invalid(format!(
            "Tag \"{}\" is in both add_tags and remove_tags",
            lookups.tag_name(*both)
        )));
    }

    if !add.is_empty() || !remove.is_empty() {
        // Paperless replaces the whole tag list, so merge with the current one.
        let doc = paperless.document(auth, id).await?;
        let current: Vec<i64> = doc
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|t| t.iter().filter_map(|t| t.as_i64()).collect())
            .unwrap_or_default();
        let mut tags: Vec<i64> = current
            .iter()
            .copied()
            .filter(|t| !remove.contains(t))
            .collect();
        for t in &add {
            if !tags.contains(t) {
                tags.push(*t);
            }
        }
        for t in add.iter().filter(|t| !current.contains(t)) {
            changes.push(format!("+tag {}", lookups.tag_name(*t)));
        }
        for t in remove.iter().filter(|t| current.contains(t)) {
            changes.push(format!("-tag {}", lookups.tag_name(*t)));
        }
        if tags != current {
            patch.insert("tags".into(), json!(tags));
        }
    }

    if patch.is_empty() {
        return if changes.is_empty() && add.is_empty() && remove.is_empty() {
            Err(ToolError::Invalid(
                "Nothing to change: pass at least one of title, created, correspondent, \
                 document_type, add_tags, remove_tags"
                    .into(),
            ))
        } else {
            Ok(format!(
                "Document #{} already has those tags; nothing changed.",
                id
            ))
        };
    }

    let updated: Value = paperless
        .patch(
            auth,
            &format!("/api/documents/{}/", id),
            &Value::Object(patch),
        )
        .await?;
    log::info!(
        "Updated document {} for {}: {}",
        id,
        lookups.username,
        changes.join(", ")
    );
    Ok(format!(
        "Updated ({}):\n{}\n  url: {}\n",
        changes.join(", "),
        summary_line(&updated, &lookups),
        paperless.document_url(id)
    ))
}

async fn add_note(args: &Value, auth: &str, paperless: &Paperless) -> Result<String, ToolError> {
    let id = arg_int(args, "id")?.ok_or_else(|| ToolError::Invalid("`id` is required".into()))?;
    let note =
        arg_str(args, "note").ok_or_else(|| ToolError::Invalid("`note` is required".into()))?;
    let _: Value = paperless
        .post(
            auth,
            &format!("/api/documents/{}/notes/", id),
            &json!({ "note": note }),
        )
        .await?;
    log::info!("Added note to document {}", id);
    Ok(format!(
        "Added note to document #{}.\n  url: {}\n",
        id,
        paperless.document_url(id)
    ))
}

fn arg_color(args: &Value, key: &str) -> Result<Option<String>, ToolError> {
    let Some(value) = arg_str(args, key) else {
        return Ok(None);
    };
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(Some(format!("#{}", hex.to_lowercase())))
    } else {
        Err(ToolError::Invalid(format!(
            "`{}` must be a hex color like #a6cee3",
            key
        )))
    }
}

fn arg_bool(args: &Value, key: &str) -> Result<Option<bool>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" => Ok(Some(true)),
            "false" => Ok(Some(false)),
            _ => Err(ToolError::Invalid(format!("`{}` must be a boolean", key))),
        },
        Some(_) => Err(ToolError::Invalid(format!("`{}` must be a boolean", key))),
    }
}

async fn create_tag(args: &Value, auth: &str, paperless: &Paperless) -> Result<String, ToolError> {
    let name =
        arg_str(args, "name").ok_or_else(|| ToolError::Invalid("`name` is required".into()))?;
    let color = arg_color(args, "color")?;
    let is_inbox = arg_bool(args, "is_inbox_tag")?;

    let mut body = json!({ "name": name, "matching_algorithm": MATCHING_AUTO });
    if let Some(c) = &color {
        body["color"] = json!(c);
    }
    if let Some(i) = is_inbox {
        body["is_inbox_tag"] = json!(i);
    }
    create_named(
        auth,
        paperless,
        "tag",
        "/api/tags/",
        |l| &l.tags,
        name,
        body,
    )
    .await
}

async fn create_correspondent(
    args: &Value,
    auth: &str,
    paperless: &Paperless,
) -> Result<String, ToolError> {
    let name =
        arg_str(args, "name").ok_or_else(|| ToolError::Invalid("`name` is required".into()))?;
    let body = json!({ "name": name, "matching_algorithm": MATCHING_AUTO });
    create_named(
        auth,
        paperless,
        "correspondent",
        "/api/correspondents/",
        |l| &l.correspondents,
        name,
        body,
    )
    .await
}

/// Creates a tag/correspondent unless one with that name already exists.
async fn create_named(
    auth: &str,
    paperless: &Paperless,
    kind: &str,
    path: &str,
    select: impl Fn(&crate::paperless::Lookups) -> &Vec<NamedObject>,
    name: &str,
    body: Value,
) -> Result<String, ToolError> {
    // Bypass the cache so a recent creation (from any client) is seen.
    paperless.invalidate_lookups(auth);
    let lookups = paperless.lookups(auth).await?;
    if let Some(existing) = select(&lookups)
        .iter()
        .find(|o| o.name.eq_ignore_ascii_case(name))
    {
        return Ok(format!(
            "A {} named \"{}\" already exists (id {}); nothing created.",
            kind, existing.name, existing.id
        ));
    }

    let created: NamedObject = paperless.post(auth, path, &body).await?;
    paperless.invalidate_lookups(auth);
    log::info!(
        "Created {} \"{}\" (id {}) for {}",
        kind,
        created.name,
        created.id,
        lookups.username
    );
    Ok(format!(
        "Created {} \"{}\" (id {}).",
        kind, created.name, created.id
    ))
}

async fn list_named(
    auth: &str,
    paperless: &Paperless,
    kind: &str,
    select: impl Fn(&crate::paperless::Lookups) -> &Vec<NamedObject>,
) -> Result<String, ToolError> {
    let lookups = paperless.lookups(auth).await?;
    let mut objects: Vec<&NamedObject> = select(&lookups).iter().collect();
    if objects.is_empty() {
        return Ok(format!("No {}.", kind));
    }
    objects.sort_by_key(|o| o.name.to_lowercase());
    let mut out = format!("{} {}:\n", objects.len(), kind);
    for o in objects {
        match o.document_count {
            Some(count) => out.push_str(&format!("- {} (id {}, {} docs)\n", o.name, o.id, count)),
            None => out.push_str(&format!("- {} (id {})\n", o.name, o.id)),
        }
    }
    Ok(out)
}

/// Whoosh highlights come back as HTML with `<span class="match">` around hits.
fn clean_highlight(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            rest = &rest[start..];
            break;
        };
        let tag = &rest[start..start + end + 1];
        if tag.starts_with("<span") || tag == "</span>" {
            out.push_str("**");
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    let out = out
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    collapse_whitespace(&out)
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_cleanup() {
        assert_eq!(
            clean_highlight("foo <span class=\"match\">bar</span>\n  baz &amp; qux"),
            "foo **bar** baz & qux"
        );
    }

    #[test]
    fn resolve_by_name_or_id() {
        let objects = vec![
            NamedObject {
                id: 3,
                name: "Taxes".into(),
                document_count: None,
            },
            NamedObject {
                id: 7,
                name: "Insurance".into(),
                document_count: None,
            },
        ];
        assert_eq!(resolve(&objects, "tags", "taxes").ok(), Some(3));
        assert_eq!(resolve(&objects, "tags", "7").ok(), Some(7));
        match resolve(&objects, "tags", "tax") {
            Err(ToolError::Invalid(msg)) => assert!(msg.contains("Did you mean: Taxes")),
            _ => panic!("expected suggestion"),
        }
    }

    #[test]
    fn date_validation() {
        assert!(
            arg_date(&json!({"d": "2024-01-31"}), "d")
                .unwrap()
                .is_some()
        );
        assert!(arg_date(&json!({"d": "2024/01/31"}), "d").is_err());
    }

    #[test]
    fn color_validation() {
        assert_eq!(
            arg_color(&json!({"c": "A6CEE3"}), "c").unwrap().as_deref(),
            Some("#a6cee3")
        );
        assert_eq!(
            arg_color(&json!({"c": "#a6cee3"}), "c").unwrap().as_deref(),
            Some("#a6cee3")
        );
        assert!(arg_color(&json!({"c": "red"}), "c").is_err());
        assert!(arg_color(&json!({}), "c").unwrap().is_none());
    }
}
