use island_design::agent_pulse as t;
use island_plugin_agent_pulse::{
    list::Viewport,
    model::Session,
    render::{self, Action, Page, Renderer, Ui},
};

fn sessions() -> Vec<Session> {
    (0..8)
        .map(|i| {
            let mut s = render::samples(100_000).remove(i % 3);
            s.id = format!("session-{i}");
            s.project = format!("project-{i}");
            s
        })
        .collect()
}

#[test]
fn continuous_scrolling_reaches_every_row_without_changing_selection() {
    let sessions = sessions();
    let mut ui = Ui::default();
    let h = render::preferred_height(sessions.len(), Page::Sessions);
    ui.list.scroll(t::ROW * 3.5, sessions.len(), h);
    assert_eq!(ui.selected, 0);
    assert_eq!(ui.list.rows(sessions.len(), h), 3..7);
    assert_eq!(
        render::hit(340, h, &sessions, &ui, 40.0, t::HEADER),
        Some(Action::Row(3))
    );
    assert_eq!(
        render::hit(340, h, &sessions, &ui, 40.0, t::HEADER + 27.0),
        Some(Action::Row(4))
    );
    assert_eq!(
        render::hit(340, h, &sessions, &ui, 40.0, h as f32 - t::BOTTOM),
        None
    );
    assert_eq!(
        render::hit(340, h, &sessions, &ui, 320.0, 13.0),
        Some(Action::Menu)
    );
    assert_eq!(
        render::hit(340, h, &sessions, &ui, 338.0, 80.0),
        Some(Action::Scrollbar)
    );
    for delta in [0.0, f32::NAN, f32::INFINITY] {
        ui.list.scroll(delta, sessions.len(), h);
        assert_eq!(ui.list.offset, t::ROW * 3.5);
    }
    ui.list.scroll(1e6, sessions.len(), h);
    assert_eq!(ui.list.rows(sessions.len(), h), 5..8);
    assert_eq!(
        render::hit(340, h, &sessions, &ui, 40.0, 170.0),
        Some(Action::Row(7))
    );
    ui.list.scroll(-1e6, sessions.len(), h);
    assert_eq!(ui.list.offset, 0.0);
}

#[test]
fn keyboard_reveal_and_scrollbar_drag_share_the_same_bounds() {
    let mut list = Viewport::default();
    list.reveal(7, 8, 192);
    assert_eq!(list.offset, 260.0);
    list.reveal(0, 8, 192);
    assert_eq!(list.offset, 0.0);
    let thumb = list.thumb(8, 192).unwrap();
    list.press_scrollbar(thumb.y + 3.0, 8, 192);
    list.drag_to(190.0, 8, 192);
    assert_eq!(list.offset, 260.0);
    list.end_drag();
    list.drag_to(0.0, 8, 192);
    assert_eq!(list.offset, 260.0);
    list.press_scrollbar(t::HEADER, 8, 192);
    assert_eq!(list.offset, 0.0);
    assert!(list.thumb(2, 140).is_none());
    list.offset = f32::NAN;
    list.clamp(0, 132);
    assert_eq!(list.offset, 0.0);
}

#[test]
fn live_updates_preserve_the_viewport_anchor_and_cancel_a_stale_click() {
    let old = sessions();
    let mut ui = Ui::default();
    ui.list.scroll(t::ROW * 4.0 + 10.0, old.len(), 192);
    ui.pressed = Some(Action::Row(4));
    let mut new = old.clone();
    new.remove(0);
    ui.reconcile_sessions(&old, &new, 192);
    assert_eq!(ui.list.offset, t::ROW * 3.0 + 10.0);
    assert_eq!(
        new[ui.list.hit(new.len(), 192, t::HEADER).unwrap()].id,
        old[4].id
    );
    assert_eq!(ui.pressed, None);
    ui.reconcile_sessions(&new, &new[..1], 88);
    assert_eq!(ui.list.offset, 0.0);
}

#[test]
fn scrolled_rows_are_clipped_below_the_fixed_header_and_above_the_footer() {
    let renderer = Renderer::new();
    let sessions = sessions();
    for scale in [1.0, 2.0, 3.0] {
        let mut ui = Ui::default();
        let before = renderer.frame(340, 192, scale, &sessions, &ui, 100_000);
        ui.list.scroll(101.5, sessions.len(), 192);
        let after = renderer.frame(340, 192, scale, &sessions, &ui, 100_000);
        after.validate().unwrap();
        let header = (t::HEADER * scale) as usize * before.width as usize * 4;
        let footer = ((192.0 - t::BOTTOM) * scale) as usize * before.width as usize * 4;
        assert_eq!(
            &before.pixels[..header],
            &after.pixels[..header],
            "rows must not overwrite header"
        );
        assert_eq!(
            &before.pixels[footer..],
            &after.pixels[footer..],
            "rows must not leak into footer"
        );
        assert_ne!(
            &before.pixels[header..footer],
            &after.pixels[header..footer]
        );
    }
}
