//! The palette seam: `Flavor` on the Model, and `ui::draw` composing it into a
//! frame.
//!
//! `main.rs` is unreachable from the suite — `tests/cli_args.rs` says so, and
//! spawning the binary with no arguments launches the TUI and hangs. So without
//! `ui::draw` the composition it performs would be verifiable only by
//! inspection. These drive the same function `main.rs` calls.
//!
//! Fields are set directly rather than through the Omnibox: this file exists a
//! step before `Action::Omnibox` does, and the command-driven half is pinned
//! separately in `tests/omnibox_render.rs`.

use chrono::{TimeZone, Utc};
use oxidone::app::{update, Message, Model};
use oxidone::config::Flavor;
use oxidone::domain::{List, ListId, Selection, Status, Task, TaskId};
use oxidone::ui::{self, theme::Theme};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

/// One List holding one Completed and one open Task, selected — enough for the
/// sidebar and pane completion meters to draw. A bare `Model::new()` has an empty
/// sidebar and no meter at all, so the `ascii` test below could not tell braille
/// from its fallback with one.
fn model_with_a_meter() -> Model {
    let list = List {
        id: ListId("l".into()),
        title: "L".into(),
        etag: String::new(),
        updated: Utc.timestamp_opt(0, 0).unwrap(),
    };
    let task = |id: &str, status: Status| Task {
        id: TaskId(id.into()),
        list: ListId("l".into()),
        parent: None,
        title: id.into(),
        notes: None,
        status,
        due: None,
        completed_at: (status == Status::Completed).then(|| Utc.timestamp_opt(1, 0).unwrap()),
        links: Vec::new(),
        position: format!("{id:0>20}"),
        etag: String::new(),
        updated: Utc.timestamp_opt(0, 0).unwrap(),
    };
    let mut model = Model::new();
    update(&mut model, Message::ListsLoaded(vec![list]));
    model.selected = Selection::List(0);
    update(
        &mut model,
        Message::TasksLoaded(
            ListId("l".into()),
            vec![task("a", Status::Completed), task("b", Status::NeedsAction)],
        ),
    );
    model
}

/// The background colour `ui::draw` paints, which is the palette's `base`.
fn drawn_background(model: &Model) -> Color {
    let mut terminal =
        Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("TestBackend terminal");
    terminal.draw(|frame| ui::draw(model, frame)).expect("draw");
    terminal.backend().buffer()[(0, 0)].bg
}

/// Every symbol `ui::draw` puts on screen, joined — enough to tell braille from
/// its ASCII fallback.
fn drawn_symbols(model: &Model) -> String {
    let mut terminal =
        Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("TestBackend terminal");
    terminal.draw(|frame| ui::draw(model, frame)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
        .map(|(x, y)| buffer[(x, y)].symbol().to_string())
        .collect()
}

/// `as_str` must never name a flavor `Theme::from_flavor` does not know.
///
/// Its `_ => mocha` arm is fail-open, so an unrecognised name silently returns
/// Mocha's palette — and would then collide with `Mocha`'s own entry. Pairwise
/// distinctness catches that for every variant at once.
///
/// Written over `Color` rather than over whole palettes because `Theme` derives
/// neither `Debug` nor `PartialEq`; `Color` derives both, and `base` differs
/// across all four Catppuccin flavors.
#[test]
fn every_flavor_names_a_distinct_palette() {
    let bases: Vec<(Flavor, Color)> = Flavor::ALL
        .iter()
        .map(|f| (*f, Theme::from_flavor(f.as_str()).base))
        .collect();

    for (i, (a, base_a)) in bases.iter().enumerate() {
        for (b, base_b) in &bases[i + 1..] {
            assert_ne!(
                base_a, base_b,
                "{a:?} and {b:?} paint the same background — one of them fell \
                 through `from_flavor`'s default arm"
            );
        }
    }
}

/// The seam composes: the Model's flavor is what reaches the frame.
#[test]
fn draw_paints_the_models_flavor() {
    let mut model = Model::new();
    model.flavor = Flavor::Mocha;
    let mocha = drawn_background(&model);

    model.flavor = Flavor::Latte;
    let latte = drawn_background(&model);

    assert_ne!(
        mocha, latte,
        "`ui::draw` ignored `Model::flavor` — both frames painted the same background"
    );
    assert_eq!(latte, Theme::from_flavor("latte").base);
}

/// And the Model's `ascii` reaches the renderer the same way. The sidebar's
/// completion meters are braille (U+2800 block) unless it is set.
#[test]
fn draw_honours_the_models_ascii_flag() {
    let mut model = model_with_a_meter();
    model.ascii = false;
    let braille = drawn_symbols(&model)
        .chars()
        .any(|c| ('\u{2800}'..='\u{28FF}').contains(&c));

    model.ascii = true;
    let braille_when_ascii = drawn_symbols(&model)
        .chars()
        .any(|c| ('\u{2800}'..='\u{28FF}').contains(&c));

    assert!(
        braille,
        "no braille drawn with `ascii` off — the fixture cannot tell the two apart"
    );
    assert!(
        !braille_when_ascii,
        "`ui::draw` ignored `Model::ascii` — braille survived the fallback"
    );
}
