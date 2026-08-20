use serde_json::Value;

#[derive(Debug, Clone)]
pub struct DataTableColumn {
    pub id: String,
    pub title: String,
    pub width: Option<f32>,
}

pub struct DataTableState {
    pub columns: Vec<DataTableColumn>,
    pub rows: Vec<Value>,
    pub sortable: bool,
    pub selectable: bool,
    pub max_rows: usize,
    pub selected_id: Option<String>,
    pub sort: Option<(String, bool)>,
    pub next_row_id: u64,
}

impl DataTableState {
    pub fn from_config(config: &serde_json::Map<String, Value>) -> Result<Self, String> {
        let columns = config
            .get("columns")
            .and_then(Value::as_array)
            .ok_or_else(|| "table panel requires columns".to_owned())?
            .iter()
            .map(|v| {
                let id = v
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "table column requires id".to_owned())?;
                Ok(DataTableColumn {
                    id: id.to_owned(),
                    title: v
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .to_owned(),
                    width: v.get("width").and_then(Value::as_f64).map(|x| x as f32),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        if columns.is_empty() {
            return Err("table panel requires at least one column".to_owned());
        }
        let mut s = Self {
            columns,
            rows: Vec::new(),
            sortable: config
                .get("sortable")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            selectable: config
                .get("selectable")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            max_rows: config
                .get("max_rows")
                .and_then(Value::as_u64)
                .unwrap_or(1_000) as usize,
            selected_id: None,
            sort: None,
            next_row_id: 1,
        };
        s.set_rows(
            config
                .get("rows")
                .cloned()
                .unwrap_or(Value::Array(vec![])),
        );
        Ok(s)
    }

    pub fn set_rows(&mut self, rows: Value) {
        self.rows = rows.as_array().cloned().unwrap_or_default();
        self.ensure_row_ids();
        self.trim();
        self.apply_sort();
        if self
            .selected_id
            .as_deref()
            .is_some_and(|id| !self.rows.iter().any(|r| row_id(r) == id))
        {
            self.selected_id = None;
        }
    }
    pub fn append_rows(&mut self, rows: Value) {
        if let Some(arr) = rows.as_array() {
            self.rows.extend(arr.iter().cloned());
            self.ensure_row_ids();
            self.trim();
            self.apply_sort();
        }
    }
    pub fn remove_rows(&mut self, ids: Value) {
        let set: std::collections::HashSet<String> = ids
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        self.rows.retain(|r| !set.contains(row_id(r)));
        if self.selected_id.as_ref().is_some_and(|id| set.contains(id)) {
            self.selected_id = None;
        }
    }
    pub fn clear(&mut self) {
        self.rows.clear();
        self.selected_id = None;
    }

    fn trim(&mut self) {
        if self.rows.len() > self.max_rows {
            self.rows.drain(..self.rows.len() - self.max_rows);
        }
    }
    fn ensure_row_ids(&mut self) {
        for row in &mut self.rows {
            let Some(obj) = row.as_object_mut() else {
                continue;
            };
            if obj.get("id").and_then(Value::as_str).is_none() {
                obj.insert(
                    "id".to_owned(),
                    Value::String(format!("__row_{}", self.next_row_id)),
                );
                self.next_row_id += 1;
            }
        }
    }
    fn apply_sort(&mut self) {
        let Some((col, asc)) = self.sort.clone() else {
            return;
        };
        self.rows.sort_by(|a, b| {
            let ord = compare_values(a.get(&col), b.get(&col));
            if asc { ord } else { ord.reverse() }
        });
    }

    pub fn sort_by(&mut self, column_id: &str) {
        if !self.sortable {
            return;
        }
        let asc = self
            .sort
            .as_ref()
            .map(|(id, asc)| id != column_id || !asc)
            .unwrap_or(true);
        self.sort = Some((column_id.to_owned(), asc));
        self.apply_sort();
    }

    pub fn select(&mut self, id: &str) {
        if !self.selectable {
            return;
        }
        if self.rows.iter().any(|r| row_id(r) == id) {
            self.selected_id = Some(id.to_owned());
        }
    }
}

fn row_id(row: &Value) -> &str {
    row.get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
}
fn compare_values(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(va), Some(vb)) => match (va, vb) {
            (Value::Number(na), Value::Number(nb)) => {
                let fa = na.as_f64().unwrap_or(0.0);
                let fb = nb.as_f64().unwrap_or(0.0);
                fa.partial_cmp(&fb).unwrap_or(Ordering::Equal)
            }
            (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
            _ => display_value(Some(va)).cmp(&display_value(Some(vb))),
        },
    }
}
pub fn display_value(v: Option<&Value>) -> String {
    match v {
        None => "".to_owned(),
        Some(Value::Null) => "".to_owned(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(other) => other.to_string(),
    }
}
