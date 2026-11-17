//! What the live link tells, in words and as JSON: events of the game's
//! journal, the breakpoints and whether the game stands, the free camera.
//! The command line prints these and the MCP server answers with them, so
//! both say the same.

use serde_json::json;

use super::names::LiveNames;
use super::proto::journal::{EventBody, MessageOrigin};
use super::proto::{self, Event};

/// The name of a message: from the index, else the hash.
pub fn message_name(names: Option<&LiveNames>, hash: u32) -> String {
    names
        .and_then(|n| n.message(hash))
        .map_or_else(|| format!("{hash:#010x}"), str::to_owned)
}

/// `{GUID} name` of an object: the name from the index when known.
pub fn object_label(names: Option<&LiveNames>, guid: &proto::Guid) -> String {
    match names.and_then(|n| n.object(guid)) {
        Some(o) if !o.name.is_empty() => format!("{} {}", o.name, proto::guid_hex(guid)),
        _ => proto::guid_hex(guid),
    }
}

/// The lines of one event.
pub fn event_lines(e: &Event, names: Option<&LiveNames>) -> Vec<String> {
    let head = format!(
        "#{} frame {} {:>8.1}s",
        e.seq,
        e.frame,
        f64::from(e.sim_time_ms) / 1000.0
    );
    match &e.body {
        EventBody::Output {
            source,
            text,
            script,
            line,
        } => {
            let at = if script.is_empty() {
                String::new()
            } else {
                format!(" {script}:{line}:")
            };
            vec![format!("{head} {}{at} {text}", source.name())]
        }
        EventBody::LuaError {
            message, traceback, ..
        } => {
            let mut out = vec![format!("{head} lua error: {message}")];
            out.extend(traceback.iter().map(|t| format!("    {t}")));
            out
        }
        EventBody::Message {
            hash,
            data,
            forced,
            listeners,
            receivers,
            origin,
            from,
        } => {
            let mut line = format!(
                "{head} message {}{}{} -> {listeners} listener(s)",
                message_name(names, *hash),
                if *data != 0 {
                    format!(" data {data:#x}")
                } else {
                    String::new()
                },
                if *forced { " forced" } else { "" },
            );
            if !receivers.is_empty() {
                let list: Vec<String> = receivers.iter().map(|g| object_label(names, g)).collect();
                line += &format!(": {}", list.join(", "));
            }
            if *origin == MessageOrigin::Link {
                line += " (live link)";
            }
            if let Some(f) = from {
                if f.direct {
                    line += " [to one listener]";
                }
                if let Some(g) = &f.sender {
                    line += &format!(" from {}", object_label(names, g));
                }
                if f.cause != 0 {
                    line += &format!(" on {}", message_name(names, f.cause));
                }
                if f.site != 0 {
                    line += &format!(" at {:#010x}", f.site);
                }
                if let Some(g) = &f.data_object {
                    line += &format!(" data object {}", object_label(names, g));
                }
            }
            vec![line]
        }
        EventBody::MessageCounts { window_ms, counts } => {
            let mut top = counts.clone();
            top.sort_by_key(|x| std::cmp::Reverse(x.1));
            let list: Vec<String> = top
                .iter()
                .map(|(h, n)| format!("{} x{n}", message_name(names, *h)))
                .collect();
            vec![format!(
                "{head} messages without own events in {:.1} s: {}",
                f64::from(*window_ms) / 1000.0,
                list.join(", ")
            )]
        }
    }
}

/// One line about breakpoints and whether the game stands.
pub fn debug_lines(d: &proto::DebugState, names: Option<&LiveNames>) -> Vec<String> {
    let mut out = Vec::new();
    for r in &d.rules {
        let once = if r.once { " (once)" } else { "" };
        out.push(match &r.on {
            proto::BreakOn::Message { hash, receiver } => format!(
                "break on message {}{}{once}",
                message_name(names, *hash),
                receiver
                    .map(|g| format!(" to {}", object_label(names, &g)))
                    .unwrap_or_default()
            ),
            proto::BreakOn::Output { text } => format!("break on print `{text}`{once}"),
            proto::BreakOn::LuaError => format!("break on Lua errors{once}"),
        });
    }
    if d.rules.is_empty() {
        out.push("no breakpoints".into());
    }
    match &d.stop {
        Some(stop) => {
            let why = match &stop.reason {
                proto::StopReason::Pause => "pause".to_owned(),
                proto::StopReason::Step => "step".to_owned(),
                proto::StopReason::Message {
                    hash, receivers, ..
                } => {
                    let to: Vec<String> =
                        receivers.iter().map(|g| object_label(names, g)).collect();
                    format!(
                        "message {} to {}",
                        message_name(names, *hash),
                        if to.is_empty() {
                            "-".to_owned()
                        } else {
                            to.join(", ")
                        }
                    )
                }
                proto::StopReason::Output { text, script, line } => {
                    format!("print {script}:{line}: {text}")
                }
                proto::StopReason::LuaError { message } => format!("Lua error: {message}"),
            };
            out.push(format!(
                "STOPPED in the {} at frame {}, {:.1} s game time: {why}; journal #{}; for {:.1} s, goes on by itself in {:.0} s",
                stop.place.name(),
                stop.frame,
                f64::from(stop.sim_time_ms) / 1000.0,
                stop.seq,
                f64::from(stop.held_ms) / 1000.0,
                f64::from(stop.resume_in_ms) / 1000.0
            ));
        }
        None if d.pending => out.push("running; stops in the next level frame".into()),
        None => out.push("running".into()),
    }
    out.push(format!(
        "{} hit(s); a stop lasts at most {} s{}",
        d.hits,
        d.hold_limit_s,
        if d.camera.is_some() {
            "; free camera on"
        } else {
            ""
        }
    ));
    out
}

/// One event as JSON.
pub fn event_json(e: &Event, names: Option<&LiveNames>) -> serde_json::Value {
    let mut v = json!({"seq": e.seq, "frame": e.frame, "sim_time_ms": e.sim_time_ms});
    match &e.body {
        EventBody::Output {
            source,
            text,
            script,
            line,
        } => {
            v["kind"] = json!("output");
            v["source"] = json!(source.name());
            v["text"] = json!(text);
            if !script.is_empty() {
                v["script"] = json!(script);
                v["line"] = json!(line);
            }
        }
        EventBody::LuaError {
            status,
            message,
            chunk,
            line,
            traceback,
        } => {
            v["kind"] = json!("lua_error");
            v["status"] = json!(status);
            v["message"] = json!(message);
            v["chunk"] = json!(chunk);
            v["line"] = json!(line);
            v["traceback"] = json!(traceback);
        }
        EventBody::Message {
            hash,
            data,
            forced,
            listeners,
            receivers,
            origin,
            from,
        } => {
            v["kind"] = json!("message");
            v["hash"] = json!(format!("{hash:#010x}"));
            v["name"] = json!(names.and_then(|n| n.message(*hash)));
            v["data"] = json!(data);
            v["forced"] = json!(forced);
            v["listeners"] = json!(listeners);
            v["receivers"] = json!(
                receivers
                    .iter()
                    .map(|g| json!({
                        "guid": proto::guid_hex(g),
                        "name": names.and_then(|n| n.object(g)).map(|o| o.name.clone()),
                    }))
                    .collect::<Vec<_>>()
            );
            v["by_link"] = json!(*origin == MessageOrigin::Link);
            if let Some(f) = from {
                let object = |g: &proto::Guid| {
                    json!({
                        "guid": proto::guid_hex(g),
                        "name": names.and_then(|n| n.object(g)).map(|o| o.name.clone()),
                    })
                };
                v["from"] = json!({
                    "site": format!("{:#010x}", f.site),
                    "cause": if f.cause != 0 { json!(format!("{:#010x}", f.cause)) } else { serde_json::Value::Null },
                    "cause_name": names.and_then(|n| n.message(f.cause)),
                    "sender": f.sender.as_ref().map(object),
                    "data_object": f.data_object.as_ref().map(object),
                    "direct": f.direct,
                });
            }
        }
        EventBody::MessageCounts { window_ms, counts } => {
            v["kind"] = json!("message_counts");
            v["window_ms"] = json!(window_ms);
            v["counts"] = json!(
                counts
                    .iter()
                    .map(|(h, n)| json!({
                        "hash": format!("{h:#010x}"),
                        "name": names.and_then(|x| x.message(*h)),
                        "count": n,
                    }))
                    .collect::<Vec<_>>()
            );
        }
    }
    v
}

/// A free camera at `at` (else where `now` is), turned to `yaw` and
/// `pitch` in degrees (else as `now` looks).
pub fn free_camera(
    now: proto::CameraView,
    at: Option<[f32; 3]>,
    yaw: Option<f32>,
    pitch: Option<f32>,
) -> proto::CameraView {
    let f = now.pose.forward;
    let now_yaw = f[0].atan2(f[2]);
    let now_pitch = f[1].clamp(-1.0, 1.0).asin();
    let y = yaw.map_or(now_yaw, f32::to_radians);
    let p = pitch.map_or(now_pitch, f32::to_radians).clamp(-1.55, 1.55);
    let forward = [y.sin() * p.cos(), p.sin(), y.cos() * p.cos()];
    let pose = super::pose_looking(at.unwrap_or(now.pose.position), forward);
    proto::CameraView { pose, fov: now.fov }
}

/// The breakpoints and whether the game stands, as JSON.
pub fn debug_json(d: &proto::DebugState, names: Option<&LiveNames>) -> serde_json::Value {
    json!({
        "lines": debug_lines(d, names),
        "stopped": d.stop.is_some(),
        "place": d.stop.as_ref().map(|s| s.place.name()),
        "frame": d.stop.as_ref().map(|s| s.frame),
        "journal_seq": d.stop.as_ref().map(|s| s.seq),
        "rules": d.rules.len(),
        "hits": d.hits,
        "pending": d.pending,
        "hold_limit_s": d.hold_limit_s,
        "free_camera": d.camera.is_some(),
    })
}
