//! `search`, `resource_info`, `lookup_hash` and `strings`: the game index.

use serde_json::{Value, json};

use doctor_toolkit::navigation::{HashFn, Query, search as nav_search};
use doctor_toolkit::resource::names::{mem_policies, type_tags};

use super::{
    LookupHashParams, Output, ResourceParams, SearchParams, StringsParams, brief, clamp, hex,
    parse_hash, resolve,
};
use crate::state::{ToolResult, Toolkit, fail};

/// Results of a query of the game index.
pub fn search(kit: &Toolkit, p: SearchParams) -> ToolResult<Output> {
    let query = Query::parse(&p.query)?;
    if query.is_empty() {
        return fail("empty search; see the description of `query`");
    }
    let offset = p.offset.unwrap_or(0);
    let limit = clamp(p.limit, 30, 200);
    let index = kit.index();
    let (results, total) = nav_search::search(index, &query, offset + limit);
    let items: Vec<Value> = results
        .iter()
        .skip(offset)
        .map(|r| {
            let mut item = brief(kit, r.record);
            item["hits"] = json!(r.hits.iter().map(|h| h.describe()).collect::<Vec<_>>());
            item
        })
        .collect();
    Ok(Output::Json(json!({
        "query": p.query,
        "total": total,
        "offset": offset,
        "more": offset + items.len() < total,
        "results": items,
    })))
}

/// Everything the index knows about one resource.
pub fn resource_info(kit: &Toolkit, p: ResourceParams) -> ToolResult<Output> {
    const NAMES: usize = 60;
    const REFS: usize = 60;
    let id = resolve(kit, &p.resource)?;
    let index = kit.index();
    let dict = kit.dictionary();
    let record = index.record(id);
    let h = &record.header;
    let key = index.key(id);
    let mut out = brief(kit, id);
    out["type_id"] = json!(hex(h.type_id));
    out["ids"] = json!(h.ids.iter().map(|&i| hex(i)).collect::<Vec<_>>());
    out["data_size"] = json!(h.data_size);
    out["stored_size"] = json!(record.stored_size);
    out["compressed"] = json!(record.compressed);
    out["mem_policy"] = json!(mem_policies().display(h.mem_policy));
    out["alignment"] = json!(h.alignment);
    let names: Vec<Value> = record
        .names
        .iter()
        .take(NAMES)
        .map(|(kind, name)| json!({ "kind": kind.label(), "name": name }))
        .collect();
    out["names"] = json!({ "total": record.names.len(), "items": names });
    let hashes: Vec<Value> = record
        .hashes
        .iter()
        .take(NAMES)
        .map(|(kind, hash)| {
            let known: Vec<&str> = dict
                .lookup(*hash)
                .into_iter()
                .map(|(n, _)| n)
                .take(3)
                .collect();
            json!({ "kind": kind.label(), "hash": hex(*hash), "names": known })
        })
        .collect();
    out["hashes"] = json!({ "total": record.hashes.len(), "items": hashes });
    let texts: Vec<Value> = record
        .texts
        .iter()
        .take(NAMES)
        .map(|t| {
            json!({
                "hash": hex(t.hash),
                "language": record.languages.get(t.language as usize).map_or("", String::as_str),
                "text": t.text,
            })
        })
        .collect();
    out["texts"] = json!({ "total": record.texts.len(), "items": texts });
    let uses: Vec<(&doctor_toolkit::navigation::Ref, &[_])> = index.uses(id).collect();
    let uses_total = uses.len();
    let uses: Vec<Value> = uses
        .into_iter()
        .take(REFS)
        .map(|(reference, targets)| {
            json!({
                "label": reference.label,
                "target": reference.target.describe(),
                "resolved": targets.iter().take(4).map(|&t| index.key(t).to_string()).collect::<Vec<_>>(),
                "resolved_total": targets.len(),
            })
        })
        .collect();
    out["uses"] = json!({ "total": uses_total, "items": uses });
    let used_by = index.used_by(id);
    let used: Vec<Value> = used_by
        .iter()
        .take(REFS)
        .map(|&(source, k)| {
            let mut item = brief(kit, source);
            item["ref"] = json!(index.record(source).refs[k as usize].label);
            item
        })
        .collect();
    out["used_by"] = json!({ "total": used_by.len(), "items": used });
    let copies = index.copies(id);
    out["copies"] = json!({
        "total": copies.len(),
        "items": copies.iter().take(20).map(|&c| index.key(c).to_string()).collect::<Vec<_>>(),
    });
    out["error"] = json!(record.error);
    out["json_view"] = json!(super::json::view_name(h.type_id));
    let ws = kit.workspace()?;
    out["changed_in_workspace"] = json!(ws.as_ref().is_some_and(|w| w.change(&key).is_some()));
    Ok(Output::Json(out))
}

/// Names of a hash, or the hashes of a name.
pub fn lookup_hash(kit: &Toolkit, p: LookupHashParams) -> ToolResult<Output> {
    if p.hash.is_none() && p.text.is_none() {
        return fail("give `hash` or `text`");
    }
    let index = kit.index();
    let occurrences = |hash: u32| -> ToolResult<Value> {
        let query = Query::parse(&format!("0x{hash:08x}"))?;
        let (results, total) = nav_search::search(index, &query, 10);
        Ok(json!({
            "resources": total,
            "first": results.iter().map(|r| brief(kit, r.record)).collect::<Vec<_>>(),
        }))
    };
    let mut out = json!({});
    if let Some(text) = &p.hash {
        let hash = parse_hash(text)?;
        let names: Vec<Value> = kit
            .dictionary()
            .lookup(hash)
            .into_iter()
            .take(50)
            .map(|(name, origin)| json!({ "name": name, "origin": origin.describe() }))
            .collect();
        out["hash"] = json!({
            "hash": hex(hash),
            "decimal": hash,
            "type_tag": type_tags().get(hash),
            "names": names,
            "occurs_in": occurrences(hash)?,
        });
    }
    if let Some(text) = &p.text {
        let mut functions = Vec::new();
        for f in HashFn::ALL {
            let hash = f.apply(text);
            let (_, total) = nav_search::search(index, &Query::parse(&format!("0x{hash:08x}"))?, 1);
            functions.push(json!({ "function": f.name(), "hash": hex(hash), "resources": total }));
        }
        out["text"] = json!({ "text": text, "hashes": functions });
    }
    Ok(Output::Json(out))
}

/// Localised strings by text or entry hash.
pub fn strings(kit: &Toolkit, p: StringsParams) -> ToolResult<Output> {
    let needle = p.text.as_deref().map(str::to_lowercase);
    let hash = p.hash.as_deref().map(parse_hash).transpose()?;
    if needle.is_none() && hash.is_none() {
        return fail("give `text` or `hash`");
    }
    let language = p.language.as_deref().map(str::to_ascii_lowercase);
    let offset = p.offset.unwrap_or(0);
    let limit = clamp(p.limit, 50, 300);
    let index = kit.index();
    let mut total = 0usize;
    let mut items = Vec::new();
    for id in index.ids() {
        let record = index.record(id);
        if record.texts.is_empty() {
            continue;
        }
        for t in &record.texts {
            let lang = record
                .languages
                .get(t.language as usize)
                .map_or("", String::as_str);
            if language
                .as_deref()
                .is_some_and(|l| !lang.eq_ignore_ascii_case(l))
            {
                continue;
            }
            if hash.is_some_and(|h| h != t.hash) {
                continue;
            }
            if let Some(n) = &needle
                && !t.text.to_lowercase().contains(n.as_str())
            {
                continue;
            }
            total += 1;
            if total > offset && items.len() < limit {
                items.push(json!({
                    "table": index.key(id).to_string(),
                    "hash": hex(t.hash),
                    "language": lang,
                    "text": t.text,
                }));
            }
        }
    }
    Ok(Output::Json(json!({
        "total": total,
        "offset": offset,
        "more": offset + items.len() < total,
        "strings": items,
    })))
}
