//! XLIFF 1.2 and 2.0 parser + surgical patcher.
//!
//! # Design
//!
//! We expose two operations:
//!
//! * [`parse`] — a cheap, read-only semantic view used by the diff engine
//!   (what's translated, what's not, what's final). Lossy on purpose: it
//!   throws away attributes Engo doesn't care about.
//!
//! * [`patch`] — a *streaming rewrite* that copies every event through
//!   unchanged except for the `<target>` content of patched units. All other
//!   XML (comments, whitespace, unknown child elements, namespaces, XLIFF 2.0
//!   inline tags) survives round-trip byte-for-byte when we can manage it,
//!   and semantically in every case.
//!
//! The patcher buffers events only within a single `<trans-unit>` / `<unit>`
//! so it can update the enclosing segment's `state` attribute after deciding
//! whether any contained target was changed. Memory is bounded by the size of
//! one unit, which is typically tiny.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Cursor;

use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::name::QName;
use quick_xml::{Reader, Writer};

use super::UnitState;
use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XliffVersion {
    V1_2,
    V2_0,
}

impl XliffVersion {
    fn from_attr(v: &str) -> XliffVersion {
        match v.trim() {
            "2.0" | "2" => XliffVersion::V2_0,
            _ => XliffVersion::V1_2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransUnit {
    pub id: String,
    pub source: String,
    pub target: Option<String>,
    pub state: UnitState,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XliffView {
    pub version: XliffVersion,
    pub source_lang: Option<String>,
    pub target_lang: Option<String>,
    pub units: Vec<TransUnit>,
}

/// Map an XLIFF state string to our semantic bucket.
fn state_from_attr(s: &str, version: XliffVersion) -> UnitState {
    match (version, s) {
        (XliffVersion::V1_2, "needs-translation")
        | (XliffVersion::V1_2, "new")
        | (XliffVersion::V1_2, "needs-adaptation")
        | (XliffVersion::V1_2, "needs-l10n")
        | (XliffVersion::V1_2, "needs-review-translation") => UnitState::NeedsTranslation,
        (XliffVersion::V1_2, "translated") => UnitState::Translated,
        (XliffVersion::V1_2, "final") | (XliffVersion::V1_2, "signed-off") => UnitState::Final,

        (XliffVersion::V2_0, "initial") => UnitState::NeedsTranslation,
        (XliffVersion::V2_0, "translated") => UnitState::Translated,
        (XliffVersion::V2_0, "reviewed") | (XliffVersion::V2_0, "final") => UnitState::Final,

        _ => UnitState::Other,
    }
}

fn local<'a>(qname: QName<'a>) -> &'a [u8] {
    let bytes: &'a [u8] = qname.into_inner();
    match bytes.iter().position(|&b| b == b':') {
        Some(i) => &bytes[i + 1..],
        None => bytes,
    }
}

fn attr_value(elem: &BytesStart<'_>, key: &[u8]) -> Result<Option<String>> {
    for a in elem.attributes() {
        let a = a?;
        if local(a.key) == key {
            return Ok(Some(a.unescape_value()?.into_owned()));
        }
    }
    Ok(None)
}

/// Parse an XLIFF document into its semantic view.
///
/// Accepts both XLIFF 1.2 and 2.0. `source`/`target`/`note` text is recovered
/// as concatenated UTF-8 text (inline placeholder tags like `<ph/>` are *not*
/// expanded — their text content is ignored for the semantic view; the
/// patcher preserves them on write).
pub fn parse(xml: &[u8]) -> Result<XliffView> {
    let mut reader = Reader::from_reader(xml);
    reader.trim_text(false);
    let mut buf = Vec::new();

    let mut version: Option<XliffVersion> = None;
    let mut source_lang: Option<String> = None;
    let mut target_lang: Option<String> = None;
    let mut units: Vec<TransUnit> = Vec::new();

    // Current-unit parsing state.
    let mut cur: Option<PendingUnit> = None;
    let mut in_source_depth = 0u32;
    let mut in_target_depth = 0u32;
    let mut in_note = false;
    let mut cur_note = String::new();

    loop {
        let evt = reader.read_event_into(&mut buf)?;
        match &evt {
            Event::Start(e) => {
                let name = local(e.name());
                match name {
                    b"xliff" => {
                        if let Some(v) = attr_value(e, b"version")? {
                            version = Some(XliffVersion::from_attr(&v));
                        }
                        if let Some(v) = attr_value(e, b"srcLang")? {
                            source_lang = Some(v);
                        }
                        if let Some(v) = attr_value(e, b"trgLang")? {
                            target_lang = Some(v);
                        }
                    }
                    b"file" => {
                        if let Some(v) = attr_value(e, b"source-language")? {
                            source_lang.get_or_insert(v);
                        }
                        if let Some(v) = attr_value(e, b"target-language")? {
                            target_lang.get_or_insert(v);
                        }
                    }
                    b"trans-unit" | b"unit" => {
                        let id = attr_value(e, b"id")?.unwrap_or_default();
                        cur = Some(PendingUnit::new(id));
                    }
                    b"segment" if cur.is_some() => {
                        // XLIFF 2.0 carries state on <segment>.
                        if let Some(s) = attr_value(e, b"state")? {
                            let v = version.unwrap_or(XliffVersion::V2_0);
                            if let Some(pu) = cur.as_mut() {
                                pu.state = Some(state_from_attr(&s, v));
                            }
                        }
                    }
                    b"source" if cur.is_some() => {
                        in_source_depth += 1;
                    }
                    b"target" if cur.is_some() => {
                        in_target_depth += 1;
                        if let Some(pu) = cur.as_mut() {
                            pu.has_target = true;
                        }
                        // XLIFF 1.2 carries state on <target>.
                        if let Some(s) = attr_value(e, b"state")? {
                            let v = version.unwrap_or(XliffVersion::V1_2);
                            if let Some(pu) = cur.as_mut() {
                                pu.state = Some(state_from_attr(&s, v));
                            }
                        }
                    }
                    b"note" if cur.is_some() => {
                        in_note = true;
                        cur_note.clear();
                    }
                    _ => {}
                }
            }
            Event::Empty(e) => {
                let name = local(e.name());
                if name == b"target" && cur.is_some() {
                    if let Some(pu) = cur.as_mut() {
                        pu.has_target = true;
                        pu.target = Some(String::new());
                        if let Some(s) = attr_value(e, b"state")? {
                            let v = version.unwrap_or(XliffVersion::V1_2);
                            pu.state = Some(state_from_attr(&s, v));
                        }
                    }
                }
            }
            Event::End(e) => {
                let name = local(e.name());
                match name {
                    b"source" if in_source_depth > 0 => in_source_depth -= 1,
                    b"target" if in_target_depth > 0 => in_target_depth -= 1,
                    b"note" if in_note => {
                        in_note = false;
                        if let Some(pu) = cur.as_mut() {
                            pu.notes.push(std::mem::take(&mut cur_note));
                        }
                    }
                    b"trans-unit" | b"unit" => {
                        if let Some(pu) = cur.take() {
                            units.push(pu.into_unit());
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                let text = t.unescape()?;
                if in_note {
                    cur_note.push_str(&text);
                } else if let Some(pu) = cur.as_mut() {
                    if in_source_depth > 0 {
                        pu.source.push_str(&text);
                    } else if in_target_depth > 0 {
                        pu.target.get_or_insert_with(String::new).push_str(&text);
                    }
                }
            }
            Event::CData(c) => {
                let s = std::str::from_utf8(c.as_ref())?;
                if in_note {
                    cur_note.push_str(s);
                } else if let Some(pu) = cur.as_mut() {
                    if in_source_depth > 0 {
                        pu.source.push_str(s);
                    } else if in_target_depth > 0 {
                        pu.target.get_or_insert_with(String::new).push_str(s);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    let version = version
        .ok_or_else(|| Error::Format("missing <xliff version=\"…\"> root element".into()))?;

    Ok(XliffView {
        version,
        source_lang,
        target_lang,
        units,
    })
}

struct PendingUnit {
    id: String,
    source: String,
    target: Option<String>,
    state: Option<UnitState>,
    notes: Vec<String>,
    has_target: bool,
}

impl PendingUnit {
    fn new(id: String) -> Self {
        Self {
            id,
            source: String::new(),
            target: None,
            state: None,
            notes: Vec::new(),
            has_target: false,
        }
    }

    fn into_unit(self) -> TransUnit {
        // Default state: if the target is missing or empty, we treat the unit
        // as needing translation. Otherwise, if XLIFF didn't tell us, assume
        // the author translated it (pragmatic default — real state is usually
        // present in well-formed XLIFF).
        let default_state = match &self.target {
            Some(t) if !t.is_empty() => UnitState::Translated,
            _ => UnitState::NeedsTranslation,
        };
        TransUnit {
            id: self.id,
            source: self.source,
            target: if self.has_target {
                self.target.or(Some(String::new()))
            } else {
                None
            },
            state: self.state.unwrap_or(default_state),
            notes: self.notes,
        }
    }
}

/// Rewrite `xml` so that the `<target>` content of every id in `patches` is
/// replaced by the new translation. All other bytes are preserved to the
/// greatest extent possible.
///
/// When patching, the enclosing unit's state is updated:
/// * XLIFF 1.2 — `<target state="needs-translation">` becomes `state="translated"`.
/// * XLIFF 2.0 — `<segment state="initial">` becomes `state="translated"`.
///
/// If a patched unit has no `<target>` element at all, one is inserted
/// immediately after `</source>` (1.2) or before `</segment>` (2.0).
pub fn patch(xml: &[u8], patches: &HashMap<String, String>) -> Result<Vec<u8>> {
    if patches.is_empty() {
        return Ok(xml.to_vec());
    }

    let version = parse(xml)?.version;

    let mut reader = Reader::from_reader(xml);
    reader.trim_text(false);
    let mut buf = Vec::new();

    let mut out = Cursor::new(Vec::<u8>::with_capacity(xml.len()));
    let mut writer = Writer::new(&mut out);

    // Buffered events within a trans-unit/unit. When the unit closes we
    // decide whether to rewrite it and then flush.
    let mut unit_buf: Option<Vec<Event<'static>>> = None;
    let mut unit_id: Option<String> = None;

    loop {
        let evt = reader.read_event_into(&mut buf)?;
        let is_eof = matches!(evt, Event::Eof);

        // Decide if this event opens or closes a trans-unit/unit.
        match &evt {
            Event::Start(e) => {
                let name = local(e.name());
                if matches!(name, b"trans-unit" | b"unit") {
                    unit_id = attr_value(e, b"id")?;
                    unit_buf = Some(Vec::new());
                }
            }
            Event::End(e) => {
                let name = local(e.name());
                if matches!(name, b"trans-unit" | b"unit") {
                    let mut events = unit_buf.take().unwrap_or_default();
                    events.push(evt.clone().into_owned());
                    let id = unit_id.take();
                    let new_target = id.as_deref().and_then(|i| patches.get(i));
                    let emitted = if let Some(nt) = new_target {
                        rewrite_unit(events, nt, version)?
                    } else {
                        events
                    };
                    for e in emitted {
                        writer.write_event(e)?;
                    }
                    buf.clear();
                    continue;
                }
            }
            _ => {}
        }

        if let Some(ub) = unit_buf.as_mut() {
            ub.push(evt.clone().into_owned());
        } else {
            writer.write_event(evt)?;
        }

        if is_eof {
            break;
        }
        buf.clear();
    }

    Ok(out.into_inner())
}

/// Rewrite a buffered unit's events to inject `new_target`.
fn rewrite_unit(
    events: Vec<Event<'static>>,
    new_target: &str,
    version: XliffVersion,
) -> Result<Vec<Event<'static>>> {
    let mut out: Vec<Event<'static>> = Vec::with_capacity(events.len() + 4);
    let mut i = 0;
    let mut target_handled = false;

    while i < events.len() {
        let e = &events[i];
        match e {
            Event::Start(start) if local(start.name()) == b"target" => {
                let updated_start = update_target_state_attr(start, version)?;
                out.push(Event::Start(updated_start));
                out.push(Event::Text(BytesText::from_escaped(
                    escape_text(new_target).into_owned(),
                )));
                // Skip to matching </target>.
                let mut depth = 1u32;
                i += 1;
                while i < events.len() && depth > 0 {
                    match &events[i] {
                        Event::Start(s) if local(s.name()) == b"target" => depth += 1,
                        Event::End(end) if local(end.name()) == b"target" => {
                            depth -= 1;
                            if depth == 0 {
                                out.push(Event::End(end.clone().into_owned()));
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                target_handled = true;
                continue;
            }
            Event::Empty(start) if local(start.name()) == b"target" => {
                let updated_start = update_target_state_attr(start, version)?;
                out.push(Event::Start(updated_start));
                out.push(Event::Text(BytesText::from_escaped(
                    escape_text(new_target).into_owned(),
                )));
                out.push(Event::End(BytesEnd::new("target")));
                target_handled = true;
            }
            Event::Start(start)
                if version == XliffVersion::V2_0 && local(start.name()) == b"segment" =>
            {
                out.push(Event::Start(update_segment_state_attr(start)?));
            }
            _ => out.push(e.clone()),
        }
        i += 1;
    }

    if !target_handled {
        // No <target> was present. Insert one — placement depends on version.
        let new_target_events = vec![
            Event::Start({
                let mut bs = BytesStart::new("target");
                if version == XliffVersion::V1_2 {
                    bs.push_attribute(("state", "translated"));
                }
                bs
            }),
            Event::Text(BytesText::from_escaped(
                escape_text(new_target).into_owned(),
            )),
            Event::End(BytesEnd::new("target")),
        ];
        let insert_at = match version {
            XliffVersion::V1_2 => position_after_source_end(&out),
            XliffVersion::V2_0 => position_before_segment_end(&out),
        };
        let insert_at = insert_at.unwrap_or(out.len().saturating_sub(1));
        for (offset, ev) in new_target_events.into_iter().enumerate() {
            out.insert(insert_at + offset, ev);
        }
    }

    Ok(out)
}

fn update_target_state_attr(
    start: &BytesStart<'_>,
    version: XliffVersion,
) -> Result<BytesStart<'static>> {
    if version != XliffVersion::V1_2 {
        return Ok(start.clone().into_owned());
    }
    replace_attr(start, b"state", "translated")
}

fn update_segment_state_attr(start: &BytesStart<'_>) -> Result<BytesStart<'static>> {
    replace_attr(start, b"state", "translated")
}

/// Return a clone of `start` with `key` set to `new_value`, replacing or adding.
fn replace_attr(
    start: &BytesStart<'_>,
    key: &[u8],
    new_value: &str,
) -> Result<BytesStart<'static>> {
    let name = start.name().as_ref().to_vec();
    let mut new_elem = BytesStart::new(std::str::from_utf8(&name)?.to_string());
    let mut replaced = false;
    for a in start.attributes() {
        let a = a?;
        if local(a.key) == key {
            new_elem.push_attribute(Attribute {
                key: a.key,
                value: Cow::Owned(new_value.as_bytes().to_vec()),
            });
            replaced = true;
        } else {
            new_elem.push_attribute(a);
        }
    }
    if !replaced {
        new_elem.push_attribute((std::str::from_utf8(key)?, new_value));
    }
    Ok(new_elem)
}

fn position_after_source_end(events: &[Event<'static>]) -> Option<usize> {
    for (i, e) in events.iter().enumerate() {
        if let Event::End(end) = e {
            if local(end.name()) == b"source" {
                return Some(i + 1);
            }
        }
        if let Event::Empty(st) = e {
            if local(st.name()) == b"source" {
                return Some(i + 1);
            }
        }
    }
    None
}

fn position_before_segment_end(events: &[Event<'static>]) -> Option<usize> {
    for (i, e) in events.iter().enumerate().rev() {
        if let Event::End(end) = e {
            if local(end.name()) == b"segment" {
                return Some(i);
            }
        }
    }
    None
}

/// Escape text for safe inclusion as XML character data.
fn escape_text(s: &str) -> Cow<'_, str> {
    // Only `&`, `<`, `>` need escaping in text nodes. We intentionally do *not*
    // escape quotes — they're only special in attribute values.
    let needs = s
        .as_bytes()
        .iter()
        .any(|&b| matches!(b, b'<' | b'>' | b'&'));
    if !needs {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            other => out.push(other),
        }
    }
    Cow::Owned(out)
}
