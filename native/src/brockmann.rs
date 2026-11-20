//! Translate JGUI's public descriptions to Brockmann. No graphics concepts cross
//! the native boundary; Ject callbacks run synchronously on the interpreter thread.
use crate::{
    document::{children, find_widget, text},
    theme, Session,
};
use ::brockmann::{
    layout::Layout,
    theme::{Color, TypeRole},
    ui::Value as UiValue,
    widget::{ButtonStyle, Kind, PlotData, PlotKind, PlotSeries},
    Application, Event, Id, Rect, Ui, Widget,
};
use serde_json::{json, Map, Value};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

struct View {
    ids: HashMap<Id, String>,
    generated: HashSet<Id>,
    collapsed: HashMap<String, bool>,
    drafts: HashMap<String, String>,
}
impl View {
    fn new() -> Self {
        Self {
            ids: HashMap::new(),
            generated: HashSet::new(),
            collapsed: HashMap::new(),
            drafts: HashMap::new(),
        }
    }
    fn id(&mut self, node: &Value, path: &str) -> Result<Id, String> {
        let name = node
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("@jgui:{path}"));
        let id = Id::from(name.as_str());
        if id.0 == 0 || !self.generated.insert(id) {
            return Err(format!("widget key collision at '{name}'"));
        }
        self.ids.insert(id, name);
        Ok(id)
    }
    fn build(
        &mut self,
        widgets: &[Value],
        values: &Map<String, Value>,
        viewport: Rect,
    ) -> Result<Widget, String> {
        self.ids.clear();
        self.generated.clear();
        let margin = if viewport.w < 720. { 24. } else { 48. };
        let width = (viewport.w - 2. * margin).max(1.);
        let mut children = Vec::new();
        for (i, node) in widgets.iter().enumerate() {
            if let Some(widget) = self.node(node, values, &format!("root/{i}"), width)? {
                children.push(widget);
            }
        }
        self.drafts
            .retain(|name, _| self.ids.values().any(|id| id == name));
        let content = Widget::column(Id::from("@jgui:content"), children).gap(12.);
        // Brockmann has one scroll surface per Ui. The app root is that surface;
        // legacy scroll groups remain ordinary composition inside it.
        Ok(content
            .at(Rect::new(
                margin,
                24.,
                (viewport.w - 2. * margin).max(1.),
                (viewport.h - 48.).max(1.),
            ))
            .scrollable())
    }
    fn node(
        &mut self,
        node: &Value,
        values: &Map<String, Value>,
        path: &str,
        width: f32,
    ) -> Result<Option<Widget>, String> {
        if node.get("visible").and_then(Value::as_bool) == Some(false) {
            return Ok(None);
        }
        let id = self.id(node, path)?;
        let key = self.ids[&id].clone();
        let kind = text(node, "kind");
        let title = {
            let t = text(node, "text");
            if t.is_empty() {
                text(node, "label")
            } else {
                t
            }
        };
        let value = values
            .get(&key)
            .or_else(|| node.get("value"))
            .unwrap_or(&Value::Null);
        let widget = match kind.as_str() {
            "row" | "wrap" | "column" | "group" | "card" | "grid" | "scroll" | "collapsible" => {
                let mut nested = Vec::new();
                let open = *self
                    .collapsed
                    .entry(key.clone())
                    .or_insert(node.get("open").and_then(Value::as_bool).unwrap_or(true));
                let gap = node.get("spacing").and_then(Value::as_f64).unwrap_or(12.) as f32;
                let columns = if kind == "grid" {
                    node["columns"].as_u64().unwrap_or(1) as usize
                } else if kind == "wrap" {
                    ((width / 180.).floor() as usize).max(1)
                } else if kind == "row" {
                    children(node).len().max(1)
                } else {
                    1
                };
                let cell_width =
                    ((width - columns.saturating_sub(1) as f32 * gap) / columns as f32).max(1.);
                if kind == "group" {
                    nested.push(Widget::label(
                        Id::from(format!("@jgui:{path}/title").as_str()),
                        &title,
                        TypeRole::Heading,
                    ));
                }
                if kind == "collapsible" {
                    nested.push(Widget::button(
                        id,
                        format!("{} {title}", if open { "−" } else { "+" }),
                        ButtonStyle::Text,
                    ));
                }
                if kind != "collapsible" || open {
                    for (index, child) in children(node).iter().enumerate() {
                        if let Some(widget) =
                            self.node(child, values, &format!("{path}/{index}"), cell_width)?
                        {
                            nested.push(widget);
                        }
                    }
                }
                let mut group = if kind == "row" {
                    Widget::row(id, nested).gap(12.)
                } else {
                    Widget::column(id, nested).gap(12.)
                };
                if kind == "collapsible" {
                    // The disclosure button owns the public key, the container a private one.
                    group.id = Id::from(format!("@jgui:{path}/container").as_str());
                }
                if kind == "grid" || kind == "wrap" {
                    let nodes = std::mem::take(&mut group.children);
                    let mut rows = Vec::new();
                    let mut iter = nodes.into_iter();
                    loop {
                        let cells: Vec<_> = iter
                            .by_ref()
                            .take(columns)
                            .map(|child| child.width(cell_width))
                            .collect();
                        if cells.is_empty() {
                            break;
                        }
                        let row = Widget::row(
                            Id::from(format!("@jgui:{path}/grid-row/{}", rows.len()).as_str()),
                            cells,
                        )
                        .gap(gap);
                        rows.push(size_group(row));
                    }
                    group.children = rows;
                    group = group.gap(gap);
                }
                size_group(group)
            }
            "heading" => Widget::label(id, title, TypeRole::Title),
            "label" | "badge" => Widget::label(id, title, TypeRole::Body),
            "plot" => plot_widget(id, title, node)?,
            "code" => Widget::label(id, title, TypeRole::MonoValue),
            "value_text" => Widget::label(id, format!("{title}{}", display(value)), TypeRole::Body),
            "separator" => Widget::label(id, "", TypeRole::Caption).height(4.),
            "spacer" => Widget::label(id, "", TypeRole::Caption)
                .height(value.as_f64().unwrap_or(8.).max(0.) as f32),
            "button" | "link" => Widget::button(
                id,
                title,
                if kind == "link" {
                    ButtonStyle::Text
                } else {
                    ButtonStyle::Secondary
                },
            ),
            "checkbox" | "toggle" => Widget::toggle(id, title, value.as_bool().unwrap_or(false)),
            "text_input" | "multiline" => {
                let mut input = Widget::input(id, title, value.as_str().unwrap_or(""));
                if kind == "multiline" {
                    if let Kind::Input { multiline, .. } = &mut input.kind {
                        *multiline = true;
                    }
                    input = input.height(160.);
                }
                input
            }
            "password" => {
                return Err("password fields are not yet supported by the new JGUI backend".into())
            }
            "number_input" => {
                let draft = self
                    .drafts
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| display(value));
                let mut input = Widget::input(id, title, &draft);
                if draft
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .is_none()
                {
                    if let Kind::Input { error, .. } = &mut input.kind {
                        *error = Some("Enter a finite number".into());
                    }
                }
                input
            }
            "slider" => Widget::slider(
                id,
                title,
                value.as_f64().unwrap_or(0.),
                node["min"].as_f64().unwrap_or(0.),
                node["max"].as_f64().unwrap_or(100.),
                node["value"].as_f64().unwrap_or(0.),
            ),
            "select" | "radio" => {
                let options: Vec<String> = node["options"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                let index = options
                    .iter()
                    .position(|item| Some(item.as_str()) == value.as_str())
                    .unwrap_or(0);
                if kind == "radio" {
                    Widget::choice(id, options, index, false)
                } else {
                    Widget::picker(id, title, options, index)
                }
            }
            "progress" | "meter" => Widget::new(
                id,
                title,
                Kind::Progress {
                    value: Some(value.as_f64().unwrap_or(0.).clamp(0., 1.) as f32),
                },
            )
            .height(32.),
            _ => return Err(format!("unsupported widget '{kind}'")),
        };
        let mut widget =
            widget.disabled(node.get("enabled").and_then(Value::as_bool) == Some(false));
        if let Some(tip) = node.get("tooltip").and_then(Value::as_str) {
            widget = widget.tooltip(tip);
        }
        Ok(Some(widget))
    }
}
fn plot_widget(id: Id, title: String, node: &Value) -> Result<Widget, String> {
    let plot_kind = match text(node, "chart").as_str() {
        "line" => PlotKind::Line,
        "scatter" => PlotKind::Scatter,
        "bar" => PlotKind::Bar,
        other => return Err(format!("unsupported native chart type '{other}'")),
    };
    let series = node["series"]
        .as_array()
        .ok_or("plot series must be an array")?
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Series {}", index + 1));
            let color = item
                .get("color")
                .and_then(Value::as_str)
                .map(parse_color)
                .transpose()?
                .unwrap_or_else(|| Color::hex(PLOT_PALETTE[index % PLOT_PALETTE.len()]));
            let points = if plot_kind == PlotKind::Scatter {
                item["points"]
                    .as_array()
                    .ok_or("scatter plot points must be an array")?
                    .iter()
                    .map(|point| {
                        let pair = point
                            .as_array()
                            .filter(|pair| pair.len() == 2)
                            .ok_or("scatter plot points must be [x, y] pairs")?;
                        let x = pair[0].as_f64().ok_or("scatter x values must be numeric")?;
                        let y = pair[1].as_f64().ok_or("scatter y values must be numeric")?;
                        Ok((x, y))
                    })
                    .collect::<Result<Vec<_>, String>>()?
            } else {
                item["values"]
                    .as_array()
                    .ok_or("plot values must be an array")?
                    .iter()
                    .enumerate()
                    .map(|(index, value)| {
                        value
                            .as_f64()
                            .map(|value| (index as f64, value))
                            .ok_or_else(|| "plot values must be numeric".to_owned())
                    })
                    .collect::<Result<Vec<_>, String>>()?
            };
            Ok(PlotSeries {
                name,
                points,
                color,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let labels = node["labels"]
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let height = node["height"].as_f64().unwrap_or(320.) as f32;
    Ok(Widget::plot(
        id,
        title,
        PlotData {
            kind: plot_kind,
            series,
            labels,
        },
        height,
    ))
}

const PLOT_PALETTE: [u32; 8] = [
    0x4f86f7, 0xe76f51, 0x2a9d8f, 0xe9c46a, 0x8b5cf6, 0x64748b, 0xec4899, 0x84a98c,
];

fn parse_color(value: &str) -> Result<Color, String> {
    let hex = value
        .strip_prefix('#')
        .filter(|hex| hex.len() == 6)
        .ok_or("plot color must be a six-digit hex color")?;
    u32::from_str_radix(hex, 16)
        .map(Color::hex)
        .map_err(|_| "plot color must be a six-digit hex color".into())
}

fn display(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| {
        if value.is_null() {
            String::new()
        } else {
            value.to_string()
        }
    })
}
fn size_group(mut group: Widget) -> Widget {
    let height = match group.layout {
        Layout::Horizontal { .. } => group
            .children
            .iter()
            .map(|child| child.size.height)
            .fold(0., f32::max),
        Layout::Tiles { columns } => group
            .children
            .chunks(columns.max(1))
            .map(|row| row.iter().map(|child| child.size.height).fold(0., f32::max) + 8.)
            .sum::<f32>(),
        _ => {
            group
                .children
                .iter()
                .map(|child| child.size.height)
                .sum::<f32>()
                + group.children.len().saturating_sub(1) as f32
                    * match group.layout {
                        Layout::Vertical { gap, .. } => gap,
                        _ => 12.,
                    }
        }
    };
    if matches!(group.layout, Layout::Horizontal { .. }) {
        for child in &mut group.children {
            if child.size.width.is_none() {
                child.size.grow = 1.;
            }
        }
    }
    group.size.height = height;
    group
}

struct App {
    session: Rc<RefCell<Session>>,
    view: View,
    title: String,
    close: bool,
}
impl Application for App {
    fn configure(&mut self, ui: &mut Ui) {
        let session = self.session.borrow();
        ui.set_theme(theme::named(
            session
                .document
                .get("theme")
                .and_then(Value::as_str)
                .unwrap_or("linen"),
        ));
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn initial_size(&self) -> (f64, f64) {
        let session = self.session.borrow();
        (
            session
                .document
                .get("width")
                .and_then(Value::as_f64)
                .unwrap_or(680.),
            session
                .document
                .get("height")
                .and_then(Value::as_f64)
                .unwrap_or(560.),
        )
    }
    fn should_close(&self) -> bool {
        self.close || self.session.borrow().output.error.is_some()
    }
    fn view(&mut self, viewport: Rect) -> Widget {
        let mut session = self.session.borrow_mut();
        match self
            .view
            .build(&session.widgets, &session.output.values, viewport)
        {
            Ok(tree) => tree,
            Err(error) => {
                session.output.error = Some(error);
                Widget::column(Id::from("@jgui:error"), vec![])
            }
        }
    }
    fn event(&mut self, event: Event, _ui: &mut Ui) {
        let (id, kind, value) = match event {
            Event::Click(id) => (id, "click", Value::Bool(true)),
            Event::Changed(id, value) => {
                let value = match value {
                    UiValue::Bool(value) => json!(value),
                    UiValue::Number(value) => json!(value),
                    UiValue::Text(value) => json!(value),
                    UiValue::Selection(index) => json!(index),
                };
                (id, "change", value)
            }
            _ => return,
        };
        let Some(key) = self.view.ids.get(&id).cloned() else {
            return;
        };
        let mut session = self.session.borrow_mut();
        let Some(node) = find_widget(&session.widgets, &key).cloned() else {
            // Generated IDs belong to layout controls such as disclosures.
            if kind == "click" {
                let open = self.view.collapsed.entry(key).or_insert(true);
                *open = !*open;
            }
            return;
        };
        if node["kind"] == "collapsible" && kind == "click" {
            let open = self.view.collapsed.entry(key).or_insert(true);
            *open = !*open;
            return;
        }
        let value = if node["kind"] == "select" || node["kind"] == "radio" {
            let Some(value) = value
                .as_u64()
                .and_then(|i| node["options"].get(i as usize))
                .cloned()
            else {
                return;
            };
            value
        } else if node["kind"] == "number_input" {
            let draft = value.as_str().unwrap_or("").to_owned();
            self.view.drafts.insert(key.clone(), draft.clone());
            let Some(number) = draft.parse::<f64>().ok().filter(|n| n.is_finite()) else {
                return;
            };
            json!(number)
        } else {
            value
        };
        match session.event(&key, kind, value) {
            Ok(close) => self.close |= close,
            Err(error) => session.output.error = Some(format!("event callback failed: {error}")),
        }
    }
}

pub(crate) fn run(session: Session) -> Result<Value, String> {
    // Fail before opening a window for unsupported controls, including hidden
    // controls when they later become visible through a reactive view.
    let mut check = View::new();
    check.build(
        &session.widgets,
        &session.output.values,
        Rect::new(0., 0., 680., 560.),
    )?;
    let title = session
        .document
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("JGUI")
        .to_owned();
    let session = Rc::new(RefCell::new(session));
    ::brockmann::run(App {
        session: Rc::clone(&session),
        view: View::new(),
        title,
        close: false,
    })
    .map_err(|e| format!("window failed: {e}"))?;
    let session = session.borrow();
    if let Some(error) = &session.output.error {
        return Err(error.clone());
    }
    Ok(json!({ "values": session.output.values, "clicked": session.output.clicked }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adapter_preserves_input_identity_across_reordering() {
        let mut view = View::new();
        let values = Map::new();
        let viewport = Rect::new(0., 0., 680., 560.);
        let a = json!({"kind":"text_input","id":"name","value":"Ada"});
        let b = json!({"kind":"button","id":"save","text":"Save"});
        let first = view
            .build(&[a.clone(), b.clone()], &values, viewport)
            .unwrap();
        let second = view.build(&[b, a], &values, viewport).unwrap();
        assert_eq!(first.children[0].id, second.children[1].id);
    }
    #[test]
    fn disabled_containers_and_visibility_reach_the_toolkit() {
        let mut view = View::new();
        let values = Map::new();
        let nodes = json!([{"kind":"column","enabled":false,"children":[{"kind":"button","id":"save"}]},{"kind":"label","visible":false}]);
        let tree = view
            .build(
                nodes.as_array().unwrap(),
                &values,
                Rect::new(0., 0., 680., 560.),
            )
            .unwrap();
        assert_eq!(tree.children.len(), 1);
        assert!(tree.children[0].disabled);
    }
    unsafe extern "C" fn mock_callback(id: u64, ptr: *const u8, len: usize) -> ject_native::Buffer {
        let args: Vec<Value> =
            serde_json::from_slice(unsafe { std::slice::from_raw_parts(ptr, len) }).unwrap();
        let value = match id {
            1 => json!([
                {"kind":"heading","text":format!("Count: {}", args[0]["count"])},
                {"kind":"button","id":"increment","text":"Increment"},
                {"kind":"button","id":"close","text":"Close","close":true}
            ]),
            2 if args[0]["id"] == "increment" => {
                json!({"set":{"count":args[0]["values"]["count"].as_i64().unwrap()+1}})
            }
            _ => Value::Null,
        };
        ject_native::Buffer::from_vec(serde_json::to_vec(&json!({"ok":value})).unwrap())
    }

    #[test]
    fn toolkit_pointer_events_update_ject_state_rebuild_the_view_and_close() {
        use ::brockmann::{
            ui::{Input, PointerButton},
            Point,
        };
        let host = ject_native::HostV1 {
            call_callback: mock_callback,
            free_buffer: ject_native::free_buffer,
        };
        let document = json!({"widgets":[],"state":{"count":0},"view":ject_native::callback(1),"on_event":ject_native::callback(2)});
        let session = Rc::new(RefCell::new(
            Session::new(document.as_object().unwrap().clone(), &host).unwrap(),
        ));
        let mut app = App {
            session: Rc::clone(&session),
            view: View::new(),
            title: "Counter".into(),
            close: false,
        };
        let mut ui = Ui::new(theme::named("metro-dark"));
        let viewport = Rect::new(0., 0., 680., 560.);
        let tree = app.view(viewport);
        ui.reconcile(tree, viewport);
        for key in ["increment", "close"] {
            let (rect, _) = ui.widget_geometry(Id::from(key)).unwrap();
            let position = Point::new(rect.x + rect.w / 2., rect.y + rect.h / 2.);
            ui.input(Input::Down {
                position,
                button: PointerButton::Primary,
                touch: false,
            });
            let events: Vec<_> = ui.drain_events().collect();
            assert!(!events.is_empty(), "button must produce a toolkit event");
            for event in events {
                app.event(event, &mut ui);
            }
            ui.input(Input::Up(position));
            let tree = app.view(viewport);
            ui.reconcile(tree, viewport);
        }
        assert_eq!(session.borrow().output.values["count"], json!(1));
        assert_eq!(session.borrow().widgets[0]["text"], json!("Count: 1"));
        assert!(app.should_close());
    }

    #[test]
    fn grids_use_available_width_and_collapsed_defaults_are_preserved() {
        let mut view = View::new();
        let values = Map::new();
        let nodes = json!([
            {"kind":"grid","columns":2,"spacing":8,"children":[{"kind":"button","id":"a"},{"kind":"button","id":"b"}]},
            {"kind":"collapsible","text":"More","open":false,"children":[{"kind":"label","text":"Hidden"}]}
        ]);
        let tree = view
            .build(
                nodes.as_array().unwrap(),
                &values,
                Rect::new(0., 0., 680., 560.),
            )
            .unwrap();
        assert_eq!(
            tree.children[0].children[0].children[0].size.width,
            Some(312.)
        );
        assert_eq!(tree.children[1].children.len(), 1);
        assert!(!view.collapsed["@jgui:root/1"]);
    }

    #[test]
    fn native_plot_renders_and_supports_wheel_zoom_and_drag_pan() {
        use ::brockmann::{
            scene::Primitive,
            ui::{Input, PointerButton},
            Point,
        };
        let nodes = json!([{
            "kind":"plot",
            "id":"metrics",
            "text":"Resource usage",
            "chart":"line",
            "height":280,
            "labels":["a","b","c"],
            "series":[{"name":"CPU","values":[0,5,1],"color":"#4f86f7"}]
        }]);
        let mut view = View::new();
        let tree = view
            .build(
                nodes.as_array().unwrap(),
                &Map::new(),
                Rect::new(0., 0., 720., 560.),
            )
            .unwrap();
        assert!(tree.children[0].plot.is_some());

        let mut ui = Ui::new(theme::named("metro-dark"));
        let viewport = Rect::new(0., 0., 720., 560.);
        ui.reconcile(tree, viewport);
        let (bounds, _) = ui.widget_geometry(Id::from("metrics")).unwrap();
        let pointer = Point::new(bounds.x + bounds.w / 2., bounds.y + bounds.h / 2.);
        let series_lines = |scene: &::brockmann::scene::Scene| {
            scene
                .draws
                .iter()
                .filter_map(|draw| match &draw.primitive {
                    Primitive::Line {
                        from, to, width, ..
                    } if *width == 2. => Some((*from, *to)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        ui.input(Input::Move(pointer));
        let initial = series_lines(&ui.scene());
        assert!(
            !initial.is_empty(),
            "native line chart must produce plot strokes"
        );
        ui.input(Input::Wheel { delta: 1. });
        let zoomed = series_lines(&ui.scene());
        assert_ne!(initial, zoomed, "wheel input should zoom the plotted data");

        ui.input(Input::Down {
            position: pointer,
            button: PointerButton::Primary,
            touch: false,
        });
        let moved = Point::new(pointer.x + 24., pointer.y + 12.);
        ui.input(Input::Move(moved));
        ui.input(Input::Up(moved));
        let panned = series_lines(&ui.scene());
        assert_ne!(zoomed, panned, "drag input should pan the plotted data");
    }
}
