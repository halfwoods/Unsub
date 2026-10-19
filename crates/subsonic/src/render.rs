//! The Subsonic response envelope in XML and JSON.
//!
//! Payloads are built once as `serde_json::Value` trees and rendered either way. The mapping
//! follows how Subsonic's own XML and JSON relate: scalar members are XML attributes, objects and
//! arrays are child elements (an array repeats its element), and a member named `value` is the
//! element's text.

use std::borrow::Cow;

use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value, json};

use crate::{Error, params::Params};

pub const API_VERSION: &str = "1.16.1";
const XML_NAMESPACE: &str = "http://subsonic.org/restapi";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Xml,
    Json,
}

impl Format {
    /// XML unless the client asked for JSON, as Subsonic does.
    pub fn of(params: &Params) -> Self {
        match params.get("f") {
            Some(f) if f.eq_ignore_ascii_case("json") => Self::Json,
            _ => Self::Xml,
        }
    }
}

/// A successful response, with an optional named payload.
pub fn ok(format: Format, payload: Option<(&'static str, Value)>) -> Response {
    let mut envelope = envelope("ok");
    if let Some((name, body)) = payload {
        envelope.insert(name.to_owned(), body);
    }
    respond(format, StatusCode::OK, &envelope)
}

pub fn error(format: Format, err: &Error, status: StatusCode) -> Response {
    let mut envelope = envelope("failed");
    envelope.insert(
        "error".to_owned(),
        json!({ "code": err.code(), "message": err.to_string() }),
    );
    respond(format, status, &envelope)
}

fn envelope(status: &str) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("status".to_owned(), status.into());
    map.insert("version".to_owned(), API_VERSION.into());
    map.insert("type".to_owned(), "AntiStreaming".into());
    map.insert("serverVersion".to_owned(), env!("CARGO_PKG_VERSION").into());
    map.insert("openSubsonic".to_owned(), true.into());
    map
}

fn respond(format: Format, status: StatusCode, envelope: &Map<String, Value>) -> Response {
    let (content_type, body) = match format {
        Format::Json => (
            "application/json; charset=utf-8",
            json!({ "subsonic-response": envelope }).to_string(),
        ),
        Format::Xml => ("text/xml; charset=utf-8", xml_document(envelope)),
    };
    (status, [(header::CONTENT_TYPE, content_type)], body).into_response()
}

fn xml_document(envelope: &Map<String, Value>) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    write_object(&mut out, "subsonic-response", envelope, Some(XML_NAMESPACE));
    out
}

fn write_value(out: &mut String, name: &str, value: &Value) {
    match value {
        Value::Object(map) => write_object(out, name, map, None),
        Value::Array(items) => items.iter().for_each(|item| write_value(out, name, item)),
        Value::Null => {}
        scalar => {
            if let Some(text) = scalar_text(scalar) {
                out.push_str(&format!("<{name}>"));
                escape_into(out, &text, false);
                out.push_str(&format!("</{name}>"));
            }
        }
    }
}

fn write_object(out: &mut String, name: &str, map: &Map<String, Value>, namespace: Option<&str>) {
    out.push('<');
    out.push_str(name);
    if let Some(namespace) = namespace {
        out.push_str(" xmlns=\"");
        escape_into(out, namespace, true);
        out.push('"');
    }
    for (key, value) in map {
        if key != "value"
            && let Some(text) = scalar_text(value)
        {
            out.push_str(&format!(" {key}=\""));
            escape_into(out, &text, true);
            out.push('"');
        }
    }

    let text = map.get("value").and_then(scalar_text);
    if text.is_none() && !map.values().any(is_container) {
        out.push_str("/>");
        return;
    }
    out.push('>');
    if let Some(text) = text {
        escape_into(out, &text, false);
    }
    for (key, value) in map {
        if is_container(value) {
            write_value(out, key, value);
        }
    }
    out.push_str(&format!("</{name}>"));
}

fn is_container(value: &Value) -> bool {
    matches!(value, Value::Object(_) | Value::Array(_))
}

fn scalar_text(value: &Value) -> Option<Cow<'_, str>> {
    match value {
        Value::String(text) => Some(Cow::Borrowed(text)),
        Value::Number(number) => Some(Cow::Owned(number.to_string())),
        Value::Bool(flag) => Some(Cow::Borrowed(if *flag { "true" } else { "false" })),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn escape_into(out: &mut String, text: &str, attribute: bool) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            // Attribute normalisation would turn raw whitespace controls into spaces.
            '\n' if attribute => out.push_str("&#10;"),
            '\r' if attribute => out.push_str("&#13;"),
            '\t' if attribute => out.push_str("&#9;"),
            // XML 1.0 cannot represent these at all, and tags scraped from damaged files do contain
            // them; one would make the whole document unparsable for the client.
            c if (c < ' ' && !matches!(c, '\t' | '\n' | '\r'))
                || matches!(c, '\u{FFFE}' | '\u{FFFF}') => {}
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xml(value: Value) -> String {
        let Value::Object(map) = value else {
            panic!("object expected")
        };
        let mut out = String::new();
        write_object(&mut out, "root", &map, None);
        out
    }

    #[test]
    fn scalars_are_attributes_and_containers_are_children() {
        let out = xml(
            json!({ "n": 1, "ok": true, "child": { "x": "y" }, "item": [{ "i": 1 }, { "i": 2 }] }),
        );
        assert!(out.starts_with("<root"));
        assert!(out.contains(" n=\"1\"") && out.contains(" ok=\"true\""));
        assert!(out.contains("<child x=\"y\"/>"));
        assert!(out.contains("<item i=\"1\"/><item i=\"2\"/>"));
        assert!(out.ends_with("</root>"));
    }

    #[test]
    fn value_member_is_element_text() {
        assert_eq!(
            xml(json!({ "value": "Rock", "songCount": 3 })),
            "<root songCount=\"3\">Rock</root>"
        );
    }

    #[test]
    fn scalar_arrays_repeat_the_element() {
        assert_eq!(
            xml(json!({ "versions": [1, 2] })),
            "<root><versions>1</versions><versions>2</versions></root>"
        );
    }

    #[test]
    fn escapes_markup_and_drops_forbidden_characters() {
        let out = xml(json!({ "t": "a&b <c> \"d\"\n\u{1}e", "value": "x&y\u{0}<" }));
        assert_eq!(
            out,
            "<root t=\"a&amp;b &lt;c&gt; &quot;d&quot;&#10;e\">x&amp;y&lt;</root>"
        );
    }

    #[test]
    fn null_members_are_skipped() {
        assert_eq!(xml(json!({ "a": null })), "<root/>");
    }

    #[test]
    fn error_envelope_carries_code_and_message() {
        let map = {
            let mut envelope = envelope("failed");
            envelope.insert("error".into(), json!({ "code": 70, "message": "gone" }));
            envelope
        };
        let out = xml_document(&map);
        assert!(out.contains("xmlns=\"http://subsonic.org/restapi\""));
        assert!(out.contains("<error code=\"70\" message=\"gone\"/>"));
    }
}
