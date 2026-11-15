//! `probe-detail` paging of a validated `probe-result` (ROK-2191, CONTRACT
//! "Endpoints"): a result is split into **items** — each call without its
//! `iterations` (its head), then each of its iterations — and a page carries
//! as many whole items as fit. A `forEach` call whose iterations span pages
//! repeats its head on the next page with the next slice of `iterations`
//! (clients merge calls by `id`, iterations by `i`).

use super::paging::json_len;
use super::probe_result::call_head;
use serde_json::{Map, Value};

/// One paging item of a probe result.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Index of the call in `result.calls`.
    pub call: usize,
    /// The call's `id`.
    pub id: String,
    /// `None` = the call head; `Some(i)` = the iteration whose `i` this is.
    pub i: Option<u64>,
    /// The head (call without `iterations`) or the iteration object.
    pub value: Value,
    /// Head only: the call carries an `iterations` array (even an empty one).
    pub has_iterations: bool,
}

/// Bytes `"iterations":[]` adds to a call object (with its comma).
const EMPTY_ITERATIONS_BYTES: usize = 16;

/// Split a validated result into its paging items, in order.
pub fn page_items(result: &Value) -> Vec<Item> {
    let calls = result.get("calls").and_then(Value::as_array);
    let mut out = Vec::new();
    for (c, call) in calls.into_iter().flatten().enumerate() {
        let id = call.get("id").and_then(Value::as_str).unwrap_or_default();
        let its = call.get("iterations").and_then(Value::as_array);
        out.push(Item {
            call: c,
            id: id.to_string(),
            i: None,
            value: call_head(call),
            has_iterations: its.is_some(),
        });
        for it in its.into_iter().flatten() {
            out.push(Item {
                call: c,
                id: id.to_string(),
                i: it.get("i").and_then(Value::as_u64),
                value: it.clone(),
                has_iterations: false,
            });
        }
    }
    out
}

/// A copy of `result` with `calls: []` — the summary every page repeats.
fn summary(result: &Value) -> Value {
    let mut base = result.clone();
    if let Some(m) = base.as_object_mut() {
        m.insert("calls".into(), Value::Array(Vec::new()));
    }
    base
}

/// Builds one page's `calls` while tracking the slice's serialised size.
struct Slice {
    calls: Vec<Map<String, Value>>,
    bytes: usize,
}

impl Slice {
    /// Bytes `item` adds to the slice (exact for `serde_json` output).
    fn cost(&self, item: &Item) -> usize {
        let len = json_len(&item.value);
        if item.i.is_none() {
            let extra = if item.has_iterations {
                EMPTY_ITERATIONS_BYTES
            } else {
                0
            };
            return len + extra + usize::from(!self.calls.is_empty());
        }
        let n = self.last_iterations().map_or(0, Vec::len);
        len + usize::from(n > 0)
    }

    fn last_iterations(&self) -> Option<&Vec<Value>> {
        self.calls.last()?.get("iterations")?.as_array()
    }

    fn push(&mut self, item: &Item) {
        self.bytes += self.cost(item);
        if item.i.is_none() {
            let mut head = item.value.as_object().cloned().unwrap_or_default();
            if item.has_iterations {
                head.insert("iterations".into(), Value::Array(Vec::new()));
            }
            self.calls.push(head);
        } else if let Some(Value::Array(its)) =
            self.calls.last_mut().and_then(|c| c.get_mut("iterations"))
        {
            its.push(item.value.clone());
        }
    }
}

/// The head item of the call `items[at]` belongs to.
fn head_of(items: &[Item], at: usize) -> Option<&Item> {
    let call = items.get(at)?.call;
    items[..at]
        .iter()
        .rev()
        .find(|it| it.call == call && it.i.is_none())
}

/// One page of `result` from item `start`: the result with `calls` holding as
/// many whole items as keep the serialised slice ≤ `budget` bytes (always at
/// least one item; a page that opens mid-`forEach` repeats that call's head).
/// Returns the slice and the next item index (`None` = last page).
pub fn assemble_page(
    result: &Value,
    items: &[Item],
    start: usize,
    budget: usize,
) -> (Value, Option<usize>) {
    let mut base = summary(result);
    let mut slice = Slice {
        calls: Vec::new(),
        bytes: json_len(&base),
    };
    if items.get(start).is_some_and(|it| it.i.is_some()) {
        if let Some(head) = head_of(items, start) {
            slice.push(head);
        }
    }
    let mut next = None;
    for (idx, item) in items.iter().enumerate().skip(start) {
        let first = idx == start;
        if !first && slice.bytes + slice.cost(item) > budget {
            next = Some(idx);
            break;
        }
        slice.push(item);
    }
    if let Some(m) = base.as_object_mut() {
        let calls = slice.calls.into_iter().map(Value::Object).collect();
        m.insert("calls".into(), Value::Array(calls));
    }
    (base, next)
}
