use crate::{theme, Output};
use serde_json::{json, Map, Value};
use std::collections::HashSet;

pub(crate) fn text(widget: &Value, key: &str) -> String {
    widget
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn callback(widget: &Value, key: &str) -> Option<u64> {
    widget.get(key).and_then(ject_native::callback_id)
}

pub(crate) fn emit_event(
    host: *const ject_native::HostV1,
    callback: Option<u64>,
    kind: &str,
    id: &str,
    value: Value,
    values: &Map<String, Value>,
) -> Result<Value, String> {
    let Some(callback) = callback else {
        return Ok(Value::Null);
    };
    let event = json!({
        "kind": kind,
        "id": id,
        "value": value,
        "values": values,
    });
    unsafe { ject_native::invoke_callback(host, callback, vec![event]) }
}

pub(crate) fn merge_action(action: &Value, output: &mut Output) -> bool {
    let Some(action) = action.as_object() else {
        return false;
    };
    if let Some(values) = action
        .get("set")
        .or_else(|| action.get("values"))
        .and_then(Value::as_object)
    {
        output.values.extend(values.clone());
    }
    action
        .get("close")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub(crate) fn children(widget: &Value) -> &[Value] {
    widget
        .get("children")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

pub(crate) fn find_widget<'a>(widgets: &'a [Value], id: &str) -> Option<&'a Value> {
    for widget in widgets {
        if widget.get("id").and_then(Value::as_str) == Some(id) {
            return Some(widget);
        }
        if let Some(found) = find_widget(children(widget), id) {
            return Some(found);
        }
    }
    None
}

// Seed every bound widget before the first event, including hidden children.
// Explicit document state takes precedence over constructor defaults.
pub(crate) fn initial_values(document: &Map<String, Value>) -> Map<String, Value> {
    fn collect(widgets: &[Value], values: &mut Map<String, Value>) {
        for widget in widgets {
            let kind = widget.get("kind").and_then(Value::as_str).unwrap_or("");
            let default = match kind {
                "text_input" | "password" | "multiline" | "select" | "radio" => Some(json!("")),
                "checkbox" | "toggle" => Some(json!(false)),
                "slider" | "number_input" | "meter" => Some(json!(0)),
                "value_text" => Some(Value::Null),
                _ => None,
            };
            if let (Some(id), Some(default)) = (widget.get("id").and_then(Value::as_str), default) {
                values.insert(
                    id.to_owned(),
                    widget.get("value").cloned().unwrap_or(default),
                );
            }
            collect(children(widget), values);
        }
    }
    let mut values = Map::new();
    if let Some(widgets) = document.get("widgets").and_then(Value::as_array) {
        collect(widgets, &mut values);
    }
    if let Some(state) = document.get("state").and_then(Value::as_object) {
        values.extend(state.clone());
    }
    values
}

pub(crate) fn validate_document(document: &Map<String, Value>) -> Result<(), String> {
    const MAX_WIDGETS: usize = 10_000;
    const MAX_LAYOUT_DEPTH: usize = 64;

    fn validate_plot(widget: &Value, location: &str) -> Result<(), String> {
        let chart = widget
            .get("chart")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{location}.chart must be a string"))?;
        if !["line", "scatter", "bar"].contains(&chart) {
            return Err(format!("{location}.chart must be line, scatter, or bar"));
        }
        let series = widget
            .get("series")
            .and_then(Value::as_array)
            .filter(|series| !series.is_empty() && series.len() <= 8)
            .ok_or_else(|| format!("{location}.series must contain 1 to 8 series"))?;
        let mut sample_count = None;
        for (series_index, item) in series.iter().enumerate() {
            let series_location = format!("{location}.series[{series_index}]");
            if item.get("name").is_some_and(|value| !value.is_string()) {
                return Err(format!("{series_location}.name must be a string"));
            }
            if let Some(color) = item.get("color") {
                let valid = color.as_str().is_some_and(|color| {
                    color.len() == 7
                        && color.starts_with('#')
                        && color[1..].chars().all(|c| c.is_ascii_hexdigit())
                });
                if !valid {
                    return Err(format!(
                        "{series_location}.color must be a six-digit hex color"
                    ));
                }
            }
            let count = if chart == "scatter" {
                let points = item
                    .get("points")
                    .and_then(Value::as_array)
                    .filter(|points| !points.is_empty() && points.len() <= 1000)
                    .ok_or_else(|| {
                        format!("{series_location}.points must contain 1 to 1000 [x, y] pairs")
                    })?;
                for (point_index, point) in points.iter().enumerate() {
                    let pair =
                        point
                            .as_array()
                            .filter(|pair| pair.len() == 2)
                            .ok_or_else(|| {
                                format!(
                                "{series_location}.points[{point_index}] must be an [x, y] pair"
                            )
                            })?;
                    if pair.iter().any(|value| {
                        value
                            .as_f64()
                            .is_none_or(|number| !number.is_finite() || number.abs() > 1e15)
                    }) {
                        return Err(format!(
                            "{series_location}.points[{point_index}] must contain finite values within +/- 1e15"
                        ));
                    }
                }
                points.len()
            } else {
                let values = item
                    .get("values")
                    .and_then(Value::as_array)
                    .filter(|values| !values.is_empty() && values.len() <= 1000)
                    .ok_or_else(|| {
                        format!("{series_location}.values must contain 1 to 1000 numbers")
                    })?;
                if values.iter().any(|value| {
                    value
                        .as_f64()
                        .is_none_or(|number| !number.is_finite() || number.abs() > 1e15)
                }) {
                    return Err(format!(
                        "{series_location}.values must contain finite numbers within +/- 1e15"
                    ));
                }
                values.len()
            };
            if let Some(expected) = sample_count {
                if count != expected {
                    return Err(format!(
                        "{location}.series must all have the same number of samples"
                    ));
                }
            } else {
                sample_count = Some(count);
            }
        }
        if let Some(labels) = widget.get("labels") {
            let labels = labels
                .as_array()
                .filter(|labels| labels.iter().all(Value::is_string))
                .ok_or_else(|| format!("{location}.labels must be an array of strings"))?;
            if !labels.is_empty() && Some(labels.len()) != sample_count {
                return Err(format!(
                    "{location}.labels must match the number of plotted samples"
                ));
            }
        }
        Ok(())
    }

    fn validate_widgets(
        widgets: &[Value],
        path: &str,
        ids: &mut HashSet<String>,
        widget_count: &mut usize,
        depth: usize,
    ) -> Result<(), String> {
        const KINDS: [&str; 29] = [
            "row",
            "wrap",
            "column",
            "group",
            "card",
            "collapsible",
            "scroll",
            "grid",
            "heading",
            "label",
            "code",
            "link",
            "badge",
            "separator",
            "spacer",
            "progress",
            "meter",
            "value_text",
            "text_input",
            "password",
            "multiline",
            "checkbox",
            "toggle",
            "slider",
            "number_input",
            "select",
            "radio",
            "button",
            "plot",
        ];
        if depth > MAX_LAYOUT_DEPTH {
            return Err(format!(
                "{path} exceeds the maximum layout depth of {MAX_LAYOUT_DEPTH}"
            ));
        }
        for (index, widget) in widgets.iter().enumerate() {
            *widget_count = widget_count
                .checked_add(1)
                .ok_or("document contains too many widgets")?;
            if *widget_count > MAX_WIDGETS {
                return Err(format!("document contains more than {MAX_WIDGETS} widgets"));
            }
            let location = format!("{path}[{index}]");
            let kind = widget
                .get("kind")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{location}.kind must be a string"))?;
            if !KINDS.contains(&kind) {
                return Err(format!("{location} has unknown widget kind '{kind}'"));
            }
            let requires_id = matches!(
                kind,
                "meter"
                    | "value_text"
                    | "text_input"
                    | "password"
                    | "multiline"
                    | "checkbox"
                    | "toggle"
                    | "slider"
                    | "number_input"
                    | "select"
                    | "radio"
                    | "button"
                    | "plot"
            );
            if requires_id && widget.get("id").is_none() {
                return Err(format!("{location}.id must be a non-empty string"));
            }
            if let Some(id) = widget.get("id") {
                let id = id
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| format!("{location}.id must be a non-empty string"))?;
                if id.starts_with("@jgui:") {
                    return Err(format!("{location}.id uses the reserved @jgui: prefix"));
                }
                if !ids.insert(id.to_string()) {
                    return Err(format!("{location}.id duplicates '{id}'"));
                }
            }
            for callback_key in ["on_change", "on_click"] {
                if let Some(value) = widget.get(callback_key) {
                    if !value.is_null() && ject_native::callback_id(value).is_none() {
                        return Err(format!(
                            "{location}.{callback_key} must be a callback or nil"
                        ));
                    }
                }
            }
            for key in ["text", "label", "url", "tooltip"] {
                if widget.get(key).is_some_and(|value| !value.is_string()) {
                    return Err(format!("{location}.{key} must be a string"));
                }
            }
            for key in ["enabled", "visible", "open", "close"] {
                if widget.get(key).is_some_and(|value| !value.is_boolean()) {
                    return Err(format!("{location}.{key} must be a boolean"));
                }
            }
            let value_type = match kind {
                "text_input" | "password" | "multiline" | "select" | "radio" => Some("string"),
                "checkbox" | "toggle" => Some("boolean"),
                "slider" | "number_input" | "progress" | "meter" | "spacer" => Some("number"),
                _ => None,
            };
            if let (Some(value), Some(expected)) = (widget.get("value"), value_type) {
                let valid = match expected {
                    "string" => value.is_string(),
                    "boolean" => value.is_boolean(),
                    _ => value.as_f64().is_some_and(f64::is_finite),
                };
                if !valid {
                    return Err(format!("{location}.value must be a {expected}"));
                }
            }
            for key in ["min", "max", "speed", "height", "spacing"] {
                if let Some(value) = widget.get(key) {
                    let number = value
                        .as_f64()
                        .filter(|n| n.is_finite())
                        .ok_or_else(|| format!("{location}.{key} must be a finite number"))?;
                    if matches!(key, "speed" | "height") && number <= 0.0
                        || key == "spacing" && number < 0.0
                    {
                        return Err(format!("{location}.{key} is out of range"));
                    }
                }
            }
            if kind == "slider" {
                let min = widget.get("min").and_then(Value::as_f64).unwrap_or(0.0);
                let max = widget.get("max").and_then(Value::as_f64).unwrap_or(100.0);
                if min > max {
                    return Err(format!("{location}.min must not exceed max"));
                }
            }
            if kind == "plot" {
                let height = widget.get("height").and_then(Value::as_f64).unwrap_or(320.);
                if !(200. ..=1200.).contains(&height) {
                    return Err(format!("{location}.height must be from 200 to 1200"));
                }
                validate_plot(widget, &location)?;
            }
            if kind == "grid"
                && !widget
                    .get("columns")
                    .and_then(Value::as_u64)
                    .is_some_and(|n| n > 0)
            {
                return Err(format!("{location}.columns must be a positive integer"));
            }
            if matches!(kind, "select" | "radio")
                && !widget
                    .get("options")
                    .and_then(Value::as_array)
                    .is_some_and(|options| options.iter().all(Value::is_string))
            {
                return Err(format!("{location}.options must be an array of strings"));
            }
            if let Some(children) = widget.get("children") {
                let children = children
                    .as_array()
                    .ok_or_else(|| format!("{location}.children must be an array"))?;
                validate_widgets(
                    children,
                    &format!("{location}.children"),
                    ids,
                    widget_count,
                    depth + 1,
                )?;
            }
        }
        Ok(())
    }

    let widgets = document
        .get("widgets")
        .and_then(Value::as_array)
        .ok_or("document.widgets must be an array")?;
    if document
        .get("title")
        .is_some_and(|title| !title.is_string())
    {
        return Err("document.title must be a string".into());
    }
    for key in ["width", "height"] {
        let value = match document.get(key) {
            Some(value) => value
                .as_f64()
                .ok_or_else(|| format!("document.{key} must be a positive number"))?,
            None => 1.0,
        };
        if !value.is_finite() || value <= 0.0 {
            return Err(format!("document.{key} must be a positive number"));
        }
    }
    if document
        .get("state")
        .is_some_and(|state| !state.is_object())
    {
        return Err("document.state must be a dictionary".into());
    }
    if document
        .get("theme")
        .is_some_and(|theme| !theme.is_string())
    {
        return Err("document.theme must be a string".into());
    }
    theme::validate(
        document
            .get("theme")
            .and_then(Value::as_str)
            .unwrap_or("linen"),
    )?;
    for key in ["view", "on_event"] {
        if document
            .get(key)
            .is_some_and(|v| !v.is_null() && ject_native::callback_id(v).is_none())
        {
            return Err(format!("document.{key} must be a callback or nil"));
        }
    }
    let mut widget_count = 0;
    validate_widgets(
        widgets,
        "document.widgets",
        &mut HashSet::new(),
        &mut widget_count,
        0,
    )
}
