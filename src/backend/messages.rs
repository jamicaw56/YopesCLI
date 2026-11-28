//! Request-body building: how conversation history, images, system prompts
//! and skills are shaped into the messages array a provider expects.

use std::path::Path;

use anyhow::Result;

use crate::session::{Message, Role};
use crate::{skills, tools};

/// The system prompt every provider shape sends: the built-in prompt (tool,
/// plain agent, or chat), the user's custom prompt, the mode note, then the
/// skills listing.
fn system_prompt(
    prompt: tools::Prompt,
    custom_prompt: Option<&str>,
    mode_note: Option<&str>,
    workspace: &Path,
) -> String {
    let mut system = tools::system_prompt(prompt, workspace);
    // The user's prompt may duplicate what a built-in line already says: send
    // it anyway, since a provider may also enforce its own system prompt.
    if let Some(custom) = custom_prompt.filter(|custom| !custom.trim().is_empty()) {
        system.push_str("\n\n");
        system.push_str(custom.trim());
    }
    if let Some(note) = mode_note {
        system.push_str("\n\n");
        system.push_str(note);
    }
    // Skills auto-load through read_file, so only tool-capable sessions get
    // the listing; slash invocation injects the content by itself.
    if prompt.tools() {
        if let Some(listing) = skills::section(workspace) {
            system.push_str("\n\n");
            system.push_str(&listing);
        }
    }
    system
}

pub(super) fn request_messages(
    messages: &[Message],
    prompt: tools::Prompt,
    image_style: ImageStyle,
    custom_prompt: Option<&str>,
    mode_note: Option<&str>,
    workspace: &Path,
) -> Result<Vec<serde_json::Value>> {
    let tooling = prompt.tools();
    let mut request = Vec::with_capacity(messages.len() + 1);
    let valid_tool_call_ids: std::collections::HashSet<&str> = messages
        .iter()
        .filter_map(|m| m.tool_calls.as_ref())
        .flat_map(|calls| calls.iter().map(|c| c.id.as_str()))
        .collect();
    // Images a tool returned travel in a user message right after that
    // turn's last tool result: every tool_call id still gets its own tool
    // message first, and the image sits in the turn it was read in.
    let mut tool_images: Vec<&crate::session::ImageAttachment> = Vec::new();
    let system = system_prompt(prompt, custom_prompt, mode_note, workspace);
    request.push(serde_json::json!({
        "role": "system",
        "content": system,
    }));
    for (index, message) in messages.iter().enumerate() {
        // Parts and Ollama branches below carry the images themselves.
        let openai_parts = image_style == ImageStyle::Parts;
        // A provider failure goes back to the model as a user-role note: it
        // learns its previous turn failed and why, so the next prompt can
        // recover instead of pretending the turn never happened.
        if let Some(error) = &message.error {
            request.push(serde_json::json!({
                "role": "user",
                "content": format!("[provider error] {error}"),
            }));
            continue;
        }
        let is_valid_tool_call = message
            .tool_call_id
            .as_deref()
            .is_some_and(|id| valid_tool_call_ids.contains(id));
        if message.role == Role::Tool && (!tooling || !is_valid_tool_call) {
            if image_style == ImageStyle::Ollama && !message.images.is_empty() {
                let images = message
                    .images
                    .iter()
                    .map(|image| serde_json::Value::String(image.data.clone()))
                    .collect::<Vec<_>>();
                request.push(serde_json::json!({
                    "role": "user",
                    "content": format!("Tool result:\n{}", message.content),
                    "images": images,
                }));
            } else if image_style == ImageStyle::Parts && !message.images.is_empty() {
                let mut parts = vec![serde_json::json!({
                    "type": "text",
                    "text": format!("Tool result:\n{}", message.content),
                })];
                for image in &message.images {
                    parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:image/png;base64,{}", image.data) }
                    }));
                }
                request.push(serde_json::json!({ "role": "user", "content": parts }));
            } else {
                request.push(serde_json::json!({
                    "role": "user",
                    "content": format!("Tool result:\n{}", message.content),
                }));
            }
            continue;
        }
        // A live tool message stays text; its images travel in a user
        // message right after this turn's last tool result. That keeps the
        // tool-call pairing valid for every provider and parks the image in
        // the turn it was read in — collecting every image ever read into
        // one message appended after the newest prompt made models treat
        // them as freshly attached and re-mention them every turn.
        if message.role == Role::Tool && tooling && is_valid_tool_call {
            let mut value = serde_json::to_value(message)?;
            if let Some(object) = value.as_object_mut() {
                object.remove("images");
            }
            request.push(value);
            if openai_parts {
                tool_images.extend(message.images.iter());
                // More tool results of this turn follow only when the next
                // live message is another tool result; otherwise hand the
                // buffered images over before the conversation moves on.
                let more_tool_results = messages[index + 1..]
                    .iter()
                    .find(|next| next.error.is_none())
                    .is_some_and(|next| {
                        next.role == Role::Tool
                            && next
                                .tool_call_id
                                .as_deref()
                                .is_some_and(|id| valid_tool_call_ids.contains(id))
                    });
                if !more_tool_results && !tool_images.is_empty() {
                    let images = std::mem::take(&mut tool_images);
                    request.push(tool_images_message(&images));
                }
            }
            continue;
        }
        // Attached screenshots travel so a vision-capable model sees them:
        // multimodal parts for OpenAI-compatible providers, Ollama's own
        // `images` array there.
        if openai_parts && message.role == Role::User && !message.images.is_empty() {
            let mut parts: Vec<serde_json::Value> = Vec::with_capacity(1 + message.images.len());
            if !message.content.is_empty() {
                parts.push(serde_json::json!({ "type": "text", "text": message.content }));
            }
            for image in &message.images {
                parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:image/png;base64,{}", image.data)
                    }
                }));
            }
            request.push(serde_json::json!({ "role": "user", "content": parts }));
            continue;
        }
        if image_style == ImageStyle::Ollama
            && message.role == Role::User
            && !message.images.is_empty()
        {
            let mut value = serde_json::to_value(message)?;
            if let Some(object) = value.as_object_mut() {
                let attached = object
                    .remove("images")
                    .and_then(|images| images.as_array().cloned())
                    .unwrap_or_default();
                let images = attached
                    .iter()
                    .filter_map(|image| image.get("data").and_then(serde_json::Value::as_str))
                    .map(|data| serde_json::Value::String(data.to_owned()))
                    .collect::<Vec<_>>();
                object.insert("images".to_owned(), serde_json::Value::from(images));
            }
            request.push(value);
            continue;
        }
        if message.role == Role::Assistant
            && message.content.is_empty()
            && message.tool_calls.is_none()
        {
            continue;
        }
        let mut value = serde_json::to_value(message)?;
        strip_thought_signatures(&mut value);
        if !tooling {
            if let Some(object) = value.as_object_mut() {
                object.remove("tool_calls");
                object.remove("tool_call_id");
            }
            if message.role == Role::Assistant && message.content.is_empty() {
                continue;
            }
        }
        // Parts and Ollama branches above already carry the images; anything
        // left in the plain path must not leak the raw attachment field.
        if let Some(object) = value.as_object_mut() {
            object.remove("images");
        }
        request.push(value);
    }
    Ok(request)
}

/// Gemini's thought signature belongs to Gemini's request shape, not to an
/// OpenAI tool call. Drop it when a session created on Gemini is sent to a
/// different provider, so strict compatible servers never see the extra key.
fn strip_thought_signatures(value: &mut serde_json::Value) {
    if let Some(calls) = value
        .get_mut("tool_calls")
        .and_then(serde_json::Value::as_array_mut)
    {
        for call in calls {
            if let Some(object) = call.as_object_mut() {
                object.remove("thought_signature");
            }
        }
    }
}

/// The user message that hands images a tool returned to the model: one
/// text part, then one image part per image.
fn tool_images_message(images: &[&crate::session::ImageAttachment]) -> serde_json::Value {
    let mut parts = vec![serde_json::json!({
        "type": "text",
        "text": "Images read by the tools:",
    })];
    for image in images {
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": { "url": format!("data:image/png;base64,{}", image.data) }
        }));
    }
    serde_json::json!({ "role": "user", "content": parts })
}

/// How images ride along in request messages: OpenAI-compatible multimodal
/// text parts, Ollama's `images` base64 array, or not at all.
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub(super) enum ImageStyle {
    #[default]
    None,
    Parts,
    Ollama,
}

/// Shapes the conversation history into Anthropic's Messages payload:
/// a top-level system string and alternating user/assistant turns.
///
/// ponytail: complex tool result caching controls are omitted;
/// add when prompt caching is explicitly configured.
pub(super) fn anthropic_request_payload(
    messages: &[Message],
    prompt: tools::Prompt,
    custom_prompt: Option<&str>,
    mode_note: Option<String>,
    workspace: &Path,
) -> Result<(Option<String>, Vec<serde_json::Value>)> {
    let tooling = prompt.tools();
    let system = system_prompt(prompt, custom_prompt, mode_note.as_deref(), workspace);

    let mut anthropic_messages: Vec<serde_json::Value> = Vec::new();
    let valid_tool_call_ids: std::collections::HashSet<&str> = messages
        .iter()
        .filter_map(|m| m.tool_calls.as_ref())
        .flat_map(|calls| calls.iter().map(|c| c.id.as_str()))
        .collect();
    for message in messages {
        // The same failure note, in Anthropic's content-block shape.
        if let Some(error) = &message.error {
            anthropic_messages.push(serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": format!("[provider error] {error}")}],
            }));
            continue;
        }
        match message.role {
            Role::User => {
                let mut content = Vec::new();
                for image in &message.images {
                    content.push(serde_json::json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": image.data,
                        }
                    }));
                }
                if !message.content.is_empty() {
                    content.push(serde_json::json!({
                        "type": "text",
                        "text": message.content,
                    }));
                }
                if content.is_empty() {
                    continue;
                }
                if let Some(last) = anthropic_messages
                    .last_mut()
                    .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                {
                    if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                        arr.extend(content);
                        continue;
                    }
                }
                anthropic_messages.push(serde_json::json!({
                    "role": "user",
                    "content": content,
                }));
            }
            Role::Assistant => {
                let mut content = Vec::new();
                if !message.content.is_empty() {
                    content.push(serde_json::json!({
                        "type": "text",
                        "text": message.content,
                    }));
                }
                if tooling {
                    if let Some(calls) = &message.tool_calls {
                        for call in calls {
                            let input: serde_json::Value =
                                serde_json::from_str(&call.function.arguments)
                                    .unwrap_or_else(|_| serde_json::json!({}));
                            content.push(serde_json::json!({
                                "type": "tool_use",
                                "id": call.id,
                                "name": call.function.name,
                                "input": input,
                            }));
                        }
                    }
                }
                if content.is_empty() {
                    continue;
                }
                anthropic_messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": content,
                }));
            }
            Role::Tool => {
                let call_id = message.tool_call_id.as_deref().unwrap_or_default();
                if !valid_tool_call_ids.contains(call_id) {
                    let text_block = serde_json::json!({
                        "type": "text",
                        "text": format!("Command output:\n{}", message.content),
                    });
                    if let Some(last) = anthropic_messages
                        .last_mut()
                        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                    {
                        if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                            arr.push(text_block);
                            continue;
                        }
                    }
                    anthropic_messages.push(serde_json::json!({
                        "role": "user",
                        "content": [text_block],
                    }));
                    continue;
                }
                // A screenshot a tool took (read_image, computer_use) rides in
                // the tool result itself: text block first, then one image
                // block per attachment.
                let tool_result_block = if message.images.is_empty() {
                    serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": message.content,
                    })
                } else {
                    let mut blocks = vec![serde_json::json!({
                        "type": "text",
                        "text": message.content,
                    })];
                    for image in &message.images {
                        blocks.push(serde_json::json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": image.data,
                            }
                        }));
                    }
                    serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": blocks,
                    })
                };
                if let Some(last) = anthropic_messages
                    .last_mut()
                    .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                {
                    if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                        arr.push(tool_result_block);
                        continue;
                    }
                }
                anthropic_messages.push(serde_json::json!({
                    "role": "user",
                    "content": [tool_result_block],
                }));
            }
        }
    }

    if anthropic_messages
        .first()
        .and_then(|m| m.get("role"))
        .and_then(|r| r.as_str())
        == Some("assistant")
    {
        anthropic_messages.insert(
            0,
            serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": "Begin."}],
            }),
        );
    }

    let system_opt = if system.trim().is_empty() {
        None
    } else {
        Some(system)
    };
    Ok((system_opt, anthropic_messages))
}

pub(super) fn anthropic_tools() -> serde_json::Value {
    let defs = tools::definitions();
    let tools = defs
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let f = t.get("function")?;
                    let name = f.get("name")?.as_str()?;
                    let desc = f
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or_default();
                    let schema = f
                        .get("parameters")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({"type": "object"}));
                    Some(serde_json::json!({
                        "name": name,
                        "description": desc,
                        "input_schema": schema,
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    serde_json::Value::Array(tools)
}

/// Google's documented placeholder for a function call whose original
/// signature was not stored, so history from before signatures were kept can
/// still be replayed.
const SKIP_THOUGHT_SIGNATURE: &str = "skip_thought_signature_validator";

/// Shapes the conversation history into Gemini's generateContent payload:
/// systemInstruction and contents with alternating user/model roles.
///
/// ponytail: safety settings and generationConfig overrides are skipped;
/// add when temperature or safety threshold customization is needed.
pub(super) fn gemini_request_payload(
    messages: &[Message],
    prompt: tools::Prompt,
    custom_prompt: Option<&str>,
    mode_note: Option<String>,
    workspace: &Path,
) -> Result<(Option<serde_json::Value>, Vec<serde_json::Value>)> {
    let tooling = prompt.tools();
    let system = system_prompt(prompt, custom_prompt, mode_note.as_deref(), workspace);

    let mut call_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for msg in messages {
        if let Some(calls) = &msg.tool_calls {
            for call in calls {
                call_names.insert(call.id.clone(), call.function.name.clone());
            }
        }
    }

    let mut contents: Vec<serde_json::Value> = Vec::new();
    for message in messages {
        // And in Gemini's contents shape.
        if let Some(error) = &message.error {
            contents.push(serde_json::json!({
                "role": "user",
                "parts": [{"text": format!("[provider error] {error}")}],
            }));
            continue;
        }
        match message.role {
            Role::User => {
                let mut parts = Vec::new();
                for image in &message.images {
                    parts.push(serde_json::json!({
                        "inlineData": {
                            "mimeType": "image/png",
                            "data": image.data,
                        }
                    }));
                }
                if !message.content.is_empty() {
                    parts.push(serde_json::json!({ "text": message.content }));
                }
                if parts.is_empty() {
                    continue;
                }
                if let Some(last) = contents
                    .last_mut()
                    .filter(|c| c.get("role").and_then(|r| r.as_str()) == Some("user"))
                {
                    if let Some(arr) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                        arr.extend(parts);
                        continue;
                    }
                }
                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": parts,
                }));
            }
            Role::Assistant => {
                let mut parts = Vec::new();
                if !message.content.is_empty() {
                    parts.push(serde_json::json!({ "text": message.content }));
                }
                if tooling {
                    if let Some(calls) = &message.tool_calls {
                        // Gemini 3+ requires the thought signature returned
                        // with the first function call to be replayed exactly.
                        // A parallel batch leaves its later calls unsigned. For
                        // history saved before signatures were kept, Google's
                        // documented dummy signature bypasses validation.
                        let mut signed = false;
                        for call in calls {
                            let args: serde_json::Value =
                                serde_json::from_str(&call.function.arguments)
                                    .unwrap_or_else(|_| serde_json::json!({}));
                            let mut part = serde_json::json!({
                                "functionCall": {
                                    "name": call.function.name,
                                    "args": args,
                                }
                            });
                            match call.thought_signature.as_deref() {
                                Some(signature) if !signature.is_empty() => {
                                    part["thoughtSignature"] =
                                        serde_json::Value::String(signature.to_owned());
                                    signed = true;
                                }
                                _ if !signed => {
                                    part["thoughtSignature"] = serde_json::Value::String(
                                        SKIP_THOUGHT_SIGNATURE.to_owned(),
                                    );
                                }
                                _ => {}
                            }
                            parts.push(part);
                        }
                    }
                }
                if parts.is_empty() {
                    continue;
                }
                if let Some(last) = contents
                    .last_mut()
                    .filter(|c| c.get("role").and_then(|r| r.as_str()) == Some("model"))
                {
                    if let Some(arr) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                        arr.extend(parts);
                        continue;
                    }
                }
                contents.push(serde_json::json!({
                    "role": "model",
                    "parts": parts,
                }));
            }
            Role::Tool => {
                let call_id = message.tool_call_id.as_deref().unwrap_or_default();
                let Some(func_name) = call_names.get(call_id).map(|s| s.as_str()) else {
                    let text_part = serde_json::json!({
                        "text": format!("Command output:\n{}", message.content),
                    });
                    if let Some(last) = contents
                        .last_mut()
                        .filter(|c| c.get("role").and_then(|r| r.as_str()) == Some("user"))
                    {
                        if let Some(arr) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                            arr.push(text_part);
                            continue;
                        }
                    }
                    contents.push(serde_json::json!({
                        "role": "user",
                        "parts": [text_part],
                    }));
                    continue;
                };
                let part = serde_json::json!({
                    "functionResponse": {
                        "name": func_name,
                        "response": {
                            "name": func_name,
                            "content": message.content,
                        }
                    }
                });
                // Gemini carries a tool's screenshot as a sibling inline part
                // in the same user turn.
                let mut parts = vec![part];
                for image in &message.images {
                    parts.push(serde_json::json!({
                        "inlineData": {
                            "mimeType": "image/png",
                            "data": image.data,
                        }
                    }));
                }
                if let Some(last) = contents
                    .last_mut()
                    .filter(|c| c.get("role").and_then(|r| r.as_str()) == Some("user"))
                {
                    if let Some(arr) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                        arr.extend(parts);
                        continue;
                    }
                }
                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": parts,
                }));
            }
        }
    }

    let system_instruction = if system.trim().is_empty() {
        None
    } else {
        Some(serde_json::json!({
            "parts": [{ "text": system }]
        }))
    };
    Ok((system_instruction, contents))
}

pub(super) fn gemini_tools() -> serde_json::Value {
    let defs = tools::definitions();
    let decls = defs
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let f = t.get("function")?;
                    let name = f.get("name")?.as_str()?;
                    let desc = f
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or_default();
                    let schema = f
                        .get("parameters")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({"type": "object"}));
                    Some(serde_json::json!({
                        "name": name,
                        "description": desc,
                        "parameters": schema,
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    serde_json::json!([{ "functionDeclarations": decls }])
}

#[cfg(test)]
mod tests {
    use super::request_messages;
    use crate::backend::ImageStyle;
    use crate::session::{Message, Role, ToolCall};
    use crate::tools::Prompt;
    use std::path::Path;

    /// A failed request is not dropped from history: the model gets the
    /// provider's error as a user-role note, so the next prompt can recover.
    #[test]
    fn a_provider_error_reaches_the_model_as_context() {
        let messages = vec![
            Message::new(Role::User, "do the thing"),
            Message::provider_error("API returned 503: timeout"),
        ];
        let request = request_messages(
            &messages,
            Prompt::Tools,
            ImageStyle::None,
            None,
            None,
            Path::new("."),
        )
        .expect("messages");
        let note = request
            .iter()
            .find(|entry| {
                entry.get("role").and_then(|role| role.as_str()) == Some("user")
                    && entry
                        .get("content")
                        .and_then(|content| content.as_str())
                        .is_some_and(|content| content.contains("[provider error]"))
            })
            .expect("the failure reaches the model");
        assert!(
            note["content"]
                .as_str()
                .is_some_and(|content| content.contains("503: timeout")),
            "{note}"
        );
        // The failed turn never masquerades as an assistant reply.
        assert!(
            request.iter().all(|entry| {
                entry.get("role").and_then(|role| role.as_str()) != Some("assistant")
            }),
            "{request:?}"
        );
    }

    #[test]
    fn request_messages_keep_tool_history_only_when_tooling_is_enabled() {
        let mut assistant = Message::new(Role::Assistant, "");
        assistant.tool_calls = Some(vec![ToolCall::new(
            "call_1",
            "read_file",
            r#"{"path":"README.md"}"#,
        )]);
        let messages = vec![
            Message::new(Role::User, "read it"),
            assistant,
            Message::tool_result("call_1", "contents"),
        ];

        let tooling = request_messages(
            &messages,
            Prompt::Tools,
            ImageStyle::None,
            None,
            None,
            Path::new("."),
        )
        .unwrap();
        assert_eq!(tooling[0]["role"], "system");
        assert_eq!(tooling[2]["tool_calls"][0]["function"]["name"], "read_file");
        assert_eq!(tooling[3]["tool_call_id"], "call_1");

        let plain = request_messages(
            &messages,
            Prompt::NoTools,
            ImageStyle::None,
            None,
            None,
            Path::new("."),
        )
        .unwrap();
        assert!(
            plain
                .iter()
                .all(|message| message.get("tool_calls").is_none())
        );
        assert!(plain.iter().any(|message| {
            message["content"]
                .as_str()
                .is_some_and(|content| content.contains("Tool result"))
        }));
    }

    /// Chat mode sends a conversational system prompt: no tool text, no
    /// skills listing, and tool history turned back into prose.
    #[test]
    fn a_chat_request_carries_no_tool_context() {
        let mut assistant = Message::new(Role::Assistant, "");
        assistant.tool_calls = Some(vec![ToolCall::new(
            "call_1",
            "read_file",
            r#"{"path":"README.md"}"#,
        )]);
        let messages = vec![
            Message::new(Role::User, "read it"),
            assistant,
            Message::tool_result("call_1", "contents"),
        ];
        let chat = request_messages(
            &messages,
            Prompt::Chat,
            ImageStyle::None,
            None,
            None,
            Path::new("."),
        )
        .unwrap();
        let system = chat[0]["content"].as_str().unwrap();
        assert!(system.contains("chat companion"), "{system}");
        assert!(!system.contains("run_command"), "{system}");
        assert!(!system.contains("The user installed skills"), "{system}");
        // The tool round is still in history, but as plain user text.
        assert!(chat.iter().all(|entry| entry.get("tool_calls").is_none()));
        assert!(chat.iter().any(|entry| {
            entry["content"]
                .as_str()
                .is_some_and(|content| content.contains("Tool result"))
        }));
    }

    #[test]
    fn the_mode_instruction_lands_in_the_system_prompt() {
        let messages = vec![Message::new(Role::User, "hi")];
        let plain = request_messages(
            &messages,
            Prompt::Tools,
            ImageStyle::None,
            None,
            None,
            Path::new("."),
        )
        .unwrap();
        let plan = request_messages(
            &messages,
            Prompt::Tools,
            ImageStyle::None,
            None,
            Some("PLAN MODE: propose only"),
            Path::new("."),
        )
        .unwrap();
        assert!(
            !plain[0]["content"]
                .as_str()
                .is_some_and(|content| content.contains("PLAN MODE"))
        );
        assert!(
            plan[0]["content"]
                .as_str()
                .is_some_and(|content| content.contains("PLAN MODE: propose only"))
        );
    }

    #[test]
    fn the_custom_system_prompt_lands_between_base_and_mode() {
        let messages = vec![Message::new(Role::User, "hi")];
        let request = request_messages(
            &messages,
            Prompt::Tools,
            ImageStyle::None,
            Some(" Always answer in rhyme. "),
            Some("PLAN MODE: propose only"),
            Path::new("."),
        )
        .unwrap();
        let system = request[0]["content"].as_str().unwrap();
        assert!(system.contains("Working directory:"));
        assert!(system.contains("Always answer in rhyme."));
        // The custom text precedes the mode instruction, built-ins first.
        assert!(system.find("Always answer").unwrap() < system.find("PLAN MODE").unwrap());
        // A blank prompt adds nothing.
        assert_eq!(
            request_messages(
                &messages,
                Prompt::Tools,
                ImageStyle::None,
                Some("  "),
                None,
                Path::new(".")
            )
            .unwrap(),
            request_messages(
                &messages,
                Prompt::Tools,
                ImageStyle::None,
                None,
                None,
                Path::new(".")
            )
            .unwrap()
        );
    }

    #[test]
    fn attached_images_travel_in_the_providers_own_format() {
        use crate::session::ImageAttachment;

        let mut message = Message::new(Role::User, "what changed?");
        message.images.push(ImageAttachment {
            name: "shot.png".to_owned(),
            data: "aWNvbg==".to_owned(),
            thumbnail: false,
        });
        let messages = vec![message];

        let parts = request_messages(
            &messages,
            Prompt::Tools,
            ImageStyle::Parts,
            None,
            None,
            Path::new("."),
        )
        .unwrap();
        let sent = parts
            .last()
            .unwrap()
            .get("content")
            .and_then(serde_json::Value::as_array)
            .unwrap();
        let urls = sent
            .iter()
            .filter_map(|part| part.get("image_url").and_then(serde_json::Value::as_object))
            .collect::<Vec<_>>();
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0]["url"], "data:image/png;base64,aWNvbg==");
        assert!(parts.last().unwrap().get("images").is_none());

        // Ollama wants a flat base64 array on the message itself.
        let ollama = request_messages(
            &messages,
            Prompt::Tools,
            ImageStyle::Ollama,
            None,
            None,
            Path::new("."),
        )
        .unwrap();
        let last = ollama.last().unwrap();
        assert_eq!(last["content"], "what changed?");
        assert_eq!(last["images"][0], "aWNvbg==");
        // Plain requests keep the raw attachment field out.
        let plain = request_messages(
            &messages,
            Prompt::NoTools,
            ImageStyle::None,
            None,
            None,
            Path::new("."),
        )
        .unwrap();
        assert!(plain.last().unwrap().get("images").is_none());
        assert_eq!(plain.last().unwrap()["role"], "user");
    }

    #[test]
    fn tool_read_images_land_after_their_tool_run_not_after_every_later_prompt() {
        use crate::session::ImageAttachment;

        let mut assistant = Message::new(Role::Assistant, "");
        assistant.tool_calls = Some(vec![ToolCall::new(
            "call_1",
            "read_image",
            r#"{"path":"shot.png"}"#,
        )]);
        let mut tool = Message::tool_result("call_1", "a screenshot");
        tool.images = vec![ImageAttachment {
            name: "shot.png".to_owned(),
            data: "aWNvbg==".to_owned(),
            thumbnail: false,
        }];
        let messages = vec![
            Message::new(Role::User, "what is in shot.png?"),
            assistant,
            tool,
            Message::new(Role::Assistant, "It shows a red square."),
            Message::new(Role::User, "thanks, now list the files"),
        ];

        let request = request_messages(
            &messages,
            Prompt::Tools,
            ImageStyle::Parts,
            None,
            None,
            Path::new("."),
        )
        .unwrap();
        // system, user, assistant, tool, user(images), assistant, user
        assert_eq!(request.len(), 7);
        assert_eq!(request[3]["role"], "tool");
        assert!(request[3].get("images").is_none());
        // The images follow their tool result directly, in the turn they
        // were read in.
        assert_eq!(request[4]["role"], "user");
        assert_eq!(
            request[4]["content"][0]["text"],
            "Images read by the tools:"
        );
        assert_eq!(
            request[4]["content"][1]["image_url"]["url"],
            "data:image/png;base64,aWNvbg=="
        );
        // The newest user prompt stays last: no image dump is re-attached
        // after it on every request.
        assert_eq!(
            request.last().unwrap()["content"],
            "thanks, now list the files"
        );
    }

    #[test]
    fn one_run_of_tool_results_yields_one_images_message_after_the_last_result() {
        use crate::session::ImageAttachment;

        let mut assistant = Message::new(Role::Assistant, "");
        assistant.tool_calls = Some(vec![
            ToolCall::new("call_1", "read_image", r#"{"path":"a.png"}"#),
            ToolCall::new("call_2", "read_file", r#"{"path":"b.txt"}"#),
        ]);
        let mut first = Message::tool_result("call_1", "a screenshot");
        first.images = vec![ImageAttachment {
            name: "a.png".to_owned(),
            data: "aW1hZ2U=".to_owned(),
            thumbnail: false,
        }];
        let messages = vec![
            Message::new(Role::User, "look at a.png and b.txt"),
            assistant,
            first,
            Message::tool_result("call_2", "text of b"),
        ];

        let request = request_messages(
            &messages,
            Prompt::Tools,
            ImageStyle::Parts,
            None,
            None,
            Path::new("."),
        )
        .unwrap();
        // system, user, assistant, tool, tool, user(images): the images
        // message waits for the whole run so tool-call pairing stays valid.
        assert_eq!(request.len(), 6);
        assert_eq!(request[3]["tool_call_id"], "call_1");
        assert_eq!(request[4]["tool_call_id"], "call_2");
        let images_message = request.last().unwrap();
        assert_eq!(
            images_message["content"][0]["text"],
            "Images read by the tools:"
        );
        assert_eq!(
            images_message["content"][1]["image_url"]["url"],
            "data:image/png;base64,aW1hZ2U="
        );
    }

    #[test]
    fn anthropic_payload_shapes_tools_and_roles() {
        use super::anthropic_request_payload;

        let mut assistant = Message::new(Role::Assistant, "Checking...");
        assistant.tool_calls = Some(vec![ToolCall::new(
            "call_42",
            "read_file",
            r#"{"path":"foo.rs"}"#,
        )]);
        let messages = vec![
            Message::new(Role::User, "read foo"),
            assistant,
            Message::tool_result("call_42", "fn main() {}"),
        ];

        let (system, payload) =
            anthropic_request_payload(&messages, Prompt::Tools, None, None, Path::new("."))
                .unwrap();
        assert!(system.is_some());
        assert_eq!(payload.len(), 3);
        assert_eq!(payload[0]["role"], "user");
        assert_eq!(payload[1]["role"], "assistant");
        assert_eq!(payload[1]["content"][1]["type"], "tool_use");
        assert_eq!(payload[1]["content"][1]["name"], "read_file");
        assert_eq!(payload[2]["role"], "user");
        assert_eq!(payload[2]["content"][0]["type"], "tool_result");
        assert_eq!(payload[2]["content"][0]["tool_use_id"], "call_42");
    }

    #[test]
    fn gemini_payload_shapes_system_instruction_and_contents() {
        use super::gemini_request_payload;

        let mut call = ToolCall::new("call_99", "list_dir", r#"{"path":"."}"#);
        call.thought_signature = Some("sig-99".to_owned());
        let mut assistant = Message::new(Role::Assistant, "Running tool...");
        assistant.tool_calls = Some(vec![call]);
        let messages = vec![
            Message::new(Role::User, "list files"),
            assistant,
            Message::tool_result("call_99", "file.txt"),
        ];

        let (instruction, contents) =
            gemini_request_payload(&messages, Prompt::Tools, None, None, Path::new(".")).unwrap();
        assert!(instruction.is_some());
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][1]["functionCall"]["name"], "list_dir");
        assert_eq!(contents[1]["parts"][1]["thoughtSignature"], "sig-99");
        assert_eq!(contents[2]["role"], "user");
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["name"],
            "list_dir"
        );
    }

    /// Gemini 3 rejects a replayed function call without its thought
    /// signature. Parallel calls keep their one signature on the first call;
    /// history saved before signatures were kept gets the documented fallback.
    #[test]
    fn gemini_replays_thought_signatures_and_backfills_missing_ones() {
        use super::{SKIP_THOUGHT_SIGNATURE, gemini_request_payload};

        let mut signed = ToolCall::new("call_1", "list_dir", r#"{"path":"."}"#);
        signed.thought_signature = Some("sig-1".to_owned());
        let unsigned = ToolCall::new("call_2", "read_file", r#"{"path":"a.txt"}"#);
        let mut assistant = Message::new(Role::Assistant, "");
        assistant.tool_calls = Some(vec![signed, unsigned]);
        let messages = vec![Message::new(Role::User, "go"), assistant];

        let (_, contents) =
            gemini_request_payload(&messages, Prompt::Tools, None, None, Path::new(".")).unwrap();
        assert_eq!(contents[1]["parts"][0]["thoughtSignature"], "sig-1");
        assert!(contents[1]["parts"][1].get("thoughtSignature").is_none());

        let mut old = Message::new(Role::Assistant, "");
        old.tool_calls = Some(vec![ToolCall::new("call_3", "list_dir", "{}")]);
        let messages = vec![Message::new(Role::User, "go"), old];
        let (_, contents) =
            gemini_request_payload(&messages, Prompt::Tools, None, None, Path::new(".")).unwrap();
        assert_eq!(
            contents[1]["parts"][0]["thoughtSignature"],
            SKIP_THOUGHT_SIGNATURE
        );
    }

    /// The exact failure from the field: a Gemini tool call is parsed from the
    /// stream, then the follow-up request must carry its signature back.
    #[test]
    fn a_gemini_tool_call_round_trips_its_signature_into_the_next_request() {
        use super::gemini_request_payload;
        use crate::backend::sse::{OpenAiState, parse_gemini_line};

        let mut state = OpenAiState::default();
        parse_gemini_line(
            r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"list_dir","args":{"path":"."}},"thoughtSignature":"sig-round-trip"}],"role":"model"},"finishReason":"STOP"}]}"#,
            &mut state,
        )
        .unwrap();
        let calls = state.finish(1);
        assert_eq!(calls.len(), 1);

        let mut assistant = Message::new(Role::Assistant, "");
        assistant.tool_calls = Some(calls);
        let messages = vec![
            Message::new(Role::User, "list files"),
            assistant,
            Message::tool_result("call_1_0", "file.txt"),
        ];
        let (_, contents) =
            gemini_request_payload(&messages, Prompt::Tools, None, None, Path::new(".")).unwrap();
        assert_eq!(
            contents[1]["parts"][0]["thoughtSignature"],
            "sig-round-trip"
        );
    }

    /// The Gemini signature stays in the session but never leaks into an
    /// OpenAI-compatible request after a provider switch.
    #[test]
    fn openai_requests_do_not_carry_gemini_signatures() {
        let mut call = ToolCall::new("call_1", "list_dir", "{}");
        call.thought_signature = Some("sig-1".to_owned());
        let mut assistant = Message::new(Role::Assistant, "");
        assistant.tool_calls = Some(vec![call]);

        let request = request_messages(
            &[assistant],
            Prompt::Tools,
            ImageStyle::None,
            None,
            None,
            Path::new("."),
        )
        .unwrap();
        assert!(
            request[1]["tool_calls"][0]
                .get("thought_signature")
                .is_none()
        );
    }

    #[test]
    fn orphaned_tool_messages_become_user_content_so_api_never_rejects_400() {
        use super::{anthropic_request_payload, gemini_request_payload};

        let messages = vec![
            Message::new(Role::User, "$ echo hi"),
            Message::tool_result("user-cmd-1", "hi"),
        ];

        let openai = request_messages(
            &messages,
            Prompt::Tools,
            ImageStyle::None,
            None,
            None,
            Path::new("."),
        )
        .unwrap();
        assert_eq!(openai.len(), 3);
        assert_eq!(openai[1]["role"], "user");
        assert_eq!(openai[2]["role"], "user");
        assert!(openai[2]["content"].as_str().unwrap().contains("hi"));

        let (_, anthropic) =
            anthropic_request_payload(&messages, Prompt::Tools, None, None, Path::new("."))
                .unwrap();
        assert_eq!(anthropic.len(), 1);
        assert_eq!(anthropic[0]["role"], "user");

        let (_, gemini) =
            gemini_request_payload(&messages, Prompt::Tools, None, None, Path::new(".")).unwrap();
        assert_eq!(gemini.len(), 1);
        assert_eq!(gemini[0]["role"], "user");
    }
}
