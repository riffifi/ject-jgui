mod brockmann;
mod document;
mod theme;

use document::*;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct Output {
    values: Map<String, Value>,
    clicked: HashMap<String, bool>,
    error: Option<String>,
}

/// A session owns state independently of windows and graphics devices.
pub(crate) struct Session {
    document: Map<String, Value>,
    widgets: Vec<Value>,
    output: Output,
    host: *const ject_native::HostV1,
    view: Option<u64>,
    on_event: Option<u64>,
}

impl Session {
    fn new(document: Map<String, Value>, host: *const ject_native::HostV1) -> Result<Self, String> {
        validate_document(&document)?;
        let view = callback(&Value::Object(document.clone()), "view");
        let on_event = callback(&Value::Object(document.clone()), "on_event");
        let mut session = Self {
            output: Output {
                values: initial_values(&document),
                ..Output::default()
            },
            widgets: document["widgets"].as_array().unwrap().clone(),
            document,
            host,
            view,
            on_event,
        };
        session.refresh()?;
        Ok(session)
    }

    fn refresh(&mut self) -> Result<(), String> {
        let Some(view) = self.view else {
            return Ok(());
        };
        // View functions are pure descriptions. One persistent event handler keeps
        // callback handles bounded even when the view creates a new tree each time.
        let widgets = unsafe {
            ject_native::invoke_callback(
                self.host,
                view,
                vec![Value::Object(self.output.values.clone())],
            )
        }?;
        let nodes = widgets
            .as_array()
            .ok_or("app view must return an array of widgets")?;
        fn check(nodes: &[Value]) -> Result<(), String> {
            for node in nodes {
                if ["on_click", "on_change"]
                    .iter()
                    .any(|key| callback(node, key).is_some())
                {
                    return Err("app views use the app's on_event handler; widget callbacks are supported by gui.run".into());
                }
                check(children(node))?;
            }
            Ok(())
        }
        let mut description = self.document.clone();
        description.insert("widgets".into(), widgets.clone());
        validate_document(&description)?;
        check(nodes)?;
        // A newly introduced input gets its default once. Existing state survives
        // reordering and temporary removal because IDs identify state, not positions.
        for (key, value) in initial_values(&description) {
            self.output.values.entry(key).or_insert(value);
        }
        self.widgets = nodes.clone();
        Ok(())
    }

    fn event(&mut self, id: &str, kind: &str, value: Value) -> Result<bool, String> {
        let widget = find_widget(&self.widgets, id)
            .ok_or_else(|| format!("document has no widget with ID '{id}'"))?
            .clone();
        if kind == "change" {
            self.output.values.insert(id.into(), value.clone());
        }
        if kind == "click" {
            self.output.clicked.insert(id.into(), true);
        }
        let key = match kind {
            "click" => "on_click",
            "change" => "on_change",
            _ => return Err("event kind must be 'change' or 'click'".into()),
        };
        let action = emit_event(
            self.host,
            callback(&widget, key),
            kind,
            id,
            value.clone(),
            &self.output.values,
        )?;
        let mut close = merge_action(&action, &mut self.output);
        let action = emit_event(
            self.host,
            self.on_event,
            kind,
            id,
            value,
            &self.output.values,
        )?;
        close |= merge_action(&action, &mut self.output);
        close |= kind == "click"
            && widget
                .get("close")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        self.refresh()?;
        Ok(close)
    }
}

fn call(
    function: &str,
    args: Vec<Value>,
    host: *const ject_native::HostV1,
) -> Result<Value, String> {
    if function == "themes" {
        return Ok(json!(theme::NAMES));
    }
    let document = args
        .first()
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{function} expects a document"))?;
    match function {
        "validate" => {
            validate_document(document)?;
            Ok(Value::Bool(true))
        }
        "dispatch" => {
            let [_, Value::String(id), Value::String(kind), value] = args.as_slice() else {
                return Err("dispatch expects a document, widget ID, event kind, and value".into());
            };
            validate_document(document)?;
            let widget = find_widget(document["widgets"].as_array().unwrap(), id)
                .ok_or_else(|| format!("document has no widget with ID '{id}'"))?;
            let key = match kind.as_str() {
                "change" => "on_change",
                "click" => "on_click",
                _ => return Err("event kind must be 'change' or 'click'".into()),
            };
            let mut values = initial_values(document);
            if kind == "change" {
                values.insert(id.clone(), value.clone());
            }
            emit_event(
                host,
                callback(widget, key),
                kind,
                id,
                value.clone(),
                &values,
            )
        }
        "run" => brockmann::run(Session::new(document.clone(), host)?),
        "replay" => {
            let events = args
                .get(1)
                .and_then(Value::as_array)
                .ok_or("replay expects an array of events")?;
            let mut session = Session::new(document.clone(), host)?;
            let mut closed = false;
            for event in events {
                let id = event
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or("event.id must be a string")?;
                let kind = event
                    .get("kind")
                    .and_then(Value::as_str)
                    .ok_or("event.kind must be a string")?;
                closed =
                    session.event(id, kind, event.get("value").cloned().unwrap_or(Value::Null))?;
                if closed {
                    break;
                }
            }
            Ok(
                json!({"values": session.output.values, "widgets": session.widgets, "closed": closed}),
            )
        }
        _ => Err(format!("unknown function '{function}'")),
    }
}

ject_native::ject_plugin_v2!(
    "jgui",
    ["run", "themes", "validate", "dispatch", "replay"],
    call
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_is_complete_before_rendering_and_explicit_state_wins() {
        let document = json!({"widgets": [
            {"kind": "column", "visible": false, "children": [
                {"kind": "text_input", "id": "name", "value": "default"},
                {"kind": "checkbox", "id": "ready"},
                {"kind": "slider", "id": "gain", "value": 3}
            ]},
            {"kind": "button", "id": "save"}
        ], "state": {"name": "override", "status": "idle"}});
        assert_eq!(
            initial_values(document.as_object().unwrap()),
            json!({"name": "override", "ready": false, "gain": 3, "status": "idle"})
                .as_object()
                .unwrap()
                .clone()
        );
    }

    #[test]
    fn malformed_fields_are_rejected_instead_of_silently_defaulted() {
        for fields in [
            json!({"width": "wide"}),
            json!({"height": null}),
            json!({"theme": 3}),
            json!({"view": "not a function"}),
            json!({"on_event": 42}),
        ] {
            let mut document = fields.as_object().unwrap().clone();
            document.insert("widgets".into(), json!([]));
            assert!(validate_document(&document).is_err());
        }
        for widget in [
            json!({"kind": "slider", "id": "x", "min": 10, "max": 2}),
            json!({"kind": "grid", "columns": 0}),
            json!({"kind": "select", "id": "x", "options": [1]}),
            json!({"kind": "checkbox", "id": "x", "value": "yes"}),
            json!({"kind": "label", "visible": "false"}),
            json!({"kind": "scroll", "height": -1}),
        ] {
            let document = json!({"widgets": [widget]});
            assert!(
                validate_document(document.as_object().unwrap()).is_err(),
                "{document}"
            );
        }
    }

    #[test]
    fn callback_actions_patch_state_and_request_close() {
        let mut output = Output::default();
        output.values.insert("tempo".into(), json!(120));
        let close = merge_action(
            &json!({ "set": { "tempo": 132, "status": "rendered" }, "close": true }),
            &mut output,
        );
        assert!(close);
        assert_eq!(output.values["tempo"], json!(132));
        assert_eq!(output.values["status"], json!("rendered"));
    }

    #[test]
    fn documents_are_validated_before_opening_a_window() {
        let valid =
            json!({"widgets":[{"kind":"column","children":[{"kind":"label"}]}],"theme":"linen"});
        assert!(validate_document(valid.as_object().unwrap()).is_ok());

        let unknown = json!({"widgets":[{"kind":"mystery"}]});
        assert_eq!(
            validate_document(unknown.as_object().unwrap()).unwrap_err(),
            "document.widgets[0] has unknown widget kind 'mystery'"
        );
        let bad_size = json!({"widgets":[],"width":0});
        assert_eq!(
            validate_document(bad_size.as_object().unwrap()).unwrap_err(),
            "document.width must be a positive number"
        );
        let duplicate =
            json!({"widgets":[{"kind":"button","id":"save"},{"kind":"button","id":"save"}]});
        assert_eq!(
            validate_document(duplicate.as_object().unwrap()).unwrap_err(),
            "document.widgets[1].id duplicates 'save'"
        );

        let mut nested = json!({"kind":"label"});
        for _ in 0..66 {
            nested = json!({"kind":"column","children":[nested]});
        }
        let deep = json!({"widgets":[nested]});
        assert!(validate_document(deep.as_object().unwrap())
            .unwrap_err()
            .contains("maximum layout depth"));
    }
}
