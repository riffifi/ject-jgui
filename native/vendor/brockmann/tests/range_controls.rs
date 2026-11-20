use brockmann::{
    scene::Primitive,
    ui::{PointerButton, Value},
    *,
};
const VIEW: Rect = Rect::new(0., 0., 640., 480.);
fn down(ui: &mut Ui, x: f32, y: f32) {
    ui.input(Input::Down {
        position: Point::new(x, y),
        button: PointerButton::Primary,
        touch: false,
    });
}
fn number(ui: &mut Ui) -> f64 {
    ui.drain_events()
        .find_map(|e| {
            if let Event::Changed(_, Value::Number(v)) = e {
                Some(v)
            } else {
                None
            }
        })
        .expect("range change")
}
fn knob_position(ui: &Ui) -> f32 {
    ui.scene()
        .draws
        .iter()
        .find_map(|d| {
            if let Primitive::Arc {
                thickness, sweep, ..
            } = d.primitive
            {
                if thickness >= 4. {
                    Some(sweep / (std::f32::consts::PI * 1.5))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .unwrap()
}

#[test]
fn vertical_fader_preserves_thumb_grab_and_tracks_pointer() {
    let mut ui = Ui::default();
    ui.reconcile(
        Widget::vertical_slider("f", "level", 50., 0., 100., 50.)
            .at(Rect::new(24., 24., 96., 200.)),
        VIEW,
    );
    // The track spans y=52..184, with a 50% thumb at y=118.
    down(&mut ui, 72., 124.);
    assert!((number(&mut ui) - 50.).abs() < 1e-6);
    ui.input(Input::Move(Point::new(72., 91.)));
    assert!((number(&mut ui) - 75.).abs() < 1e-6);
    ui.input(Input::Move(Point::new(72., -500.)));
    assert_eq!(number(&mut ui), 100.);
    ui.input(Input::Leave);
    assert!(ui.dragging());
    ui.input(Input::Up(Point::new(72., -500.)));
    assert!(!ui.dragging());
}
#[test]
fn hardware_values_render_in_same_frame_when_immediate() {
    let mut ui = Ui::default();
    let widget = |v| Widget::knob("k", "tone", v, 0., 100., 50.).immediate();
    ui.reconcile(widget(10.), VIEW);
    ui.advance(1.);
    ui.reconcile(widget(90.), VIEW);
    assert!((knob_position(&ui) - 0.9).abs() < 1e-5);
    // Ordinary programmatic transitions retain their interpolation.
    ui.reconcile(Widget::knob("k", "tone", 20., 0., 100., 50.), VIEW);
    assert!((knob_position(&ui) - 0.9).abs() < 1e-5);
    ui.advance(0.25);
    assert!((knob_position(&ui) - 0.2).abs() < 1e-5);
}
#[test]
fn captured_controls_accept_quantized_backend_echo_without_easing() {
    let mut ui = Ui::default();
    let widget = |v| Widget::knob("k", "tone", v, 0., 100., 50.).at(Rect::new(24., 24., 96., 112.));
    ui.reconcile(widget(50.), VIEW);
    down(&mut ui, 72., 60.);
    ui.drain_events().for_each(drop);
    ui.input(Input::Move(Point::new(72., 40.)));
    assert_eq!(number(&mut ui), 60.);
    // A backend may quantize a continuous drag onto a musical value.
    ui.reconcile(widget(f64::from(0.75_f32) * 100.), VIEW);
    assert!((knob_position(&ui) - 0.75).abs() < 1e-5);
}
#[test]
fn fader_ends_and_readouts_stay_inside_control_in_both_themes() {
    for light in [false, true] {
        for value in [0., 50., 100.] {
            let mut ui = Ui::new(Theme::new(light, Accent::Orange));
            let bounds = Rect::new(24., 24., 96., 200.);
            ui.reconcile(
                Widget::vertical_slider("f", "master", value, 0., 100., 50.)
                    .format_value(|v| format!("{v:.0}%"))
                    .at(bounds),
                VIEW,
            );
            let scene = ui.scene();
            assert!(scene.draws.iter().any(
                |d| matches!(&d.primitive,Primitive::Text{text,..}if text==&format!("{value:.0}%"))
            ));
            for d in scene.draws {
                if let Primitive::Rect { bounds: r, .. } = d.primitive
                    && r.w == 28.
                    && r.h == 12.
                {
                    assert!(r.y >= bounds.y && r.bottom() <= bounds.bottom());
                }
            }
        }
    }
}
