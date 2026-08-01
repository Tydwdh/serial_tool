use egui::{Button, RichText};
use serde_json::Value;
use tool_core::{Direction, Event, Payload, topics};
use tool_databus::DataBus;

#[derive(Debug, Clone)]
pub struct DataTableColumn {
    pub id: String,
    pub title: String,
    pub width: Option<f32>,
}

pub struct DataTablePanel {
    columns: Vec<DataTableColumn>,
    rows: Vec<Value>,
    sortable: bool,
    selectable: bool,
    max_rows: usize,
    selected_id: Option<String>,
    sort: Option<(String, bool)>,
    next_row_id: u64,
}

impl DataTablePanel {
    pub fn from_config(config: &serde_json::Map<String, Value>) -> Result<Self, String> {
        let columns = config
            .get("columns")
            .and_then(Value::as_array)
            .ok_or_else(|| "table panel requires columns".to_owned())?
            .iter()
            .map(|value| {
                let id = value
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "table column requires id".to_owned())?;
                Ok(DataTableColumn {
                    id: id.to_owned(),
                    title: value
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .to_owned(),
                    width: value.get("width").and_then(Value::as_f64).map(|v| v as f32),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        if columns.is_empty() {
            return Err("table panel requires at least one column".to_owned());
        }
        let mut panel = Self {
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
        panel.set_rows(
            config
                .get("rows")
                .cloned()
                .unwrap_or_else(|| Value::Array(vec![])),
        );
        Ok(panel)
    }

    pub fn set_rows(&mut self, rows: Value) {
        self.rows = rows.as_array().cloned().unwrap_or_default();
        self.ensure_row_ids();
        self.trim();
        self.apply_sort();
        if self
            .selected_id
            .as_deref()
            .is_some_and(|id| !self.rows.iter().any(|row| row_id(row) == id))
        {
            self.selected_id = None;
        }
    }

    pub fn append_rows(&mut self, rows: Value) {
        if let Some(rows) = rows.as_array() {
            self.rows.extend(rows.iter().cloned());
            self.ensure_row_ids();
            self.trim();
            self.apply_sort();
        }
    }

    pub fn remove_rows(&mut self, ids: Value) {
        let ids: std::collections::HashSet<String> = ids
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect();
        self.rows.retain(|row| !ids.contains(row_id(row)));
        if self.selected_id.as_ref().is_some_and(|id| ids.contains(id)) {
            self.selected_id = None;
        }
    }

    pub fn clear(&mut self) {
        self.rows.clear();
        self.selected_id = None;
    }

    pub fn rows(&self) -> &[Value] {
        &self.rows
    }

    fn trim(&mut self) {
        if self.rows.len() > self.max_rows {
            self.rows.drain(..self.rows.len() - self.max_rows);
        }
    }

    fn ensure_row_ids(&mut self) {
        for row in &mut self.rows {
            let Some(object) = row.as_object_mut() else {
                continue;
            };
            if object.get("id").and_then(Value::as_str).is_none() {
                object.insert(
                    "id".to_owned(),
                    Value::String(format!("__row_{}", self.next_row_id)),
                );
                self.next_row_id += 1;
            }
        }
    }

    fn apply_sort(&mut self) {
        let Some((column, ascending)) = self.sort.clone() else {
            return;
        };
        self.rows.sort_by(|a, b| {
            let order = compare_values(a.get(&column), b.get(&column));
            if ascending { order } else { order.reverse() }
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, panel_id: &str, bus: &DataBus) {
        let mut selected = None;
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new(("plugin-table", panel_id))
                    .striped(true)
                    .min_col_width(72.0)
                    .show(ui, |ui| {
                        for column in self.columns.clone() {
                            let arrow = self.sort.as_ref().and_then(|(id, asc)| {
                                (id == &column.id).then_some(if *asc { " ↑" } else { " ↓" })
                            });
                            let response = ui.add_sized(
                                [column.width.unwrap_or(100.0), 24.0],
                                Button::new(
                                    RichText::new(format!(
                                        "{}{}",
                                        column.title,
                                        arrow.unwrap_or("")
                                    ))
                                    .strong(),
                                )
                                .frame(false),
                            );
                            if self.sortable && response.clicked() {
                                let ascending = self
                                    .sort
                                    .as_ref()
                                    .map(|(id, asc)| id != &column.id || !asc)
                                    .unwrap_or(true);
                                self.sort = Some((column.id, ascending));
                                self.apply_sort();
                            }
                        }
                        ui.end_row();

                        for row in &self.rows {
                            let id = row_id(row).to_owned();
                            let is_selected = self.selected_id.as_deref() == Some(&id);
                            let mut clicked = false;
                            for column in &self.columns {
                                let text = display_value(row.get(&column.id));
                                let response = ui.add_sized(
                                    [column.width.unwrap_or(100.0), 22.0],
                                    Button::new(text).selected(is_selected).frame(is_selected),
                                );
                                clicked |= response.clicked();
                            }
                            ui.end_row();
                            if self.selectable && clicked {
                                selected = Some((id, row.clone()));
                            }
                        }
                    });
            });

        if let Some((id, row)) = selected {
            self.selected_id = Some(id.clone());
            bus.publish(Event::new(
                topics::UI_TABLE_SELECTION_CHANGED,
                "ui",
                Direction::Internal,
                Payload::Json(serde_json::json!({
                    "panel_id": panel_id,
                    "row_id": id,
                    "row": row,
                })),
            ));
        }
    }
}

fn row_id(row: &Value) -> &str {
    row.get("id").and_then(Value::as_str).unwrap_or("")
}

fn display_value(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
    }
}

fn compare_values(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    match (a.and_then(Value::as_f64), b.and_then(Value::as_f64)) {
        (Some(a), Some(b)) => a.total_cmp(&b),
        _ => display_value(a).cmp(&display_value(b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_supports_replace_append_remove_and_limit() {
        let mut table = DataTablePanel::from_config(
            serde_json::json!({
                "columns": [{"id":"value","title":"Value"}],
                "max_rows": 2
            })
            .as_object()
            .unwrap(),
        )
        .unwrap();
        table.set_rows(serde_json::json!([{"id":"1","value":1}]));
        table.append_rows(serde_json::json!([
            {"id":"2","value":2}, {"id":"3","value":3}
        ]));
        assert_eq!(table.rows().len(), 2);
        table.remove_rows(serde_json::json!(["2"]));
        assert_eq!(table.rows()[0]["id"], "3");
        table.clear();
        assert!(table.rows().is_empty());
    }
}
