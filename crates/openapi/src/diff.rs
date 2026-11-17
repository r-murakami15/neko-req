//! 同期の差分。前回の Operation と今回の Operation を突き合わせて、追加・変更・削除を出す。
//! operationId が変わったものは `METHOD path` が同じ API として引き継ぐ。

use std::collections::HashSet;

use nekoreq_core::{match_operations, OpKey, Operation, ParamSpec, SecurityRequirement};

use crate::model::{ChangedEntry, ChangedField, DiffEntry, FieldChange, SyncDiff};

fn entry(op: &Operation, drafts: &HashSet<OpKey>) -> DiffEntry {
    DiffEntry {
        op_key: op.op_key.clone(),
        operation_id: op.operation_id.clone(),
        method: op.method.clone(),
        path: op.path.clone(),
        summary: op.summary.clone(),
        tag: op.tags.first().cloned(),
        has_draft: drafts.contains(&op.op_key),
    }
}

fn param_id(p: &ParamSpec) -> String {
    format!("{:?}:{}", p.location, p.name)
}

fn join(names: &[String]) -> String {
    names.join("、")
}

fn note_parts(parts: Vec<(&str, Vec<String>)>) -> String {
    parts
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(label, v)| format!("{label}: {}", join(&v)))
        .collect::<Vec<_>>()
        .join(" / ")
}

fn security_text(s: &Option<Vec<SecurityRequirement>>) -> String {
    match s {
        None => "全体の設定に従う".to_string(),
        Some(v) if v.is_empty() => "認証なし".to_string(),
        Some(v) => v
            .iter()
            .map(|r| {
                r.schemes
                    .iter()
                    .map(|s| s.name.clone())
                    .collect::<Vec<_>>()
                    .join(" + ")
            })
            .collect::<Vec<_>>()
            .join(" または "),
    }
}

/// 前回の `a` から今回の `b` への、パラメータの変更の説明と、消えたパラメータの名前。
fn param_changes(a: &Operation, b: &Operation) -> Option<(String, Vec<String>)> {
    if a.parameters == b.parameters {
        return None;
    }
    let ids_a: Vec<String> = a.parameters.iter().map(param_id).collect();
    let ids_b: Vec<String> = b.parameters.iter().map(param_id).collect();
    let added: Vec<String> = b
        .parameters
        .iter()
        .filter(|p| !ids_a.contains(&param_id(p)))
        .map(|p| p.name.clone())
        .collect();
    let removed: Vec<String> = a
        .parameters
        .iter()
        .filter(|p| !ids_b.contains(&param_id(p)))
        .map(|p| p.name.clone())
        .collect();
    let changed: Vec<String> = b
        .parameters
        .iter()
        .filter(|p| {
            a.parameters
                .iter()
                .any(|q| param_id(q) == param_id(p) && q != *p)
        })
        .map(|p| p.name.clone())
        .collect();
    let mut note = note_parts(vec![
        ("追加", added),
        ("削除", removed.clone()),
        ("変更", changed),
    ]);
    if note.is_empty() {
        note = "並び順が変わりました".to_string();
    }
    Some((note, removed))
}

fn request_body_note(a: &Operation, b: &Operation) -> Option<String> {
    if a.request_body == b.request_body {
        return None;
    }
    Some(match (&a.request_body, &b.request_body) {
        (None, Some(_)) => "ボディが追加されました".to_string(),
        (Some(_), None) => "ボディが削除されました".to_string(),
        (Some(x), Some(y)) => {
            let cts = |r: &nekoreq_core::RequestBodySpec| -> Vec<String> {
                r.content.iter().map(|m| m.content_type.clone()).collect()
            };
            let (cx, cy) = (cts(x), cts(y));
            let added: Vec<String> = cy.iter().filter(|c| !cx.contains(c)).cloned().collect();
            let removed: Vec<String> = cx.iter().filter(|c| !cy.contains(c)).cloned().collect();
            let mut note = note_parts(vec![
                ("Content-Type 追加", added),
                ("Content-Type 削除", removed),
            ]);
            if x.required != y.required {
                if !note.is_empty() {
                    note.push_str(" / ");
                }
                note.push_str(if y.required {
                    "必須になりました"
                } else {
                    "必須ではなくなりました"
                });
            }
            if note.is_empty() {
                note = "スキーマまたは例が変わりました".to_string();
            }
            note
        }
        (None, None) => return None,
    })
}

fn responses_note(a: &Operation, b: &Operation) -> Option<String> {
    if a.responses == b.responses {
        return None;
    }
    let sa: Vec<String> = a.responses.iter().map(|r| r.status.clone()).collect();
    let sb: Vec<String> = b.responses.iter().map(|r| r.status.clone()).collect();
    let added: Vec<String> = sb.iter().filter(|s| !sa.contains(s)).cloned().collect();
    let removed: Vec<String> = sa.iter().filter(|s| !sb.contains(s)).cloned().collect();
    let changed: Vec<String> = b
        .responses
        .iter()
        .filter(|r| a.responses.iter().any(|q| q.status == r.status && q != *r))
        .map(|r| r.status.clone())
        .collect();
    let note = note_parts(vec![("追加", added), ("削除", removed), ("変更", changed)]);
    Some(if note.is_empty() {
        "並び順が変わりました".to_string()
    } else {
        note
    })
}

fn changes_between(a: &Operation, b: &Operation) -> (Vec<FieldChange>, Vec<String>) {
    let mut out = Vec::new();
    let mut removed_params = Vec::new();
    if a.operation_id != b.operation_id {
        out.push(FieldChange {
            field: ChangedField::OperationId,
            note: "operationId が変わりました（入力値は引き継ぎます）".into(),
            before: a.operation_id.clone(),
            after: b.operation_id.clone(),
        });
    }
    if a.summary != b.summary {
        out.push(FieldChange {
            field: ChangedField::Summary,
            note: "summary が変わりました".into(),
            before: a.summary.clone(),
            after: b.summary.clone(),
        });
    }
    if a.description != b.description {
        out.push(FieldChange {
            field: ChangedField::Description,
            note: "description が変わりました".into(),
            before: a.description.clone(),
            after: b.description.clone(),
        });
    }
    if let Some((note, removed)) = param_changes(a, b) {
        removed_params = removed;
        out.push(FieldChange {
            field: ChangedField::Parameters,
            note,
            before: None,
            after: None,
        });
    }
    if let Some(note) = request_body_note(a, b) {
        out.push(FieldChange {
            field: ChangedField::RequestBody,
            note,
            before: None,
            after: None,
        });
    }
    if let Some(note) = responses_note(a, b) {
        out.push(FieldChange {
            field: ChangedField::Responses,
            note,
            before: None,
            after: None,
        });
    }
    if a.security != b.security {
        out.push(FieldChange {
            field: ChangedField::Security,
            note: "認証の設定が変わりました".into(),
            before: Some(security_text(&a.security)),
            after: Some(security_text(&b.security)),
        });
    }
    if a.tags != b.tags {
        out.push(FieldChange {
            field: ChangedField::Tags,
            note: "タグが変わりました".into(),
            before: Some(a.tags.join(", ")),
            after: Some(b.tags.join(", ")),
        });
    }
    (out, removed_params)
}

/// 差分を出す。`drafts` は入力値（RequestDraft）を持つ `op_key`（前回の側）。
pub fn diff_operations(old: &[Operation], new: &[Operation], drafts: &HashSet<OpKey>) -> SyncDiff {
    let m = match_operations(old, new);
    let mut diff = SyncDiff {
        added: m.added.iter().map(|&j| entry(&new[j], drafts)).collect(),
        removed: m.removed.iter().map(|&i| entry(&old[i], drafts)).collect(),
        changed: Vec::new(),
    };
    for &(i, j) in &m.pairs {
        let (a, b) = (&old[i], &new[j]);
        if a.content_hash == b.content_hash && a.op_key == b.op_key && a.tags == b.tags {
            continue;
        }
        let (changes, removed_params) = changes_between(a, b);
        if changes.is_empty() {
            continue;
        }
        let mut e = entry(b, drafts);
        e.has_draft = drafts.contains(&a.op_key);
        diff.changed.push(ChangedEntry {
            entry: e,
            renamed_from: (a.op_key != b.op_key).then(|| a.op_key.clone()),
            changes,
            removed_params,
        });
    }
    diff
}
